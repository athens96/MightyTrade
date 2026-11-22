//! Screener-linked automatic stock rotation bot ('자동 순환매 랭킹 봇') & historical backtester (MT-6).

use chrono::{Datelike, NaiveDate, Utc};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, protections, rebalance as rebal};
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras::normalize, models::{Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{Bars, Costs};
use crate::quant::metrics::{drawdown_series, performance};
use crate::quant::{market_timing, portfolio, screener, screener_data as sdata};
use crate::state::AppState;

const LIVE_BUY_HEADROOM: f64 = 0.005;

pub fn is_valid_cadence(c: &str) -> bool {
    matches!(c, "weekly" | "biweekly" | "monthly")
}

fn client_order_id(bot_id: &str, today: &str, symbol: &str, side: &str) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:{today}:rot:{symbol}:{side}"))
}

pub fn should_rotate(today: &str, last_date: Option<&str>, cadence: &str) -> Option<String> {
    let Some(last_s) = last_date else {
        return Some("최초 랭킹 순환매 포트폴리오 구성".to_string());
    };
    let dt = NaiveDate::parse_from_str(today, "%Y-%m-%d").ok()?;
    let dl = NaiveDate::parse_from_str(last_s, "%Y-%m-%d").ok()?;
    if dt <= dl {
        return None;
    }
    match cadence {
        "weekly" => {
            let it = dt.iso_week();
            let il = dl.iso_week();
            if (it.year(), it.week()) != (il.year(), il.week()) {
                return Some("주간 정기 랭킹 순환매".to_string());
            }
        }
        "biweekly" => {
            if (dt - dl).num_days() >= 14 {
                return Some("격주 정기 랭킹 순환매".to_string());
            }
        }
        _ => {
            if (dt.year(), dt.month()) != (dl.year(), dl.month()) {
                return Some("월간 정기 랭킹 순환매".to_string());
            }
        }
    }
    None
}

pub fn select_target_picks(
    ranked_items: &[Value],
    held_symbols: &[String],
    top_n: usize,
    hold_buffer: usize,
) -> (Vec<Value>, Vec<String>) {
    let by_sym: HashMap<&str, &Value> = ranked_items
        .iter()
        .filter_map(|r| r.get("symbol").and_then(|v| v.as_str()).map(|s| (s, r)))
        .collect();
    let mut kept: Vec<Value> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for sym in held_symbols {
        if let Some(&row) = by_sym.get(sym.as_str()) {
            let rk = row.get("rank").and_then(|v| v.as_u64()).unwrap_or(9999) as usize;
            if rk <= hold_buffer {
                kept.push(row.clone());
                continue;
            }
        }
        dropped.push(sym.clone());
    }
    kept.sort_by_key(|r| r.get("rank").and_then(|v| v.as_u64()).unwrap_or(9999));
    kept.truncate(top_n);
    let kept_syms: HashSet<String> = kept
        .iter()
        .filter_map(|r| r.get("symbol").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .collect();
    let mut fresh = Vec::new();
    for r in ranked_items {
        if kept.len() + fresh.len() >= top_n {
            break;
        }
        let sym = r.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        if !kept_syms.contains(sym) {
            fresh.push(r.clone());
        }
    }
    kept.extend(fresh);
    (kept, dropped)
}

async fn fetch_open_and_close(
    state: &AppState,
    uid: &str,
    market: Market,
    symbols: &[String],
    today: &str,
    fallback_prices: &HashMap<String, f64>,
) -> (HashMap<String, f64>, HashMap<String, f64>, bool) {
    let start = Utc::now().timestamp() - 12 * 86400;
    let mut opens = HashMap::new();
    let mut closes = HashMap::new();
    let mut has_all_today = true;
    for s in symbols {
        match market::get_history(state, uid, market, s, Interval::D1, start).await {
            Ok(cs) => {
                if let Some(tb) = cs.iter().find(|c| portfolio::day_str(c.time).as_str() == today) {
                    opens.insert(s.clone(), tb.open);
                    closes.insert(s.clone(), tb.close);
                } else if let Some(last) = cs.last() {
                    has_all_today = false;
                    opens.insert(s.clone(), last.close);
                    closes.insert(s.clone(), last.close);
                } else {
                    has_all_today = false;
                    let px = fallback_prices.get(s).copied().unwrap_or(0.0);
                    opens.insert(s.clone(), px);
                    closes.insert(s.clone(), px);
                }
            }
            Err(_) => {
                has_all_today = false;
                let px = fallback_prices.get(s).copied().unwrap_or(0.0);
                opens.insert(s.clone(), px);
                closes.insert(s.clone(), px);
            }
        }
    }
    (opens, closes, has_all_today)
}

struct RotationDecision {
    date: String,
    due: bool,
    reason: Option<String>,
    total_value: f64,
    last_prices: HashMap<String, f64>,
    open_prices: HashMap<String, f64>,
    current_weights: Map<String, Value>,
    target_weights: Map<String, Value>,
    target_weights_raw: HashMap<String, f64>,
    selected: Vec<Value>,
    orders: Vec<Value>,
    has_today_bars: bool,
    market_regime: Option<Value>,
}

async fn decide_internal(state: &AppState, bot: &Value, today: &str) -> ApiResult<RotationDecision> {
    let st = bots::state(bot);
    let uid = bot["user_id"].as_str().unwrap_or("");
    let m_str = bot["market"].as_str().unwrap_or("KR");
    let market = Market::parse(m_str)?;
    let positions = st.get("positions").and_then(|v| v.as_object());
    let held_syms: Vec<String> = positions
        .map(|mp| {
            mp.iter()
                .filter(|(_, p)| p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0)
                .map(|(s, _)| s.clone())
                .collect()
        })
        .unwrap_or_default();

    let top_n = bot.get("top_n").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
    let hold_buffer = (bot
        .get("hold_buffer")
        .and_then(|v| v.as_u64())
        .unwrap_or((top_n * 2).max(10) as u64) as usize)
        .max(top_n);
    let cadence = bot.get("rebalance").and_then(|v| v.as_str()).unwrap_or("weekly");
    let mut reason = should_rotate(
        today,
        st.get("last_rebalance_date").and_then(|v| v.as_str()),
        cadence,
    );

    let custom_syms: Option<Vec<String>> = bot
        .get("custom_symbols")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect());
    let scan_limit = (top_n + 5).max(hold_buffer + 5).max(25).min(40);
    let screen = screener::run_screen(
        state,
        uid,
        market,
        bot["preset"].as_str().unwrap_or("momentum_quality"),
        bot["universe"].as_str().unwrap_or("kr_all_150"),
        custom_syms.as_deref(),
        bot.get("strategy").and_then(|v| v.as_str()),
        bot.get("exclude_warnings").and_then(|v| v.as_bool()).unwrap_or(true),
        false,
        scan_limit,
    )
    .await?;
    let ranked = screen["items"].as_array().cloned().unwrap_or_default();
    let (selected, _dropped) = select_target_picks(&ranked, &held_syms, top_n, hold_buffer);

    let mut mt_snap = None;
    let mut mt_scale = 1.0;
    let mut mt_mode = "off";
    if let Some(mt) = bot.get("market_timing").and_then(|v| v.as_object()) {
        mt_mode = mt.get("mode").and_then(|v| v.as_str()).unwrap_or("off");
        if mt_mode != "off" {
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
            mt_scale = market_timing::buy_scale_for(mt_mode, reg);
            mt_snap = Some(snap);
        }
    }

    let mut all_syms = Vec::new();
    for s in &held_syms {
        if !all_syms.contains(s) {
            all_syms.push(s.clone());
        }
    }
    for r in &selected {
        if let Some(s) = r.get("symbol").and_then(|v| v.as_str()) {
            let ss = s.to_string();
            if !all_syms.contains(&ss) {
                all_syms.push(ss);
            }
        }
    }

    let mut price_hint = HashMap::new();
    let mut name_map = HashMap::new();
    for r in &ranked {
        if let Some(s) = r.get("symbol").and_then(|v| v.as_str()) {
            if let Some(p) = r.get("price").and_then(|v| v.as_f64()) {
                price_hint.insert(s.to_string(), p);
            }
            if let Some(nm) = r.get("name").and_then(|v| v.as_str()) {
                name_map.insert(s.to_string(), nm.to_string());
            }
        }
    }
    if let Some(pm) = positions {
        for (s, p) in pm {
            price_hint.entry(s.clone()).or_insert_with(|| {
                st.get("last_prices")
                    .and_then(|lp| lp.get(s))
                    .and_then(|v| v.as_f64())
                    .or_else(|| p.get("entry_price").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0)
            });
        }
    }

    let (opens, closes, has_today) =
        fetch_open_and_close(state, uid, market, &all_syms, today, &price_hint).await;
    let pos_qty = |s: &str| -> i64 {
        positions
            .and_then(|m| m.get(s))
            .and_then(|p| p.get("qty"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64
    };
    let pos_val: f64 = held_syms
        .iter()
        .map(|s| (pos_qty(s) as f64) * closes.get(s).copied().unwrap_or(0.0))
        .sum();
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let total_val = if bot["mode"].as_str() == Some("paper") {
        st.get("cash").and_then(|v| v.as_f64()).unwrap_or(cap) + pos_val
    } else {
        cap.max(pos_val)
    };

    let mut tgt_w: HashMap<String, f64> = all_syms.iter().map(|s| (s.clone(), 0.0)).collect();
    if mt_mode == "exit_all"
        && mt_snap
            .as_ref()
            .is_some_and(|s| s["regime"].as_str() == Some("bear"))
    {
        if reason.is_none() {
            let lbl = mt_snap.as_ref().and_then(|s| s["label"].as_str()).unwrap_or("하락장");
            reason = Some(format!("마켓 타이밍 현금 대피 ({lbl})"));
        }
    } else if !selected.is_empty() {
        let slot_w = (1.0 / selected.len() as f64) * mt_scale;
        for r in &selected {
            if let Some(s) = r.get("symbol").and_then(|v| v.as_str()) {
                tgt_w.insert(s.to_string(), slot_w);
            }
        }
    }

    let mut cur_w = HashMap::new();
    for s in &all_syms {
        let w = if total_val > 0.0 {
            (pos_qty(s) as f64) * closes.get(s).copied().unwrap_or(0.0) / total_val
        } else {
            0.0
        };
        cur_w.insert(s.clone(), w);
    }

    let d_cost = cost_defaults(state, uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);

    let mut sells = Vec::new();
    let mut buys = Vec::new();
    for s in &all_syms {
        let cur_q = pos_qty(s);
        let w = tgt_w.get(s).copied().unwrap_or(0.0);
        let px = opens
            .get(s)
            .copied()
            .filter(|&x| x > 0.0)
            .or_else(|| closes.get(s).copied())
            .unwrap_or(0.0);
        let tgt_q = if w > 0.0 && px > 0.0 {
            (total_val * w / (px * (1.0 + headroom))).floor() as i64
        } else {
            0
        };
        let diff = tgt_q - cur_q;
        let nm = name_map.get(s).cloned().unwrap_or_else(|| s.clone());
        let cw = cur_w.get(s).copied().unwrap_or(0.0) * 100.0;
        let tw = w * 100.0;
        if diff < 0 {
            let tag = if w == 0.0 { "순위 이탈 교체 매도" } else { "비중 조절 매도" };
            sells.push(json!({
                "symbol": s,
                "name": nm,
                "side": "sell",
                "kind": "market",
                "qty": -diff,
                "price": px,
                "current_pct": (cw * 10.0).round() / 10.0,
                "target_pct": (tw * 10.0).round() / 10.0,
                "text": format!("{nm}({s}) 매도 {}주 — {tag}", -diff),
            }));
        } else if diff > 0 {
            buys.push(json!({
                "symbol": s,
                "name": nm,
                "side": "buy",
                "kind": "market",
                "qty": diff,
                "price": px,
                "current_pct": (cw * 10.0).round() / 10.0,
                "target_pct": (tw * 10.0).round() / 10.0,
                "text": format!("{nm}({s}) 매수 {diff}주 (목표 {tw:.1}%)"),
            }));
        }
    }
    sells.extend(buys);

    let mut cw_map = Map::new();
    let mut tw_map = Map::new();
    for s in &all_syms {
        cw_map.insert(s.clone(), json!((cur_w[s] * 10000.0).round() / 100.0));
        let w = tgt_w.get(s).copied().unwrap_or(0.0);
        if w > 0.0 {
            tw_map.insert(s.clone(), json!((w * 10000.0).round() / 100.0));
        }
    }
    let selected_out: Vec<Value> = selected
        .iter()
        .map(|r| {
            json!({
                "symbol": r["symbol"],
                "name": r["name"],
                "rank": r["rank"],
                "score": r["score"],
            })
        })
        .collect();

    Ok(RotationDecision {
        date: today.to_string(),
        due: reason.is_some(),
        reason,
        total_value: total_val,
        last_prices: closes,
        open_prices: opens,
        current_weights: cw_map,
        target_weights: tw_map,
        target_weights_raw: tgt_w,
        selected: selected_out,
        orders: sells,
        has_today_bars: has_today || all_syms.is_empty(),
        market_regime: mt_snap,
    })
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    let d = decide_internal(state, bot, today).await?;
    let st = bots::state(bot);
    let tot_qty: i64 = st
        .get("positions")
        .and_then(|v| v.as_object())
        .map(|mp| {
            mp.values()
                .map(|p| p.get("qty").and_then(|q| q.as_f64()).unwrap_or(0.0) as i64)
                .sum()
        })
        .unwrap_or(0);
    Ok(json!({
        "date": d.date,
        "bar_date": d.date,
        "due": d.due,
        "reason": d.reason,
        "position": { "qty": tot_qty, "entry_price": 0 },
        "cash": st.get("cash").cloned().unwrap_or_else(|| bot["capital"].clone()),
        "today_open": Value::Null,
        "last": d.total_value,
        "current_weights": d.current_weights,
        "target_weights": d.target_weights,
        "selected": d.selected,
        "market_regime": d.market_regime,
        "orders": if d.due { d.orders.clone() } else { vec![] },
        "planned_orders": d.orders,
    }))
}

pub async fn run_paper(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["paper"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let d = decide_internal(state, bot, today).await?;
    if !d.has_today_bars {
        return Err(ApiError::conflict(
            "market_not_open",
            "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다",
        ));
    }
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let d_cost = cost_defaults(state, &uid, market).await;
    let comm = d_cost["commission_pct"].as_f64().unwrap_or(0.015) / 100.0;
    let tax = d_cost["tax_pct"].as_f64().unwrap_or(0.0) / 100.0;
    let slip = d_cost["slippage_pct"].as_f64().unwrap_or(0.05) / 100.0;
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["paper"], today, Some(d.total_value));

    if d.due {
        let cash_init = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
        let pos_qty_of = |b: &Value, s: &str| -> i64 {
            b["state"]["paper"]["positions"]
                .get(s)
                .and_then(|p| p.get("qty"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i64
        };
        let val_open = cash_init
            + d.target_weights_raw
                .keys()
                .map(|s| (pos_qty_of(bot, s) as f64) * d.open_prices.get(s).copied().unwrap_or(0.0))
                .sum::<f64>();
        let mut desired = HashMap::new();
        for (s, &w) in &d.target_weights_raw {
            let px_b = d.open_prices.get(s).copied().unwrap_or(0.0) * (1.0 + slip) * (1.0 + comm);
            let q = if w > 0.0 && px_b > 0.0 && buy_block.is_none() {
                (val_open * w / px_b).floor() as i64
            } else {
                0
            };
            desired.insert(s.clone(), q);
        }

        let mut n_orders = 0usize;
        let held_keys: Vec<String> = bot["state"]["paper"]["positions"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for s in held_keys {
            let cur_q = pos_qty_of(bot, &s);
            let diff = desired.get(&s).copied().unwrap_or(0) - cur_q;
            if diff < 0 {
                let qty = -diff;
                let ep = bot["state"]["paper"]["positions"][&s]["entry_price"]
                    .as_f64()
                    .unwrap_or(0.0);
                let px = d.open_prices.get(&s).copied().unwrap_or(ep) * (1.0 - slip);
                let net = (qty as f64) * px * (1.0 - comm - tax);
                let cash_now = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                bot["state"]["paper"]["cash"] = json!(cash_now + net);
                let new_q = cur_q - qty;
                if new_q <= 0 {
                    bot["state"]["paper"]["positions"].as_object_mut().unwrap().remove(&s);
                } else {
                    bot["state"]["paper"]["positions"][&s]["qty"] = json!(new_q);
                }
                n_orders += 1;
                bots::log(
                    state,
                    bot,
                    "fill",
                    &format!("[모의/순환매] {s} 교체 매도 {qty}주 @ {px:.2}"),
                    "info",
                    None,
                )
                .await;
            }
        }

        if let Some(ref blk) = buy_block {
            bots::log(
                state,
                bot,
                "skip",
                &format!("[모의/순환매] 신규 매수 보류: {blk}"),
                "warn",
                None,
            )
            .await;
        } else {
            let mut sorted_des: Vec<(&String, &i64)> = desired.iter().collect();
            sorted_des.sort_by_key(|(s, _)| *s);
            for (s, &tgt_q) in sorted_des {
                let cur_q = pos_qty_of(bot, s);
                let diff = tgt_q - cur_q;
                if diff > 0 {
                    let px = d.open_prices.get(s).copied().unwrap_or(0.0) * (1.0 + slip);
                    let unit = px * (1.0 + comm);
                    let cash_now = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                    let qty = if unit > 0.0 {
                        diff.min((cash_now / unit).floor() as i64)
                    } else {
                        0
                    };
                    if qty > 0 {
                        bot["state"]["paper"]["cash"] = json!(cash_now - (qty as f64) * unit);
                        let old_ep = bot["state"]["paper"]["positions"]
                            .get(s)
                            .and_then(|p| p.get("entry_price"))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(0.0);
                        let new_q = cur_q + qty;
                        let avg = ((cur_q as f64) * old_ep + (qty as f64) * px) / (new_q as f64);
                        let nm = d
                            .selected
                            .iter()
                            .find(|x| x["symbol"].as_str() == Some(s.as_str()))
                            .and_then(|x| x["name"].as_str())
                            .unwrap_or(s.as_str());
                        bot["state"]["paper"]["positions"][s] =
                            json!({ "qty": new_q, "entry_price": avg, "name": nm });
                        n_orders += 1;
                        bots::log(
                            state,
                            bot,
                            "fill",
                            &format!("[모의/순환매] {nm}({s}) 편입 매수 {qty}주 @ {px:.2}"),
                            "info",
                            None,
                        )
                        .await;
                    }
                }
            }
        }

        let reason_txt = d.reason.clone().unwrap_or_default();
        bot["state"]["paper"]["last_rebalance_date"] = json!(today);
        bot["state"]["paper"]["rebalances"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "date": today,
                "reason": reason_txt,
                "trades": n_orders,
                "weights": d.target_weights,
            }));
        bots::log(
            state,
            bot,
            "order",
            &format!("[모의] {reason_txt} 완료 ({n_orders}건)"),
            "info",
            None,
        )
        .await;
    } else {
        bots::log(
            state,
            bot,
            "decision",
            "[모의] 순환매 교체 주기 미도래 (유지)",
            "info",
            None,
        )
        .await;
    }

    bot["state"]["paper"]["last_run_date"] = json!(today);
    if let Some(lp) = bot["state"]["paper"]["last_prices"].as_object_mut() {
        for (k, v) in &d.last_prices {
            lp.insert(k.clone(), json!(v));
        }
    }
    bot["state"]["paper"]["target_weights"] = Value::Object(d.target_weights);
    bot["state"]["paper"]["selected_picks"] = Value::Array(d.selected);

    let cash_end = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
    let pos_eq: f64 = bot["state"]["paper"]["positions"]
        .as_object()
        .map(|mp| {
            mp.iter()
                .map(|(s, p)| {
                    let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let px = bot["state"]["paper"]["last_prices"]
                        .get(s)
                        .and_then(|v| v.as_f64())
                        .or_else(|| p.get("entry_price").and_then(|v| v.as_f64()))
                        .unwrap_or(0.0);
                    q * px
                })
                .sum()
        })
        .unwrap_or(0.0);
    bot["state"]["paper"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": cash_end + pos_eq }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today, "rotated": d.due }))
}

pub async fn reconcile_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<bool> {
    rebal::reconcile_live(state, bot, today).await
}

pub async fn run_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["live"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let _ = reconcile_live(state, bot, today).await;
    let d = decide_internal(state, bot, today).await?;
    if !d.has_today_bars {
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
    let market = Market::parse(&m_str)?;
    let currency = if m_str == "KR" { "KRW" } else { "USD" };
    let (client, seq) = account_client(state, &uid)?;

    let mut all_syms: Vec<String> = bot["state"]["live"]["positions"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for s in d.target_weights_raw.keys() {
        if !all_syms.contains(s) {
            all_syms.push(s.clone());
        }
    }
    let mut live_prices = HashMap::new();
    if !all_syms.is_empty() {
        let mut pp = HashMap::new();
        pp.insert("symbols".to_string(), all_syms.iter().take(30).cloned().collect::<Vec<_>>().join(","));
        let rows = client
            .request("GET", "/api/v1/prices", Some(&pp), None, None)
            .await?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        for r in rows {
            if let (Some(sym), Some(lp)) = (
                r.get("symbol").and_then(|v| v.as_str()),
                r.get("lastPrice").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))),
            ) {
                live_prices.insert(sym.to_string(), lp);
            }
        }
    }
    let mut prices = HashMap::new();
    for s in &all_syms {
        prices.insert(
            s.clone(),
            live_prices
                .get(s)
                .copied()
                .or_else(|| d.last_prices.get(s).copied())
                .unwrap_or(0.0),
        );
    }

    let d_cost = cost_defaults(state, &uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["live"], today, None);
    let cap = bot["capital"].as_f64().unwrap_or(0.0);

    let pos_qty_of = |b: &Value, s: &str| -> i64 {
        b["state"]["live"]["positions"]
            .get(s)
            .and_then(|p| p.get("qty"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64
    };

    if d.due {
        let held_keys: Vec<String> = bot["state"]["live"]["positions"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        let pos_val: f64 = held_keys
            .iter()
            .map(|s| (pos_qty_of(bot, s) as f64) * prices.get(s).copied().unwrap_or(0.0))
            .sum();
        let target_cap = cap.max(pos_val);
        let mut desired = HashMap::new();
        for (s, &w) in &d.target_weights_raw {
            let px = prices.get(s).copied().unwrap_or(0.0);
            let q = if w > 0.0 && px > 0.0 && buy_block.is_none() {
                (target_cap * w / (px * (1.0 + headroom))).floor() as i64
            } else {
                0
            };
            desired.insert(s.clone(), q);
        }

        let mut sent_sells = HashSet::new();
        let mut n_sent = 0usize;
        for s in &held_keys {
            let cur_q = pos_qty_of(bot, s);
            let diff = desired.get(s).copied().unwrap_or(0) - cur_q;
            if diff < 0 {
                let mut sq = HashMap::new();
                sq.insert("symbol".to_string(), s.clone());
                let px = prices.get(s).copied().unwrap_or(0.0);
                let sell_res: ApiResult<Option<Value>> = async {
                    let sellable = client
                        .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
                        .await?
                        .and_then(|v| v.get("sellableQuantity").cloned())
                        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|x| x.parse().ok())))
                        .unwrap_or(0.0) as i64;
                    let qty = (-diff).min(cur_q).min(sellable);
                    if qty <= 0 {
                        return Ok(None);
                    }
                    let cid = client_order_id(&bot_id, today, s, "SELL");
                    let r = rebal::send_market(state, bot, s, "SELL", qty, px, &cid).await?;
                    Ok(Some(json!({ "r": r, "qty": qty })))
                }
                .await;
                match sell_res {
                    Ok(Some(info)) => {
                        let oid = info["r"]["orderId"].as_str().unwrap_or("").to_string();
                        let qty = info["qty"].as_i64().unwrap_or(0);
                        bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                            "kind": "order",
                            "id": oid,
                            "symbol": s,
                            "side": "SELL",
                            "qty": qty,
                            "date": today,
                            "synced": 0.0,
                            "signal_price": px,
                        }));
                        sent_sells.insert(oid);
                        n_sent += 1;
                        bots::log(
                            state,
                            bot,
                            "order",
                            &format!("[순환매] {s} 교체 매도 시장가 {qty}주"),
                            "info",
                            None,
                        )
                        .await;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        bots::log(
                            state,
                            bot,
                            "error",
                            &format!("[순환매] {s} 매도 실패: {}", e.message),
                            "error",
                            Some(json!({ "code": e.code })),
                        )
                        .await;
                    }
                }
            }
        }

        let has_buys = buy_block.is_none() && desired.iter().any(|(s, &q)| q > pos_qty_of(bot, s));
        if !sent_sells.is_empty() && has_buys {
            rebal::wait_sell_fills(state, bot, today, &sent_sells).await;
        }
        if has_buys {
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
            let invested: f64 = bot["state"]["live"]["positions"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(s, p)| {
                            p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                * prices.get(s).copied().unwrap_or(0.0)
                        })
                        .sum()
                })
                .unwrap_or(0.0);
            let mut budget = avail_cash.min((cap - invested).max(0.0));
            let mut sorted_des: Vec<(&String, &i64)> = desired.iter().collect();
            sorted_des.sort_by_key(|(s, _)| *s);
            for (s, &tgt_q) in sorted_des {
                let cur_q = pos_qty_of(bot, s);
                let diff = tgt_q - cur_q;
                if diff <= 0 {
                    continue;
                }
                let px = prices.get(s).copied().unwrap_or(0.0);
                let unit = px * (1.0 + headroom);
                let qty = if unit > 0.0 {
                    diff.min((budget / unit).floor() as i64)
                } else {
                    0
                };
                if qty <= 0 {
                    continue;
                }
                let cid = client_order_id(&bot_id, today, s, "BUY");
                match rebal::send_market(state, bot, s, "BUY", qty, px, &cid).await {
                    Ok(r) => {
                        bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                            "kind": "order",
                            "id": r["orderId"],
                            "symbol": s,
                            "side": "BUY",
                            "qty": qty,
                            "date": today,
                            "synced": 0.0,
                            "signal_price": px,
                        }));
                        budget = (budget - (qty as f64) * unit).max(0.0);
                        n_sent += 1;
                        bots::log(
                            state,
                            bot,
                            "order",
                            &format!("[순환매] {s} 편입 매수 시장가 {qty}주"),
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
                            &format!("[순환매] {s} 매수 실패: {}", e.message),
                            "error",
                            Some(json!({ "code": e.code })),
                        )
                        .await;
                    }
                }
            }
        }
        bot["state"]["live"]["last_rebalance_date"] = json!(today);
        bot["state"]["live"]["rebalances"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "date": today,
                "reason": d.reason.clone().unwrap_or_default(),
                "trades": n_sent,
                "weights": d.target_weights,
            }));
    }

    if let Some(lp) = bot["state"]["live"]["last_prices"].as_object_mut() {
        for (k, v) in &prices {
            lp.insert(k.clone(), json!(v));
        }
    }
    bot["state"]["live"]["target_weights"] = Value::Object(d.target_weights);
    bot["state"]["live"]["selected_picks"] = Value::Array(d.selected);
    let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let unreal: f64 = bot["state"]["live"]["positions"]
        .as_object()
        .map(|mp| {
            mp.iter()
                .map(|(s, p)| {
                    let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let ep = p.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    q * (prices.get(s).copied().unwrap_or(ep) - ep)
                })
                .sum()
        })
        .unwrap_or(0.0);
    bot["state"]["live"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": cap + realized + unreal }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today, "rotated": d.due }))
}

pub async fn run(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["mode"].as_str() == Some("paper") {
        run_paper(state, bot, today).await
    } else {
        run_live(state, bot, today).await
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run_rotation_backtest(
    state: &AppState,
    user_id: &str,
    market: Market,
    preset_id: &str,
    universe_id: &str,
    custom_symbols: Option<&[String]>,
    top_n: usize,
    hold_buffer: usize,
    cadence: &str,
    period: &str,
    capital: f64,
    costs_opt: Option<Costs>,
) -> ApiResult<Value> {
    if !is_valid_cadence(cadence) {
        return Err(ApiError::bad_request(
            "invalid_cadence",
            "순환 주기는 weekly / biweekly / monthly 중 하나여야 합니다",
        ));
    }
    if screener::get_preset(preset_id).is_none() {
        return Err(ApiError::code_only(400, "unknown_preset"));
    }
    let is_mock = market::provider_name(state, market) != "toss";
    let (uni_norm, mut candidates) =
        screener::resolve_candidates(state, market, universe_id, custom_symbols, is_mock, user_id)
            .await?;
    candidates.truncate(40);
    let syms: Vec<String> = candidates.iter().map(|c| c.0.clone()).collect();
    let name_map: HashMap<String, String> = candidates.iter().cloned().collect();

    let days: i64 = match period {
        "1y" => 365,
        "5y" => 365 * 5,
        _ => 365 * 3,
    };
    let start_ts = Utc::now().timestamp() - days * 86400;
    let fetch_from = start_ts - 320 * 86400;

    let mut bars_by_sym: HashMap<String, Bars> = if !is_mock {
        let p = if matches!(period, "1y" | "3y" | "5y") {
            period
        } else {
            "3y"
        };
        sdata::fetch_public_history(market.as_str(), &syms, p).await
    } else {
        HashMap::new()
    };
    for sym in &syms {
        if !bars_by_sym.contains_key(sym) {
            if let Ok(cs) =
                market::get_history(state, user_id, market, sym, Interval::D1, fetch_from).await
            {
                if cs.len() >= 60 {
                    bars_by_sym.insert(sym.clone(), Bars::from_candles(&cs));
                }
            }
        }
    }
    if bars_by_sym.len() < 2 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "순환매 백테스트에 필요한 시세 데이터가 부족합니다",
        ));
    }

    let c = match costs_opt {
        Some(x) => x,
        None => {
            let d = cost_defaults(state, user_id, market).await;
            Costs {
                commission_pct: d["commission_pct"].as_f64().unwrap_or(0.015),
                tax_pct: d["tax_pct"].as_f64().unwrap_or(0.0),
                slippage_pct: d["slippage_pct"].as_f64().unwrap_or(0.05),
            }
        }
    };
    let preset_owned = preset_id.to_string();
    let cadence_owned = cadence.to_string();
    let uni_owned = uni_norm.clone();
    let market_str = market.as_str().to_string();
    let hb = hold_buffer.max(top_n);

    tokio::task::spawn_blocking(move || -> ApiResult<Value> {
        let (times, opens, closes) = portfolio::align_bars(&bars_by_sym);
        let valid_syms: Vec<String> = closes.keys().cloned().collect();
        let warmup = (times.len() / 4).max(30).min(200);
        let first = times.iter().position(|&t| t >= start_ts).unwrap_or(warmup);
        let mut start_idx = first.max(warmup);
        if start_idx >= times.len().saturating_sub(2) {
            start_idx = (times.len() / 3).max(20);
        }
        if start_idx >= times.len().saturating_sub(2) {
            return Err(ApiError::bad_request(
                "not_enough_data",
                "공통 거래일 구간이 너무 짧습니다",
            ));
        }

        let (comm, tax, slip) = (
            c.commission_pct / 100.0,
            c.tax_pct / 100.0,
            c.slippage_pct / 100.0,
        );
        let mut cash = capital;
        let mut holdings: HashMap<String, i64> = HashMap::new();
        let mut last_rot_day: Option<String> = None;
        let mut planned_weights: Option<HashMap<String, f64>> = None;
        let mut planned_reason = String::new();
        let mut rebalances = Vec::new();
        let mut equity = Vec::new();

        for j in start_idx..times.len() {
            let day_str = portfolio::day_str(times[j]);
            if let Some(pw) = planned_weights.take() {
                let val_open = cash
                    + valid_syms
                        .iter()
                        .map(|s| (*holdings.get(s).unwrap_or(&0) as f64) * opens[s][j])
                        .sum::<f64>();
                let mut desired = HashMap::new();
                for (s, &w) in &pw {
                    let px_b = opens[s][j] * (1.0 + slip) * (1.0 + comm);
                    let q = if w > 0.0 && px_b > 0.0 {
                        (val_open * w / px_b).floor() as i64
                    } else {
                        0
                    };
                    desired.insert(s.clone(), q);
                }
                let mut traded_val = 0.0;
                let mut n_trades = 0usize;
                let cur_held: Vec<String> = holdings.keys().cloned().collect();
                for s in cur_held {
                    let cur_q = *holdings.get(&s).unwrap_or(&0);
                    let diff = desired.get(&s).copied().unwrap_or(0) - cur_q;
                    if diff < 0 {
                        let q = -diff;
                        let px = opens[&s][j] * (1.0 - slip);
                        cash += (q as f64) * px * (1.0 - comm - tax);
                        let new_q = cur_q - q;
                        if new_q <= 0 {
                            holdings.remove(&s);
                        } else {
                            holdings.insert(s.clone(), new_q);
                        }
                        traded_val += (q as f64) * px;
                        n_trades += 1;
                    }
                }
                for (s, &tgt_q) in &desired {
                    let cur_q = *holdings.get(s).unwrap_or(&0);
                    let diff = tgt_q - cur_q;
                    if diff > 0 {
                        let px = opens[s][j] * (1.0 + slip);
                        let unit = px * (1.0 + comm);
                        let q = if unit > 0.0 {
                            diff.min((cash / unit).floor() as i64)
                        } else {
                            0
                        };
                        if q > 0 {
                            cash -= (q as f64) * unit;
                            holdings.insert(s.clone(), cur_q + q);
                            traded_val += (q as f64) * px;
                            n_trades += 1;
                        }
                    }
                }
                let w_out: Map<String, Value> = pw
                    .iter()
                    .filter(|(_, &w)| w > 0.0)
                    .map(|(s, &w)| (s.clone(), json!((w * 1000.0).round() / 10.0)))
                    .collect();
                rebalances.push(json!({
                    "time": times[j],
                    "date": day_str,
                    "reason": planned_reason,
                    "trades": n_trades,
                    "turnover_pct": if val_open > 0.0 { ((traded_val / val_open * 100.0) * 100.0).round() / 100.0 } else { 0.0 },
                    "weights": w_out,
                }));
                last_rot_day = Some(day_str);
            }

            let eq = cash
                + holdings
                    .iter()
                    .map(|(s, &q)| (q as f64) * closes[s][j])
                    .sum::<f64>();
            equity.push(eq);

            if j + 1 < times.len() {
                let next_day = portfolio::day_str(times[j + 1]);
                if let Some(due_reason) = should_rotate(&next_day, last_rot_day.as_deref(), &cadence_owned) {
                    let mut slice_rows = Vec::new();
                    for s in &valid_syms {
                        let cs_slice = &closes[s][..=j];
                        if cs_slice.len() < 25 || *cs_slice.last().unwrap_or(&0.0) <= 0.0 {
                            continue;
                        }
                        let b_slice = Bars {
                            time: times[..=j].to_vec(),
                            open: opens[s][..=j].to_vec(),
                            high: cs_slice.to_vec(),
                            low: cs_slice.to_vec(),
                            close: cs_slice.to_vec(),
                            volume: vec![1000.0; j + 1],
                        };
                        let mut row = screener::compute_row_stats(&b_slice, None, capital, c);
                        let obj = row.as_object_mut().unwrap();
                        obj.insert("symbol".to_string(), json!(s));
                        obj.insert("name".to_string(), json!(name_map.get(s).unwrap_or(s)));
                        obj.insert("per".to_string(), json!(12.0));
                        obj.insert("pbr".to_string(), json!(1.2));
                        obj.insert("roe".to_string(), json!(15.0));
                        obj.insert("dividend_yield".to_string(), json!(3.0));
                        obj.insert("market_cap".to_string(), Value::Null);
                        obj.insert("flow_5d_for".to_string(), json!(100));
                        obj.insert("flow_5d_inst".to_string(), json!(100));
                        obj.insert("flow_20d_for".to_string(), json!(200));
                        obj.insert("flow_20d_inst".to_string(), json!(200));
                        obj.insert("foreign_delta_pct".to_string(), json!(0.1));
                        let r6 = obj.get("ret_6m_pct").cloned().unwrap_or(Value::Null);
                        obj.insert("fit_return_pct".to_string(), r6.clone());
                        obj.insert("fit_cagr_pct".to_string(), r6);
                        obj.insert("fit_mdd_pct".to_string(), json!(-10.0));
                        obj.insert("fit_sharpe".to_string(), json!(1.0));
                        obj.insert("fit_win_rate".to_string(), json!(55.0));
                        obj.insert("fit_trades".to_string(), json!(5));
                        obj.insert("warnings".to_string(), json!([]));
                        obj.insert("matched".to_string(), json!(false));
                        obj.insert("match_label".to_string(), json!(""));
                        obj.insert("score".to_string(), json!(0.0));
                        obj.insert("reasons".to_string(), json!([]));
                        slice_rows.push(row);
                    }
                    let eff_preset = if matches!(preset_owned.as_str(), "momentum_quality" | "oversold_bounce") {
                        preset_owned.as_str()
                    } else {
                        "momentum_quality"
                    };
                    screener::score_and_explain(eff_preset, &mut slice_rows);
                    let cur_syms: Vec<String> = holdings.keys().cloned().collect();
                    let (picks, _) = select_target_picks(&slice_rows, &cur_syms, top_n, hb);
                    if !picks.is_empty() {
                        let w_each = 1.0 / picks.len() as f64;
                        let mut pw = HashMap::new();
                        for p in picks {
                            if let Some(sym) = p.get("symbol").and_then(|v| v.as_str()) {
                                pw.insert(sym.to_string(), w_each);
                            }
                        }
                        planned_weights = Some(pw);
                        planned_reason = due_reason;
                    }
                }
            }
        }

        let eval_times = &times[start_idx..];
        let per_sym_cap = capital / valid_syms.len() as f64;
        let b_shares: HashMap<&String, f64> = valid_syms
            .iter()
            .filter(|s| closes[*s][start_idx] > 0.0)
            .map(|s| (s, per_sym_cap / closes[s][start_idx]))
            .collect();
        let bench_eq: Vec<f64> = (start_idx..times.len())
            .map(|j| {
                b_shares
                    .iter()
                    .map(|(s, &q)| q * closes[*s][j])
                    .sum::<f64>()
            })
            .collect();

        let dds = drawdown_series(&equity);
        let eq_rows: Vec<Value> = eval_times
            .iter()
            .zip(equity.iter())
            .zip(bench_eq.iter())
            .zip(dds.iter())
            .map(|(((&t, &e), &be), &dd)| {
                json!({
                    "time": t,
                    "equity": (e * 10000.0).round() / 10000.0,
                    "benchmark": (be * 10000.0).round() / 10000.0,
                    "drawdown": ((dd * 100.0) * 10000.0).round() / 10000.0,
                })
            })
            .collect();

        let final_eq = equity.last().copied().unwrap_or(capital);
        let mut cur_weights = Map::new();
        if final_eq > 0.0 {
            for (s, &q) in &holdings {
                let last_c = *closes[s].last().unwrap_or(&0.0);
                cur_weights.insert(
                    s.clone(),
                    json!((((q as f64) * last_c / final_eq) * 10000.0).round() / 100.0),
                );
            }
        }

        let mut metrics = performance(eval_times, &equity);
        if let Some(m) = metrics.as_object_mut() {
            m.insert("rebalances".to_string(), json!(rebalances.len()));
        }
        let benchmark = performance(eval_times, &bench_eq);

        Ok(json!({
            "market": market_str,
            "preset": preset_owned,
            "universe": uni_owned,
            "top_n": top_n,
            "hold_buffer": hb,
            "cadence": cadence_owned,
            "capital": capital,
            "currency": if market_str == "KR" { "KRW" } else { "USD" },
            "period": { "start": eval_times[0], "end": eval_times[eval_times.len() - 1] },
            "equity": eq_rows,
            "metrics": metrics,
            "benchmark": benchmark,
            "rebalances": rebalances,
            "current_weights": cur_weights,
        }))
    })
    .await
    .map_err(|e| ApiError::internal("worker_error", e.to_string()))?
}
