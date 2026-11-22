use chrono::{DateTime, Duration, FixedOffset, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::account::toss_account::{account_client, HIGH_VALUE_KRW};
use crate::auth::otp;
use crate::errors::{ApiError, ApiResult};
use crate::market::extras::{market_status, normalize};
use crate::market::models::Market;
use crate::state::AppState;
use crate::storage::find_where;
use crate::trading::ticks::{fmt_price, round_price};
use crate::users::repository::{self as users, now_iso};

static ORDER_ID_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Za-z0-9\-_=+/]{1,128}$").unwrap());

pub fn kst_today() -> String {
    let kst = FixedOffset::east_opt(9 * 3600).unwrap();
    Utc::now().with_timezone(&kst).date_naive().to_string()
}

pub fn today_order_count(state: &AppState, uid: &str) -> usize {
    let today = kst_today();
    find_where(state.store.as_ref(), "orders", |o| {
        o.get("user_id").and_then(|v| v.as_str()) == Some(uid)
            && o.get("date").and_then(|v| v.as_str()) == Some(today.as_str())
    })
    .len()
}

pub fn trading_status(state: &AppState, user: &Value) -> Value {
    let until = user.get("trading_unlocked_until").and_then(|v| v.as_str());
    let unlocked = until
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc) > Utc::now())
        .unwrap_or(false);
    let uid = user["id"].as_str().unwrap_or("");
    let cfg = &state.config;
    json!({
        "unlocked": unlocked,
        "unlocked_until": if unlocked { until } else { None },
        "today_orders": today_order_count(state, uid),
        "limits": {
            "max_orders_per_day": cfg.trading_max_orders_per_day,
            "max_order_krw": cfg.trading_max_order_krw,
            "max_order_usd": cfg.trading_max_order_usd,
            "unlock_minutes": cfg.trading_unlock_min,
        },
    })
}

pub fn unlock(state: &AppState, user: &Value, code: &str) -> ApiResult<Value> {
    let verified = otp::verify(state, user, code)?;
    let until = (Utc::now() + Duration::minutes(state.config.trading_unlock_min)).to_rfc3339();
    let uid = verified["id"].as_str().unwrap_or("");
    let updated = users::update_user(state, uid, json!({ "trading_unlocked_until": until }))?;
    Ok(trading_status(state, &updated))
}

pub fn lock(state: &AppState, user: &Value) -> ApiResult<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    let updated = users::update_user(state, uid, json!({ "trading_unlocked_until": Value::Null }))?;
    Ok(trading_status(state, &updated))
}

pub fn require_unlocked(state: &AppState, user: &Value) -> ApiResult<()> {
    let unlocked = trading_status(state, user)
        .get("unlocked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !unlocked {
        return Err(ApiError::forbidden(
            "trading_locked",
            "거래가 잠겨 있습니다. OTP로 잠금을 해제하세요",
        ));
    }
    Ok(())
}

pub fn check_order_value(state: &AppState, market: Market, value: f64) -> ApiResult<()> {
    let cap = match market {
        Market::KR => state.config.trading_max_order_krw,
        Market::US => state.config.trading_max_order_usd,
    };
    if value > cap {
        let unit = if market == Market::KR { "원" } else { "달러" };
        return Err(ApiError::bad_request(
            "order_value_limit",
            format!("주문 1건 한도({cap:.0}{unit})를 넘습니다"),
        ));
    }
    Ok(())
}

pub fn check_limits(state: &AppState, uid: &str, market: Market, value: f64) -> ApiResult<()> {
    if today_order_count(state, uid) >= state.config.trading_max_orders_per_day {
        return Err(ApiError::too_many_requests(
            "daily_order_limit",
            format!(
                "오늘 주문 한도({}건)에 도달했습니다",
                state.config.trading_max_orders_per_day
            ),
        ));
    }
    check_order_value(state, market, value)
}

fn parse_pos_num(v: Option<&Value>, name: &str) -> ApiResult<f64> {
    let f = v
        .and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| ApiError::bad_request("invalid_number", format!("{name}: 숫자를 입력하세요")))?;
    if !f.is_finite() || f <= 0.0 {
        return Err(ApiError::bad_request(
            "invalid_number",
            format!("{name}: 0보다 커야 합니다"),
        ));
    }
    Ok(f)
}

pub async fn build_order(state: &AppState, uid: &str, body: &Value) -> ApiResult<Value> {
    let market = Market::parse(body.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let m = market.as_str();
    let symbol = body
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if !market.validate_symbol(&symbol) {
        return Err(ApiError::code_only(400, "invalid_symbol"));
    }
    let side = body
        .get("side")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_uppercase();
    if side != "BUY" && side != "SELL" {
        return Err(ApiError::code_only(400, "invalid_side"));
    }
    let order_type = body
        .get("order_type")
        .and_then(|v| v.as_str())
        .unwrap_or("LIMIT")
        .to_uppercase();
    if order_type != "LIMIT" && order_type != "MARKET" {
        return Err(ApiError::code_only(400, "invalid_order_type"));
    }
    let tif = body
        .get("time_in_force")
        .and_then(|v| v.as_str())
        .unwrap_or("DAY")
        .to_uppercase();
    if !matches!(tif.as_str(), "DAY" | "OPG" | "CLS")
        || (tif == "OPG" && m != "KR")
        || (tif == "CLS" && (m != "US" || order_type != "LIMIT"))
    {
        return Err(ApiError::bad_request(
            "invalid_time_in_force",
            "OPG(시가 단일가)는 국내, CLS(종가)는 미국 지정가만 가능합니다",
        ));
    }

    let qty_f = parse_pos_num(body.get("quantity"), "수량")?;
    if (qty_f - qty_f.trunc()).abs() > 1e-9 {
        return Err(ApiError::bad_request(
            "invalid_quantity",
            "수량은 정수여야 합니다",
        ));
    }
    let qty = qty_f as i64;

    let price = if order_type == "LIMIT" {
        let raw = parse_pos_num(body.get("price"), "가격")?;
        Some(round_price(m, raw, if side == "BUY" { "down" } else { "up" }))
    } else {
        None
    };

    let (client, seq) = account_client(state, uid)?;
    let mut p_sym = HashMap::new();
    p_sym.insert("symbols".to_string(), symbol.clone());
    let prices = client
        .request("GET", "/api/v1/prices", Some(&p_sym), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let current = prices
        .first()
        .and_then(|r| r.get("lastPrice"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| ApiError::not_found("stock_not_found", &symbol))?;

    let est_price = price.unwrap_or(current);
    let value = (qty as f64) * est_price;
    let mut problems: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    if market == Market::KR {
        if let Some(px) = price {
            let mut pl = HashMap::new();
            pl.insert("symbol".to_string(), symbol.clone());
            let lim = normalize(
                client
                    .request("GET", "/api/v1/price-limits", Some(&pl), None, None)
                    .await?
                    .unwrap_or_else(|| json!({})),
                "",
            );
            let up = lim.get("upperLimitPrice").and_then(|v| v.as_f64());
            let lo = lim.get("lowerLimitPrice").and_then(|v| v.as_f64());
            if up.is_some_and(|u| px > u) || lo.is_some_and(|l| px < l) {
                problems.push(format!(
                    "가격이 오늘 상·하한가({:.0} ~ {:.0}) 밖입니다",
                    lo.unwrap_or(0.0),
                    up.unwrap_or(0.0)
                ));
            }
        }
    }

    let currency = if market == Market::KR { "KRW" } else { "USD" };
    if side == "BUY" {
        let mut pc = HashMap::new();
        pc.insert("currency".to_string(), currency.to_string());
        let bp = normalize(
            client
                .request("GET", "/api/v1/buying-power", Some(&pc), None, Some(seq))
                .await?
                .unwrap_or_else(|| json!({})),
            "",
        );
        let cash = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if value > cash {
            problems.push(format!(
                "매수 가능 금액({cash:.2} {currency})이 부족합니다 (필요 약 {value:.2})"
            ));
        }
        if order_type == "MARKET" {
            warnings.push("시장가 매수는 체결 가격이 현재가와 다를 수 있습니다".to_string());
        }
    } else {
        let mut ps = HashMap::new();
        ps.insert("symbol".to_string(), symbol.clone());
        let sellable = client
            .request("GET", "/api/v1/sellable-quantity", Some(&ps), None, Some(seq))
            .await?
            .and_then(|v| v.get("sellableQuantity").cloned())
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0);
        if (qty as f64) > sellable {
            problems.push(format!("매도 가능 수량({sellable}주)보다 많습니다"));
        }
    }

    if let Some(px) = price {
        if current > 0.0 && (px / current - 1.0).abs() > 0.1 {
            warnings.push(format!(
                "주문가가 현재가와 {:.1}% 차이 납니다",
                (px / current - 1.0).abs() * 100.0
            ));
        }
    }

    let session_status = match market_status(state, uid, m).await {
        Ok(st) => {
            if !st.get("open").and_then(|v| v.as_bool()).unwrap_or(false) {
                let lbl = st.get("label").and_then(|v| v.as_str()).unwrap_or("장 마감");
                warnings.push(format!(
                    "현재 {lbl} — 주문이 거부되거나 다음 거래 시간에 처리될 수 있습니다"
                ));
            }
            st
        }
        Err(_) => Value::Null,
    };

    if !problems.is_empty() {
        return Err(ApiError::bad_request("order_invalid", problems.join(" / ")));
    }

    let high_value = market == Market::KR && value >= HIGH_VALUE_KRW;
    let confirm_hv = body
        .get("confirm_high_value")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if high_value && !confirm_hv {
        return Err(ApiError::bad_request(
            "confirm_high_value_required",
            "1억 원 이상 주문입니다. 고액 주문 확인에 체크하세요",
        ));
    }

    let mut payload = json!({
        "clientOrderId": Uuid::new_v4().simple().to_string(),
        "symbol": symbol,
        "side": side,
        "orderType": order_type,
        "timeInForce": tif,
        "quantity": qty.to_string(),
    });
    if let Some(px) = price {
        payload
            .as_object_mut()
            .unwrap()
            .insert("price".to_string(), json!(fmt_price(m, px)));
    }
    if high_value {
        payload
            .as_object_mut()
            .unwrap()
            .insert("confirmHighValueOrder".to_string(), json!(true));
    }

    Ok(json!({
        "market": m,
        "payload": payload,
        "checks": {
            "current_price": current,
            "price": price,
            "estimated_value": value,
            "currency": currency,
            "session": session_status,
        },
        "warnings": warnings,
    }))
}

pub async fn submit(state: &AppState, built: &Value, uid: &str, source: &str) -> ApiResult<Value> {
    let market = Market::parse(built["market"].as_str().unwrap_or(""))?;
    let est_val = built["checks"]["estimated_value"].as_f64().unwrap_or(0.0);
    check_limits(state, uid, market, est_val)?;
    let (client, seq) = account_client(state, uid)?;
    let payload = &built["payload"];
    let result = normalize(
        client
            .request("POST", "/api/v1/orders", None, Some(payload), Some(seq))
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    );
    let cid = payload["clientOrderId"].as_str().unwrap_or("");
    state.store.put(
        "orders",
        cid,
        json!({
            "user_id": uid,
            "kind": "order",
            "source": source,
            "market": built["market"],
            "symbol": payload["symbol"],
            "side": payload["side"],
            "payload": payload,
            "result": result,
            "date": kst_today(),
            "created_at": now_iso(),
        }),
    );
    Ok(result)
}

pub async fn list_orders(
    state: &AppState,
    uid: &str,
    status: &str,
    symbol: Option<&str>,
) -> ApiResult<Vec<Value>> {
    let (client, seq) = account_client(state, uid)?;
    let mut params = HashMap::new();
    params.insert("status".to_string(), status.to_string());
    params.insert("limit".to_string(), "100".to_string());
    if let Some(s) = symbol {
        params.insert("symbol".to_string(), s.to_string());
    }
    if status == "CLOSED" {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let from_date = (Utc::now().with_timezone(&kst).date_naive() - Duration::days(30)).to_string();
        params.insert("from".to_string(), from_date);
    }

    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..5 {
        if let Some(ref c) = cursor {
            params.insert("cursor".to_string(), c.clone());
        }
        let r = client
            .request("GET", "/api/v1/orders", Some(&params), None, Some(seq))
            .await?
            .unwrap_or_else(|| json!({}));
        let norm = normalize(r.get("orders").cloned().unwrap_or_else(|| json!([])), "orders");
        if let Some(arr) = norm.as_array() {
            out.extend(arr.iter().cloned());
        }
        cursor = r.get("nextCursor").and_then(|v| v.as_str()).map(|s| s.to_string());
        let has_next = r.get("hasNext").and_then(|v| v.as_bool()).unwrap_or(false);
        if status == "OPEN" || !has_next || cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

fn check_order_id(order_id: &str) -> ApiResult<()> {
    if !ORDER_ID_RE.is_match(order_id) {
        return Err(ApiError::code_only(400, "invalid_order_id"));
    }
    Ok(())
}

pub async fn get_order(state: &AppState, uid: &str, order_id: &str) -> ApiResult<Value> {
    check_order_id(order_id)?;
    let (client, seq) = account_client(state, uid)?;
    Ok(normalize(
        client
            .request("GET", &format!("/api/v1/orders/{order_id}"), None, None, Some(seq))
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    ))
}

pub async fn cancel(state: &AppState, uid: &str, order_id: &str) -> ApiResult<Value> {
    check_order_id(order_id)?;
    let (client, seq) = account_client(state, uid)?;
    Ok(normalize(
        client
            .request(
                "POST",
                &format!("/api/v1/orders/{order_id}/cancel"),
                None,
                Some(&json!({})),
                Some(seq),
            )
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    ))
}

pub async fn modify(state: &AppState, uid: &str, order_id: &str, body: &Value) -> ApiResult<Value> {
    check_order_id(order_id)?;
    let current = get_order(state, uid, order_id).await?;
    let m = if current.get("currency").and_then(|v| v.as_str()) == Some("KRW") {
        "KR"
    } else {
        "US"
    };
    let market = Market::parse(m)?;
    let symbol = current.get("symbol").and_then(|v| v.as_str()).map(|s| s.to_string());
    let order_type = body
        .get("order_type")
        .and_then(|v| v.as_str())
        .or_else(|| current.get("orderType").and_then(|v| v.as_str()))
        .unwrap_or("LIMIT")
        .to_uppercase();
    if order_type != "LIMIT" && order_type != "MARKET" {
        return Err(ApiError::code_only(400, "invalid_order_type"));
    }

    let mut payload = json!({ "orderType": order_type });
    let mut price: Option<f64> = None;
    if order_type == "LIMIT" {
        let px = round_price(m, parse_pos_num(body.get("price"), "가격")?, "nearest");
        price = Some(px);
        payload
            .as_object_mut()
            .unwrap()
            .insert("price".to_string(), json!(fmt_price(m, px)));
    }
    let qty = if m == "KR" {
        let qv = body.get("quantity").or_else(|| current.get("quantity"));
        let q = parse_pos_num(qv, "수량")?;
        if (q - q.trunc()).abs() > 1e-9 {
            return Err(ApiError::bad_request(
                "invalid_quantity",
                "수량은 정수여야 합니다",
            ));
        }
        payload
            .as_object_mut()
            .unwrap()
            .insert("quantity".to_string(), json!((q as i64).to_string()));
        q
    } else {
        current
            .get("quantity")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };

    let (client, seq) = account_client(state, uid)?;
    if market == Market::KR {
        if let (Some(px), Some(ref sym)) = (price, &symbol) {
            let mut pl = HashMap::new();
            pl.insert("symbol".to_string(), sym.clone());
            let lim = normalize(
                client
                    .request("GET", "/api/v1/price-limits", Some(&pl), None, None)
                    .await?
                    .unwrap_or_else(|| json!({})),
                "",
            );
            let up = lim.get("upperLimitPrice").and_then(|v| v.as_f64());
            let lo = lim.get("lowerLimitPrice").and_then(|v| v.as_f64());
            if up.is_some_and(|u| px > u) || lo.is_some_and(|l| px < l) {
                return Err(ApiError::bad_request(
                    "order_invalid",
                    format!(
                        "가격이 오늘 상·하한가({:.0} ~ {:.0}) 밖입니다",
                        lo.unwrap_or(0.0),
                        up.unwrap_or(0.0)
                    ),
                ));
            }
        }
    }

    let mut est_price = price;
    if est_price.is_none() {
        if let Some(ref sym) = symbol {
            let mut ps = HashMap::new();
            ps.insert("symbols".to_string(), sym.clone());
            if let Ok(Some(Value::Array(rows))) =
                client.request("GET", "/api/v1/prices", Some(&ps), None, None).await
            {
                if let Some(lp) = rows.first().and_then(|r| r.get("lastPrice")).and_then(|v| {
                    v.as_f64()
                        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                }) {
                    est_price = Some(lp);
                }
            }
        }
        if est_price.is_none() {
            est_price = current.get("price").and_then(|v| v.as_f64());
        }
    }
    if qty > 0.0 && est_price.unwrap_or(0.0) > 0.0 {
        check_order_value(state, market, qty * est_price.unwrap())?;
    }

    Ok(normalize(
        client
            .request(
                "POST",
                &format!("/api/v1/orders/{order_id}/modify"),
                None,
                Some(&payload),
                Some(seq),
            )
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    ))
}
