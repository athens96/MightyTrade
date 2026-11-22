//! Semi-automated '1-Click Confirm-to-Trade' ('승인 후 주문') staging & execution (MT-8).

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, executor, protections};
use crate::errors::{ApiError, ApiResult};
use crate::market::extras::normalize;
use crate::market::models::Market;
use crate::quant::data::cost_defaults;
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::trading::ticks::{fmt_price, round_price};

const LIVE_BUY_HEADROOM: f64 = 0.005;

fn cid(bot_id: &str, approval_id: &str, idx: usize) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:semi:{approval_id}:{idx}"))
}

pub fn is_expired(proposal: Option<&Value>, now: Option<DateTime<Utc>>) -> bool {
    let Some(prop) = proposal.filter(|v| v.is_object()) else {
        return true;
    };
    let Some(exp) = prop.get("expires_at").and_then(|v| v.as_str()) else {
        return false;
    };
    let now_dt = now.unwrap_or_else(Utc::now);
    DateTime::parse_from_rfc3339(exp)
        .map(|d| now_dt >= d.with_timezone(&Utc))
        .unwrap_or(false)
}

pub fn active_proposal(st: &Value, now: Option<DateTime<Utc>>) -> Option<Value> {
    let prop = st.get("pending_approval")?;
    if prop.is_null() || is_expired(Some(prop), now) {
        None
    } else {
        Some(prop.clone())
    }
}

pub async fn stage_from_scheduler(
    state: &AppState,
    bot: &mut Value,
    today: &str,
    session_end: DateTime<Utc>,
    now: Option<DateTime<Utc>>,
) -> ApiResult<Value> {
    let is_intra = executor::is_intraday(bot);
    let sk = bots::state_key(bot["mode"].as_str().unwrap_or("semi")).to_string();
    if !is_intra && bot["state"][&sk]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_staged" }));
    }

    let _ = executor::reconcile_live(state, bot, today).await;
    let pv = executor::preview(state, bot, today).await?;
    if is_intra {
        let (_, bars, _, _) = executor::load_intraday(state, bot).await?;
        let bar_time = *bars.time.last().unwrap_or(&0);
        if bot["state"][&sk]["last_bar_time"].as_i64() == Some(bar_time) {
            return Ok(json!({ "skipped": "already_staged_bar" }));
        }
        bot["state"][&sk]["last_bar_time"] = json!(bar_time);
    }

    bot["state"][&sk]["last_run_date"] = json!(today);
    let raw_orders = pv
        .get("orders")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if raw_orders.is_empty() {
        let bar_d = pv.get("bar_date").and_then(|v| v.as_str()).unwrap_or(today);
        bots::log(
            state,
            bot,
            "decision",
            &format!("[반자동] {bar_d} 기준 주문 신호 없음"),
            "info",
            None,
        )
        .await;
        bots::save(state, bot);
        return Ok(json!({ "staged": 0 }));
    }

    let buy_block = protections::check_buy_allowed(bot, &bot["state"][&sk], today, None);
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let (client, seq) = account_client(state, &uid)?;
    let m_str = bot["market"].as_str().unwrap_or("KR").to_string();
    let market = Market::parse(&m_str)?;
    let currency = if m_str == "KR" { "KRW" } else { "USD" };
    let d_cost = cost_defaults(state, &uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);

    let mut bp_p = HashMap::new();
    bp_p.insert("currency".to_string(), currency.to_string());
    let bp = normalize(
        client
            .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    );
    let avail_cash = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let kind = bots::bot_kind(bot).to_string();
    let invested: f64 = if kind == "rebalance" || kind == "rotation" {
        bot["state"][&sk]["positions"]
            .as_object()
            .map(|mp| {
                mp.values()
                    .map(|p| {
                        p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0)
                            * p.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.0)
                    })
                    .sum()
            })
            .unwrap_or(0.0)
    } else {
        bot["state"][&sk]["position"]["qty"].as_f64().unwrap_or(0.0)
            * bot["state"][&sk]["position"]["entry_price"]
                .as_f64()
                .unwrap_or(0.0)
    };
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let mut rem_budget = avail_cash.min((cap - invested).max(0.0));

    let mut staged: Vec<Value> = Vec::new();
    for o in &raw_orders {
        let side = o
            .get("side")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        if side == "BUY" {
            if let Some(ref blk) = buy_block {
                bots::log(
                    state,
                    bot,
                    "skip",
                    &format!("[반자동] 신규 매수 보류: {blk}"),
                    "warn",
                    None,
                )
                .await;
                continue;
            }
        }
        let sym = o
            .get("symbol")
            .and_then(|v| v.as_str())
            .or_else(|| bot.get("symbol").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim()
            .to_uppercase();

        let mut pp = HashMap::new();
        pp.insert("symbols".to_string(), sym.clone());
        let p_rows = client
            .request("GET", "/api/v1/prices", Some(&pp), None, None)
            .await?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let cur_px = p_rows
            .first()
            .and_then(|r| r.get("lastPrice"))
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .or_else(|| o.get("price").and_then(|v| v.as_f64()))
            .or_else(|| pv.get("last").and_then(|v| v.as_f64()))
            .unwrap_or(0.0);

        let o_kind = o.get("kind").and_then(|v| v.as_str()).unwrap_or("market");
        let level: Option<f64> = if o_kind == "limit" {
            o.get("limit").and_then(|v| v.as_f64())
        } else if let Some(st_v) = o.get("stop").and_then(|v| v.as_f64()) {
            Some(st_v)
        } else if let Some(sfo) = o.get("stop_from_open").and_then(|v| v.as_f64()) {
            let base = pv.get("today_open").and_then(|v| v.as_f64()).unwrap_or(cur_px);
            Some(base + sfo)
        } else {
            None
        };

        let qty: i64 = if side == "BUY" {
            let ref_px = level.unwrap_or(cur_px);
            if ref_px <= 0.0 {
                continue;
            }
            let max_q = (rem_budget / (ref_px * (1.0 + headroom))).floor() as i64;
            let req_q = o.get("qty").and_then(|v| v.as_i64()).unwrap_or(max_q);
            let q = req_q.min(max_q);
            if q <= 0 {
                continue;
            }
            rem_budget = (rem_budget - (q as f64) * ref_px * (1.0 + headroom)).max(0.0);
            q
        } else {
            let mut sq = HashMap::new();
            sq.insert("symbol".to_string(), sym.clone());
            let sellable = client
                .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
                .await?
                .and_then(|v| v.get("sellableQuantity").cloned())
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0.0) as i64;
            let held_q = if kind == "rebalance" || kind == "rotation" {
                bot["state"][&sk]["positions"]
                    .get(&sym)
                    .and_then(|p| p.get("qty"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as i64
            } else {
                bot["state"][&sk]["position"]["qty"].as_f64().unwrap_or(0.0) as i64
            };
            let req_q = o.get("qty").and_then(|v| v.as_i64()).unwrap_or(held_q);
            let q = req_q.min(held_q).min(sellable);
            if q <= 0 {
                continue;
            }
            q
        };

        let default_text = format!(
            "{sym} {} {qty}주",
            if side == "BUY" { "매수" } else { "매도" }
        );
        staged.push(json!({
            "symbol": sym,
            "side": side,
            "kind": o_kind,
            "qty": qty,
            "signal_price": (cur_px * 10000.0).round() / 10000.0,
            "level": level.map(|l| (l * 10000.0).round() / 10000.0),
            "reason": o.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
            "text": o.get("text").and_then(|v| v.as_str()).unwrap_or(&default_text),
        }));
    }

    if staged.is_empty() {
        bots::log(
            state,
            bot,
            "skip",
            "[반자동] 실행 가능한 주문 수량이 없어 승인 대기를 생략했습니다",
            "info",
            None,
        )
        .await;
        bots::save(state, bot);
        return Ok(json!({ "staged": 0 }));
    }

    let app_id = Uuid::new_v4().simple().to_string()[..12].to_string();
    let created_at = now.unwrap_or_else(Utc::now).to_rfc3339();
    let proposal = json!({
        "id": app_id,
        "date": today,
        "created_at": created_at,
        "expires_at": session_end.to_rfc3339(),
        "reason": pv.get("reason").and_then(|v| v.as_str()).unwrap_or(""),
        "orders": staged,
    });
    let mut summary = staged
        .iter()
        .take(3)
        .map(|x| {
            format!(
                "{} {} {}주",
                x["symbol"].as_str().unwrap_or(""),
                if x["side"].as_str() == Some("BUY") { "매수" } else { "매도" },
                x["qty"].as_i64().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(" / ");
    if staged.len() > 3 {
        summary.push_str(&format!(" 외 {}건", staged.len() - 3));
    }
    bot["state"][&sk]["pending_approval"] = proposal;
    bots::log(
        state,
        bot,
        "approval",
        &format!("✋ [승인 대기] {summary} — 화면에서 [1클릭 승인] 시 실제 주문 전송"),
        "info",
        None,
    )
    .await;
    bots::save(state, bot);
    Ok(json!({ "staged": staged.len(), "approval_id": app_id }))
}

pub async fn approve_proposal(
    state: &AppState,
    bot: &mut Value,
    now: Option<DateTime<Utc>>,
) -> ApiResult<Value> {
    let sk = bots::state_key(bot["mode"].as_str().unwrap_or("semi")).to_string();
    let prop = bot["state"][&sk]
        .get("pending_approval")
        .filter(|v| !v.is_null())
        .cloned()
        .ok_or_else(|| ApiError::not_found("no_pending_approval", "승인 대기 중인 주문이 없습니다"))?;
    if is_expired(Some(&prop), now) {
        if let Some(obj) = bot["state"][&sk].as_object_mut() {
            obj.remove("pending_approval");
        }
        bots::save(state, bot);
        return Err(ApiError::new(
            410,
            "approval_expired",
            "승인 가능 시간(정규장 마감)이 지나 자동 만료되었습니다",
        ));
    }

    // Clear proposal BEFORE sending orders so a network failure cannot double-approve
    if let Some(obj) = bot["state"][&sk].as_object_mut() {
        obj.remove("pending_approval");
    }
    bots::save(state, bot);

    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let today = prop["date"].as_str().unwrap_or("").to_string();
    let kind = bots::bot_kind(bot).to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let prop_id = prop["id"].as_str().unwrap_or("").to_string();
    let orders = prop["orders"].as_array().cloned().unwrap_or_default();

    let mut sent = 0usize;
    for (idx, item) in orders.iter().enumerate() {
        let sym = item["symbol"].as_str().unwrap_or("").to_string();
        let side = item["side"].as_str().unwrap_or("BUY").to_string();
        let qty = item["qty"].as_i64().unwrap_or(0);
        let sig_px = item.get("signal_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let o_kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("market");
        let level = item.get("level").and_then(|v| v.as_f64());
        let order_cid = cid(&bot_id, &prop_id, idx);

        let res: ApiResult<()> = async {
            if o_kind == "limit" && level.is_some() {
                let px = round_price(&m, level.unwrap(), if side == "BUY" { "down" } else { "up" });
                let payload = json!({
                    "clientOrderId": order_cid,
                    "symbol": sym,
                    "side": side,
                    "orderType": "LIMIT",
                    "timeInForce": "DAY",
                    "quantity": qty.to_string(),
                    "price": fmt_price(&m, px),
                });
                let built = json!({
                    "market": m,
                    "payload": payload,
                    "checks": { "estimated_value": (qty as f64) * px },
                });
                let r = trading::submit(state, &built, &uid, &format!("bot:{bot_id}")).await?;
                bot["state"][&sk]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "order",
                    "id": r["orderId"],
                    "symbol": sym,
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": px,
                }));
            } else if o_kind == "stop" && level.is_some() {
                let lvl = level.unwrap();
                let r = executor::send_conditional(state, bot, &side, qty, lvl, &today, &order_cid).await?;
                bot["state"][&sk]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "conditional",
                    "id": r["conditionalOrderId"],
                    "symbol": sym,
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": lvl,
                }));
            } else {
                let payload = json!({
                    "clientOrderId": order_cid,
                    "symbol": sym,
                    "side": side,
                    "orderType": "MARKET",
                    "timeInForce": "DAY",
                    "quantity": qty.to_string(),
                });
                let built = json!({
                    "market": m,
                    "payload": payload,
                    "checks": { "estimated_value": (qty as f64) * sig_px.max(1.0) },
                });
                let r = trading::submit(state, &built, &uid, &format!("bot:{bot_id}")).await?;
                bot["state"][&sk]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "order",
                    "id": r["orderId"],
                    "symbol": sym,
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": sig_px,
                }));
            }
            Ok(())
        }
        .await;

        match res {
            Ok(()) => {
                sent += 1;
                bots::log(
                    state,
                    bot,
                    "order",
                    &format!(
                        "[반자동 승인 전송] {sym} {} {qty}주",
                        if side == "BUY" { "매수" } else { "매도" }
                    ),
                    "info",
                    None,
                )
                .await;
            }
            Err(e) => {
                bots::log(
                    state,
                    bot,
                    "error",
                    &format!("[반자동 승인 실패] {sym}: {}", e.message),
                    "error",
                    Some(json!({ "code": e.code })),
                )
                .await;
            }
        }
    }

    if sent > 0 {
        if kind == "rebalance" || kind == "rotation" {
            bot["state"][&sk]["last_rebalance_date"] = json!(today);
            let tw = bot["state"][&sk]
                .get("target_weights")
                .cloned()
                .unwrap_or_else(|| json!({}));
            if let Some(rebs) = bot["state"][&sk]
                .get_mut("rebalances")
                .and_then(|v| v.as_array_mut())
            {
                rebs.push(json!({
                    "date": today,
                    "reason": prop.get("reason").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("반자동 승인 리밸런싱"),
                    "trades": sent,
                    "weights": tw,
                }));
            }
        } else if kind == "dca" {
            bot["state"][&sk]["last_dca_date"] = json!(today);
        }
    }
    bots::save(state, bot);
    Ok(json!({ "sent": sent }))
}

pub async fn skip_proposal(state: &AppState, bot: &mut Value) -> ApiResult<()> {
    let sk = bots::state_key(bot["mode"].as_str().unwrap_or("semi")).to_string();
    let removed = bot["state"][&sk]
        .as_object_mut()
        .and_then(|m| m.remove("pending_approval"))
        .is_some_and(|v| !v.is_null());
    if !removed {
        return Err(ApiError::not_found(
            "no_pending_approval",
            "건너뛸 승인 대기 주문이 없습니다",
        ));
    }
    bots::log(
        state,
        bot,
        "skip",
        "[반자동] 사용자가 오늘 승인 대기 주문을 건너뛰었습니다",
        "info",
        None,
    )
    .await;
    bots::save(state, bot);
    Ok(())
}
