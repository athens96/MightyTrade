use chrono::{Duration, NaiveDate, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::errors::{ApiError, ApiResult};
use crate::integrations::broker::BrokerClient;
use crate::market::extras::normalize;
use crate::market::models::Market;
use crate::state::AppState;
use crate::trading::ticks::{fmt_price, round_price};
use crate::users::credentials::{self, BrokerKind};

pub const HIGH_VALUE_KRW: f64 = 100_000_000.0;
static COND_ID_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Za-z0-9\-_]{1,64}$").unwrap());

pub fn account_client(state: &AppState, user_id: &str) -> ApiResult<(BrokerClient, i64)> {
    if credentials::active_broker(state, user_id) == BrokerKind::Kiwoom {
        let client = state.get_kiwoom_client(user_id)?;
        return Ok((BrokerClient::Kiwoom(client), 0));
    }
    let creds = credentials::get_toss(state, user_id).ok_or_else(|| {
        ApiError::conflict(
            "toss_credentials_required",
            "증권사 API 키(토스증권 또는 키움증권)를 먼저 등록하세요 (설정)",
        )
    })?;
    let seq = creds.account_seq.ok_or_else(|| {
        ApiError::conflict(
            "toss_account_missing",
            "토스증권 위탁 계좌를 찾지 못했습니다. 설정에서 키를 다시 등록하세요",
        )
    })?;
    let client = state.get_toss_client(user_id)?;
    Ok((BrokerClient::Toss(client), seq))
}

pub async fn summary(state: &AppState, user_id: &str) -> ApiResult<Value> {
    let (client, seq) = account_client(state, user_id)?;
    let holdings = normalize(
        client
            .request("GET", "/api/v1/holdings", None, None, Some(seq))
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    );
    let mut bp = serde_json::Map::new();
    for cur in ["KRW", "USD"] {
        let mut params = HashMap::new();
        params.insert("currency".to_string(), cur.to_string());
        let r = normalize(
            client
                .request("GET", "/api/v1/buying-power", Some(&params), None, Some(seq))
                .await?
                .unwrap_or_else(|| json!({})),
            "",
        );
        bp.insert(
            cur.to_string(),
            r.get("cashBuyingPower").cloned().unwrap_or(Value::Null),
        );
    }
    let is_mock = match &client {
        BrokerClient::Kiwoom(k) => Some(k.is_mock),
        BrokerClient::Toss(_) => None,
    };
    Ok(json!({
        "broker": client.kind().as_str(),
        "kiwoom_mock": is_mock,
        "holdings": holdings,
        "buying_power": Value::Object(bp),
    }))
}

pub async fn conditional_orders(
    state: &AppState,
    user_id: &str,
    status: &str,
) -> ApiResult<Vec<Value>> {
    let (client, seq) = account_client(state, user_id)?;
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..10 {
        let mut params = HashMap::new();
        params.insert("status".to_string(), status.to_string());
        params.insert("limit".to_string(), "100".to_string());
        if let Some(ref c) = cursor {
            params.insert("cursor".to_string(), c.clone());
        }
        let r = client
            .request("GET", "/api/v1/conditional-orders", Some(&params), None, Some(seq))
            .await?
            .unwrap_or_else(|| json!({}));
        let norm = normalize(
            r.get("conditionalOrders")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "conditionalOrders",
        );
        if let Some(arr) = norm.as_array() {
            out.extend(arr.iter().cloned());
        }
        cursor = r
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let has_next = r.get("hasNext").and_then(|v| v.as_bool()).unwrap_or(false);
        if !has_next || cursor.is_none() {
            break;
        }
    }
    Ok(out)
}

pub async fn cancel_conditional(state: &AppState, user_id: &str, order_id: &str) -> ApiResult<()> {
    if !COND_ID_RE.is_match(order_id) {
        return Err(ApiError::code_only(400, "invalid_order_id"));
    }
    let (client, seq) = account_client(state, user_id)?;
    client
        .request(
            "DELETE",
            &format!("/api/v1/conditional-orders/{order_id}"),
            None,
            None,
            Some(seq),
        )
        .await?;
    Ok(())
}

fn nested_num(body: &Value, path: &str) -> ApiResult<f64> {
    let mut cur = body;
    for part in path.split('.') {
        cur = cur.get(part).unwrap_or(&Value::Null);
    }
    let v = cur
        .as_f64()
        .or_else(|| cur.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| ApiError::bad_request("invalid_number", format!("{path}: 숫자를 입력하세요")))?;
    if !v.is_finite() || v <= 0.0 {
        return Err(ApiError::bad_request(
            "invalid_number",
            format!("{path}: 0보다 커야 합니다"),
        ));
    }
    Ok(v)
}

pub async fn build_oco(state: &AppState, user_id: &str, body: &Value) -> ApiResult<Value> {
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
    let qty_f = nested_num(body, "quantity")?;
    if (qty_f - qty_f.trunc()).abs() > 1e-9 {
        return Err(ApiError::bad_request(
            "invalid_quantity",
            "수량은 정수여야 합니다",
        ));
    }
    let qty = qty_f as i64;

    let tp_trigger = round_price(m, nested_num(body, "take_profit.trigger")?, "nearest");
    let tp_price = round_price(m, nested_num(body, "take_profit.price")?, "nearest");
    let sl_trigger = round_price(m, nested_num(body, "stop_loss.trigger")?, "nearest");
    let sl_price = round_price(m, nested_num(body, "stop_loss.price")?, "down");

    let today = Utc::now().date_naive();
    let exp_str = body
        .get("expire_date")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| (today + Duration::days(30)).to_string());
    let exp = NaiveDate::parse_from_str(&exp_str, "%Y-%m-%d")
        .map_err(|_| ApiError::code_only(400, "invalid_expire_date"))?;
    if exp < today || exp > today + Duration::days(365) {
        return Err(ApiError::bad_request(
            "invalid_expire_date",
            "만료일은 오늘부터 1년 이내여야 합니다",
        ));
    }

    let (client, seq) = account_client(state, user_id)?;
    let mut ps = HashMap::new();
    ps.insert("symbols".to_string(), symbol.clone());
    let prices = client
        .request("GET", "/api/v1/prices", Some(&ps), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let current = prices
        .first()
        .and_then(|r| r.get("lastPrice"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .ok_or_else(|| ApiError::not_found("stock_not_found", &symbol))?;

    let mut sq = HashMap::new();
    sq.insert("symbol".to_string(), symbol.clone());
    let sellable = client
        .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
        .await?
        .and_then(|v| v.get("sellableQuantity").cloned())
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);

    let mut problems = Vec::new();
    if !(tp_trigger > current && current > sl_trigger) {
        problems.push(format!(
            "익절 감시가({tp_trigger}) > 현재가({current}) > 손절 감시가({sl_trigger}) 순서여야 합니다"
        ));
    }
    if (qty as f64) > sellable {
        problems.push(format!("매도 가능 수량({sellable}주)보다 많습니다"));
    }
    if sl_price > sl_trigger {
        problems.push("손절 주문가는 손절 감시가 이하여야 체결 가능성이 높습니다".to_string());
    }
    if !problems.is_empty() {
        return Err(ApiError::bad_request("oco_invalid", problems.join(" / ")));
    }

    let value = (qty as f64) * current;
    let high_value = market == Market::KR && value >= HIGH_VALUE_KRW;
    if high_value
        && !body
            .get("confirm_high_value")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return Err(ApiError::bad_request(
            "confirm_high_value_required",
            "1억 원 이상 주문입니다. 고액 주문 확인에 체크하세요",
        ));
    }

    let mut payload = json!({
        "symbol": symbol,
        "type": "OCO",
        "quantity": qty.to_string(),
        "orderType": "LIMIT",
        "expireDate": exp.to_string(),
        "clientOrderId": Uuid::new_v4().simple().to_string(),
        "first": {
            "orderSide": "SELL",
            "triggerPrice": fmt_price(m, tp_trigger),
            "orderPrice": fmt_price(m, tp_price),
        },
        "second": {
            "orderSide": "SELL",
            "triggerPrice": fmt_price(m, sl_trigger),
            "orderPrice": fmt_price(m, sl_price),
        },
    });
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
            "sellable_quantity": sellable,
            "order_value": value,
            "take_profit_pct": (tp_trigger / current - 1.0) * 100.0,
            "stop_loss_pct": (sl_trigger / current - 1.0) * 100.0,
            "rounded": {
                "take_profit": [tp_trigger, tp_price],
                "stop_loss": [sl_trigger, sl_price],
            },
        },
        "account_seq": seq,
    }))
}

pub async fn submit_oco(state: &AppState, user_id: &str, built: &Value) -> ApiResult<Value> {
    let (client, seq) = account_client(state, user_id)?;
    Ok(normalize(
        client
            .request(
                "POST",
                "/api/v1/conditional-orders",
                None,
                Some(&built["payload"]),
                Some(seq),
            )
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    ))
}
