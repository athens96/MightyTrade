//! Smart Dollar-Cost Averaging ('스마트 적립식 모으기') bot executor (MT-7).

use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, executor, protections};
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras::normalize, models::{Candle, Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{Bars, Costs};
use crate::quant::{indicators as ind, market_timing};
use crate::state::AppState;
use crate::trading::orders as trading;

const LIVE_BUY_HEADROOM: f64 = 0.005;

pub fn is_valid_cadence(c: &str) -> bool {
    matches!(c, "daily" | "weekly" | "biweekly" | "monthly")
}

fn cadence_label(c: &str) -> &str {
    match c {
        "daily" => "매일",
        "weekly" => "매주",
        "biweekly" => "격주 (2주)",
        "monthly" => "매월 초",
        other => other,
    }
}

fn client_order_id(bot_id: &str, today: &str) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:{today}:dca:BUY"))
}

fn day_of(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

pub fn should_dca(
    today: &str,
    last_dca_date: Option<&str>,
    cadence: &str,
    weekday: i64,
) -> Option<String> {
    let dt = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    let wd = dt.weekday().num_days_from_monday() as i64;
    let Some(last_s) = last_dca_date else {
        if (cadence == "weekly" || cadence == "biweekly") && wd != weekday {
            return None;
        }
        return Some(format!("최초 적립식 매수 ({})", cadence_label(cadence)));
    };
    let dl = NaiveDate::parse_from_str(last_s, "%Y-%m-%d").ok()?;
    if dt <= dl {
        return None;
    }
    match cadence {
        "daily" => Some("일간 정기 적립".to_string()),
        "weekly" => {
            let iso_t = dt.iso_week();
            let iso_l = dl.iso_week();
            if (iso_t.year(), iso_t.week()) != (iso_l.year(), iso_l.week()) {
                if wd == weekday {
                    return Some("주간 정기 적립".to_string());
                }
                if wd > weekday {
                    return Some("주간 정기 적립 (휴장일 순연)".to_string());
                }
            }
            None
        }
        "biweekly" => {
            if (dt - dl).num_days() >= 13 && wd >= weekday {
                Some("격주 정기 적립".to_string())
            } else {
                None
            }
        }
        "monthly" => {
            if (dt.year(), dt.month()) != (dl.year(), dl.month()) {
                Some("월간 정기 적립".to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn compute_dca_multiplier(
    bars: &Bars,
    smart_scale: bool,
    dip_mult: f64,
    peak_mult: f64,
) -> (f64, String) {
    if !smart_scale || bars.len() < 20 {
        return (1.0, "정액 적립 (1.0x)".to_string());
    }
    let closes = &bars.close;
    let last = *closes.last().unwrap_or(&0.0);
    let r14 = ind::rsi(closes, 14).last().copied().flatten().unwrap_or(50.0);
    let ma20 = ind::sma(closes, 20).last().copied().flatten().unwrap_or(last);

    if r14 <= 32.0 || (ma20 > 0.0 && last <= ma20 * 0.94) {
        return (
            dip_mult,
            format!(
                "세일 구간 확대 적립 ({}x · RSI {r14:.0})",
                protections::fmt_g(dip_mult)
            ),
        );
    }
    if r14 <= 42.0 || (ma20 > 0.0 && last <= ma20 * 0.97) {
        let mid = ((1.0 + (dip_mult - 1.0) * 0.5) * 100.0).round() / 100.0;
        return (
            mid,
            format!(
                "눌림목 가산 적립 ({}x · RSI {r14:.0})",
                protections::fmt_g(mid)
            ),
        );
    }
    if r14 >= 70.0 || (ma20 > 0.0 && last >= ma20 * 1.07) {
        return (
            peak_mult,
            format!(
                "과열 구간 축소 적립 ({}x · RSI {r14:.0})",
                protections::fmt_g(peak_mult)
            ),
        );
    }
    (1.0, format!("기본 정액 적립 (1.0x · RSI {r14:.0})"))
}

struct DcaDecision {
    date: String,
    bar_date: String,
    due: bool,
    reason: String,
    multiplier: f64,
    multiplier_note: String,
    position: Value,
    cash: f64,
    today_bar: Option<Candle>,
    last: f64,
    market_regime: Option<Value>,
    orders: Vec<Value>,
}

async fn decide_internal(state: &AppState, bot: &Value, today: &str) -> ApiResult<DcaDecision> {
    let st = bots::state(bot);
    let uid = bot["user_id"].as_str().unwrap_or("");
    let m_str = bot["market"].as_str().unwrap_or("KR");
    let market = Market::parse(m_str)?;
    let symbol = bot["symbol"].as_str().unwrap_or("");
    let start = Utc::now().timestamp() - 150 * 86400;
    let candles = market::get_history(state, uid, market, symbol, Interval::D1, start).await?;
    let past: Vec<Candle> = candles
        .iter()
        .filter(|c| day_of(c.time).as_str() < today)
        .cloned()
        .collect();
    let today_bar = candles
        .iter()
        .find(|c| day_of(c.time).as_str() == today)
        .cloned();
    if past.len() < 5 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "적립식 판단에 필요한 과거 시세가 부족합니다",
        ));
    }

    let bars = Bars::from_candles(&past);
    let cadence = bot.get("rebalance").and_then(|v| v.as_str()).unwrap_or("weekly");
    let weekday = bot.get("weekday").and_then(|v| v.as_i64()).unwrap_or(0);
    let due_reason = should_dca(
        today,
        st.get("last_dca_date").and_then(|v| v.as_str()),
        cadence,
        weekday,
    );

    let (mut mult, mult_note) = compute_dca_multiplier(
        &bars,
        bot.get("smart_scale").and_then(|v| v.as_bool()).unwrap_or(true),
        bot.get("dip_mult").and_then(|v| v.as_f64()).unwrap_or(1.5),
        bot.get("peak_mult").and_then(|v| v.as_f64()).unwrap_or(0.5),
    );

    let mut mt_snap = None;
    if let Some(mt) = bot.get("market_timing").and_then(|v| v.as_object()) {
        let mode = mt.get("mode").and_then(|v| v.as_str()).unwrap_or("off");
        if mode != "off" {
            let snap = market_timing::current_regime_snapshot(
                state,
                uid,
                m_str,
                mt.get("benchmark").and_then(|v| v.as_str()),
                mt.get("ma_short").and_then(|v| v.as_u64()).unwrap_or(20) as usize,
                mt.get("ma_long").and_then(|v| v.as_u64()).unwrap_or(60) as usize,
                Some(today),
            )
            .await;
            let reg = snap["regime"].as_str().unwrap_or("neutral");
            let sc = market_timing::buy_scale_for(mode, reg);
            mult = (mult * sc * 1000.0).round() / 1000.0;
            mt_snap = Some(snap);
        }
    }

    let pos = st["position"].clone();
    let px_ref = today_bar
        .as_ref()
        .map(|b| b.open)
        .unwrap_or(*bars.close.last().unwrap_or(&0.0));
    let last_px = today_bar
        .as_ref()
        .map(|b| b.close)
        .unwrap_or(*bars.close.last().unwrap_or(&0.0));
    let d_cost = cost_defaults(state, uid, market).await;
    let c = Costs {
        commission_pct: d_cost["commission_pct"].as_f64().unwrap_or(0.015),
        tax_pct: d_cost["tax_pct"].as_f64().unwrap_or(0.0),
        slippage_pct: d_cost["slippage_pct"].as_f64().unwrap_or(0.05),
    };
    let headroom = ((c.slippage_pct + c.commission_pct) / 100.0).max(LIVE_BUY_HEADROOM);

    let pos_qty = pos["qty"].as_f64().unwrap_or(0.0);
    let pos_ep = pos["entry_price"].as_f64().unwrap_or(0.0);
    let invested_so_far = st
        .get("dca_invested")
        .and_then(|v| v.as_f64())
        .filter(|&x| x > 0.0)
        .unwrap_or(pos_qty * pos_ep);
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let remaining_cap = (cap - invested_so_far).max(0.0);
    let cash = if bot["mode"].as_str() == Some("paper") {
        st["cash"].as_f64().unwrap_or(0.0)
    } else {
        remaining_cap
    };
    let base_amt = bot["base_amount"].as_f64().unwrap_or(0.0);
    let target_spend = (base_amt * mult).min(remaining_cap).min(cash);
    let qty = if px_ref > 0.0 && target_spend > 0.0 {
        (target_spend / (px_ref * (1.0 + headroom))).floor() as i64
    } else {
        0
    };

    let mut orders = Vec::new();
    if let Some(ref dr) = due_reason {
        if qty > 0 {
            orders.push(json!({
                "side": "buy",
                "kind": "market",
                "symbol": symbol,
                "qty": qty,
                "price": px_ref,
                "reason": format!("{dr} · {mult_note}"),
                "text": format!("{symbol} 적립 매수 {qty}주 — {mult_note}"),
            }));
        }
    }

    let reason_str = match &due_reason {
        Some(dr) => format!("{dr} · {mult_note}"),
        None => format!("적립 대기 ({mult_note})"),
    };

    Ok(DcaDecision {
        date: today.to_string(),
        bar_date: day_of(*bars.time.last().unwrap_or(&0)),
        due: due_reason.is_some(),
        reason: reason_str,
        multiplier: mult,
        multiplier_note: mult_note,
        position: pos,
        cash,
        today_bar,
        last: last_px,
        market_regime: mt_snap,
        orders,
    })
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    let d = decide_internal(state, bot, today).await?;
    Ok(json!({
        "date": d.date,
        "bar_date": d.bar_date,
        "due": d.due,
        "reason": d.reason,
        "multiplier": d.multiplier,
        "multiplier_note": d.multiplier_note,
        "position": d.position,
        "cash": d.cash,
        "today_open": d.today_bar.as_ref().map(|b| b.open),
        "last": d.last,
        "market_regime": d.market_regime,
        "orders": d.orders,
    }))
}

fn update_dca_benchmark(st: &mut Value, base_amount: f64, ref_price: f64) {
    if ref_price <= 0.0 || base_amount <= 0.0 {
        return;
    }
    let steps = st.get("dca_steps").and_then(|v| v.as_i64()).unwrap_or(0) + 1;
    let reg_inv = st.get("reg_dca_invested").and_then(|v| v.as_f64()).unwrap_or(0.0) + base_amount;
    let reg_sh = st.get("reg_dca_shares").and_then(|v| v.as_f64()).unwrap_or(0.0) + (base_amount / ref_price);
    st["dca_steps"] = json!(steps);
    st["reg_dca_invested"] = json!(reg_inv);
    st["reg_dca_shares"] = json!(reg_sh);
}

pub fn public_dca_stats(st: &Value) -> Value {
    let pos = &st["position"];
    let qty = pos["qty"].as_f64().unwrap_or(0.0);
    let smart_avg = if qty > 0.0 {
        pos["entry_price"].as_f64()
    } else {
        None
    };
    let reg_shares = st.get("reg_dca_shares").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let reg_inv = st.get("reg_dca_invested").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let reg_avg = if reg_shares > 0.0 {
        Some(reg_inv / reg_shares)
    } else {
        None
    };
    let saved_pct = match (smart_avg, reg_avg) {
        (Some(sa), Some(ra)) if ra > 0.0 => Some(((1.0 - sa / ra) * 100.0 * 100.0).round() / 100.0),
        _ => None,
    };
    json!({
        "steps": st.get("dca_steps").and_then(|v| v.as_i64()).unwrap_or(0),
        "total_invested": (st.get("dca_invested").and_then(|v| v.as_f64()).unwrap_or(0.0) * 100.0).round() / 100.0,
        "last_dca_date": st.get("last_dca_date").cloned().unwrap_or(Value::Null),
        "smart_avg_price": smart_avg.map(|v| (v * 100.0).round() / 100.0),
        "regular_avg_price": reg_avg.map(|v| (v * 100.0).round() / 100.0),
        "saved_pct": saved_pct,
    })
}

pub async fn run_paper(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["paper"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let d = decide_internal(state, bot, today).await?;
    let tb = d.today_bar.ok_or_else(|| {
        ApiError::conflict(
            "market_not_open",
            "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다",
        )
    })?;
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let d_cost = cost_defaults(state, &uid, market).await;
    let c = Costs {
        commission_pct: d_cost["commission_pct"].as_f64().unwrap_or(0.015),
        tax_pct: d_cost["tax_pct"].as_f64().unwrap_or(0.0),
        slippage_pct: d_cost["slippage_pct"].as_f64().unwrap_or(0.05),
    };
    let est_eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
        + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * tb.open;
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["paper"], today, Some(est_eq));

    if d.due {
        let base_amt = bot["base_amount"].as_f64().unwrap_or(0.0);
        update_dca_benchmark(&mut bot["state"]["paper"], base_amt, tb.open);
        if let Some(blk) = buy_block {
            bots::log(
                state,
                bot,
                "skip",
                &format!("[모의/적립] 신규 매수 보류: {blk}"),
                "warn",
                None,
            )
            .await;
        } else if let Some(o) = d.orders.first() {
            let px = tb.open * (1.0 + c.slippage_pct / 100.0);
            let unit = px * (1.0 + c.commission_pct / 100.0);
            let cash = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
            let max_q = if unit > 0.0 { (cash / unit).floor() as i64 } else { 0 };
            let qty = o["qty"].as_i64().unwrap_or(0).min(max_q);
            if qty > 0 {
                let cost = (qty as f64) * unit;
                bot["state"]["paper"]["cash"] = json!(cash - cost);
                let old_q = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0);
                let old_ep = bot["state"]["paper"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
                let old_ed = bot["state"]["paper"]["position"]["entry_date"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| today.to_string());
                let new_q = old_q + (qty as f64);
                let avg = (old_q * old_ep + (qty as f64) * px) / new_q;
                bot["state"]["paper"]["position"] = json!({
                    "qty": new_q as i64,
                    "entry_price": avg,
                    "entry_date": old_ed,
                });
                let prev_inv = bot["state"]["paper"]
                    .get("dca_invested")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                bot["state"]["paper"]["dca_invested"] = json!(prev_inv + cost);
                bot["state"]["paper"]["last_dca_date"] = json!(today);
                bot["state"]["paper"]["trades"].as_array_mut().unwrap().push(json!({
                    "entry_date": today,
                    "entry_price": px,
                    "qty": qty,
                    "entry_reason": o["reason"],
                    "cost_basis": cost,
                    "exit_date": Value::Null,
                    "exit_price": Value::Null,
                    "exit_reason": "",
                    "pnl": Value::Null,
                    "return_pct": Value::Null,
                }));
                bots::log(
                    state,
                    bot,
                    "fill",
                    &format!(
                        "[모의/적립] {qty}주 매수 @ {px:.2} (평균단가 {avg:.2}) — {}",
                        d.multiplier_note
                    ),
                    "info",
                    None,
                )
                .await;
            } else {
                bot["state"]["paper"]["last_dca_date"] = json!(today);
                bots::log(
                    state,
                    bot,
                    "skip",
                    &format!(
                        "[모의/적립] 1주 미만 금액이거나 잔고 부족으로 이번 회차 생략 ({})",
                        d.multiplier_note
                    ),
                    "info",
                    None,
                )
                .await;
            }
        } else {
            bot["state"]["paper"]["last_dca_date"] = json!(today);
            bots::log(
                state,
                bot,
                "decision",
                &format!(
                    "[모의/적립] 배수 0x 또는 한도 도달로 이번 회차 매수 생략 ({})",
                    d.multiplier_note
                ),
                "info",
                None,
            )
            .await;
        }
    }

    bot["state"]["paper"]["last_run_date"] = json!(today);
    bot["state"]["paper"]["last_price"] = json!(tb.close);
    let eq_val = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
        + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * tb.close;
    bot["state"]["paper"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": eq_val }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today, "dca_due": d.due }))
}

pub async fn run_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["live"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let _ = executor::reconcile_live(state, bot, today).await;
    let d = decide_internal(state, bot, today).await?;
    if d.today_bar.is_none() {
        return Err(ApiError::conflict(
            "market_not_open",
            "오늘 시가가 아직 없습니다",
        ));
    }

    bot["state"]["live"]["last_run_date"] = json!(today);
    bots::save(state, bot);

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m_str = bot["market"].as_str().unwrap_or("KR").to_string();
    let symbol = bot["symbol"].as_str().unwrap_or("").to_string();
    let market = Market::parse(&m_str)?;
    let currency = if m_str == "KR" { "KRW" } else { "USD" };
    let (client, seq) = account_client(state, &uid)?;

    let mut pp = HashMap::new();
    pp.insert("symbols".to_string(), symbol.clone());
    let current = client
        .request("GET", "/api/v1/prices", Some(&pp), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .and_then(|a| a.first().cloned())
        .and_then(|r| r.get("lastPrice").cloned())
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(d.last);

    let d_cost = cost_defaults(state, &uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["live"], today, None);

    if d.due {
        let base_amt = bot["base_amount"].as_f64().unwrap_or(0.0);
        update_dca_benchmark(&mut bot["state"]["live"], base_amt, current);
        if let Some(blk) = buy_block {
            bots::log(
                state,
                bot,
                "skip",
                &format!("[적립] 신규 매수 보류: {blk}"),
                "warn",
                None,
            )
            .await;
        } else if d.multiplier > 0.0 {
            let mut bp_p = HashMap::new();
            bp_p.insert("currency".to_string(), currency.to_string());
            let bp = normalize(
                client
                    .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                    .await?
                    .unwrap_or_else(|| json!({})),
                "",
            );
            let cash_bp = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pos_q = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
            let pos_ep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
            let cap = bot["capital"].as_f64().unwrap_or(0.0);
            let rem_cap = (cap - pos_q * pos_ep).max(0.0);
            let spend = (base_amt * d.multiplier).min(rem_cap).min(cash_bp);
            let qty = if current > 0.0 && spend > 0.0 {
                (spend / (current * (1.0 + headroom))).floor() as i64
            } else {
                0
            };
            if qty > 0 {
                let cid = client_order_id(&bot_id, today);
                let payload = json!({
                    "clientOrderId": cid,
                    "symbol": symbol,
                    "side": "BUY",
                    "orderType": "MARKET",
                    "timeInForce": "DAY",
                    "quantity": qty.to_string(),
                });
                let built = json!({
                    "market": m_str,
                    "payload": payload,
                    "checks": { "estimated_value": (qty as f64) * current },
                });
                match trading::submit(state, &built, &uid, &format!("bot:{bot_id}")).await {
                    Ok(r) => {
                        bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                            "kind": "order",
                            "id": r["orderId"],
                            "side": "BUY",
                            "qty": qty,
                            "date": today,
                            "synced": 0.0,
                            "signal_price": current,
                        }));
                        let prev_inv = bot["state"]["live"]
                            .get("dca_invested")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        bot["state"]["live"]["dca_invested"] =
                            json!(prev_inv + (qty as f64) * current);
                        bot["state"]["live"]["last_dca_date"] = json!(today);
                        bots::log(
                            state,
                            bot,
                            "order",
                            &format!("[적립] 시장가 매수 {qty}주 — {}", d.multiplier_note),
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
                            &format!("[적립] 주문 실패: {}", e.message),
                            "error",
                            Some(json!({ "code": e.code })),
                        )
                        .await;
                    }
                }
            } else {
                bot["state"]["live"]["last_dca_date"] = json!(today);
                bots::log(
                    state,
                    bot,
                    "skip",
                    "[적립] 매수 가능 현금 또는 잔여 한도 부족으로 생략",
                    "warn",
                    None,
                )
                .await;
            }
        }
    }

    bot["state"]["live"]["last_price"] = json!(current);
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pos_q = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let pos_ep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    bot["state"]["live"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": cap + realized + pos_q * (current - pos_ep) }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today, "dca_due": d.due }))
}

pub async fn run(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["mode"].as_str() == Some("paper") {
        run_paper(state, bot, today).await
    } else {
        run_live(state, bot, today).await
    }
}
