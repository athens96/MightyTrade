//! One trading-day / intraday step for a bot (paper & live).

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration as StdDuration;

use crate::account::toss_account::{account_client, HIGH_VALUE_KRW};
use crate::autotrade::{bots, dca, protections, rebalance, rotation, webhook};
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras::normalize, models::{Candle, Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{Bars, Context, Costs, Order, Position, RiskRules};
use crate::quant::market_timing;
use crate::quant::strategies::{get_spec, Strategy};
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::trading::ticks::{fmt_price, round_price};
use crate::users::repository::now_iso;

const LIVE_BUY_HEADROOM: f64 = 0.005;
const SELL_SETTLE_POLLS: usize = 5;
const SELL_SETTLE_INTERVAL_MS: u64 = 500;
const EOD_FLATTEN_MINS: i64 = 10;

fn is_terminal(status: &str) -> bool {
    matches!(status, "FILLED" | "CANCELED" | "REJECTED" | "REPLACED")
}

pub fn is_rebalance(bot: &Value) -> bool {
    bots::bot_kind(bot) == "rebalance"
}

pub fn is_rotation(bot: &Value) -> bool {
    bots::bot_kind(bot) == "rotation"
}

pub fn is_dca(bot: &Value) -> bool {
    bots::bot_kind(bot) == "dca"
}

pub fn is_webhook(bot: &Value) -> bool {
    bots::bot_kind(bot) == "webhook"
}

pub fn is_intraday(bot: &Value) -> bool {
    if is_rebalance(bot) || is_rotation(bot) || is_dca(bot) || is_webhook(bot) {
        return false;
    }
    bot.get("strategy")
        .and_then(|v| v.as_str())
        .and_then(get_spec)
        .is_some_and(|s| s.timeframe != "1d")
}

pub fn day_of(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

pub async fn costs_for(state: &AppState, uid: &str, market: Market) -> Costs {
    let d = cost_defaults(state, uid, market).await;
    Costs {
        commission_pct: d["commission_pct"].as_f64().unwrap_or(0.015),
        tax_pct: d["tax_pct"].as_f64().unwrap_or(0.0),
        slippage_pct: d["slippage_pct"].as_f64().unwrap_or(0.05),
    }
}

fn load_start(bot: &Value, warmup: usize) -> i64 {
    let mut anchor = Utc::now().timestamp() as f64;
    let pos = &bots::state(bot)["position"];
    if pos["qty"].as_f64().unwrap_or(0.0) > 0.0 {
        if let Some(ed) = pos.get("entry_date").and_then(|v| v.as_str()) {
            if let Ok(d) = NaiveDate::parse_from_str(&ed[..ed.len().min(10)], "%Y-%m-%d") {
                if let Some(dt) = d.and_hms_opt(0, 0, 0) {
                    let ts = Utc.from_utc_datetime(&dt).timestamp() as f64;
                    anchor = anchor.min(ts);
                }
            }
        }
    }
    (anchor - (warmup as f64 * 1.6 + 40.0) * 86400.0) as i64
}

pub async fn load(
    state: &AppState,
    bot: &Value,
    today: &str,
) -> ApiResult<(Vec<Candle>, Bars, Option<Candle>, Box<dyn Strategy>)> {
    let sid = bot["strategy"].as_str().unwrap_or("");
    let spec = get_spec(sid).ok_or_else(|| ApiError::code_only(400, "unknown_strategy"))?;
    let empty_obj = json!({});
    let strat = (spec.build)(bot.get("params").unwrap_or(&empty_obj))
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let uid = bot["user_id"].as_str().unwrap_or("");
    let sym = bot["symbol"].as_str().unwrap_or("");
    let start = load_start(bot, strat.warmup());
    let candles = market::get_history(state, uid, market, sym, Interval::D1, start).await?;
    let past: Vec<Candle> = candles
        .iter()
        .filter(|c| day_of(c.time).as_str() < today)
        .cloned()
        .collect();
    let today_bar = candles
        .iter()
        .find(|c| day_of(c.time).as_str() == today)
        .cloned();
    if past.len() < strat.warmup() + 2 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "전략 계산에 필요한 과거 데이터가 부족합니다",
        ));
    }
    Ok((candles, Bars::from_candles(&past), today_bar, strat))
}

pub async fn load_intraday(
    state: &AppState,
    bot: &Value,
) -> ApiResult<(Vec<Candle>, Bars, Candle, Box<dyn Strategy>)> {
    let sid = bot["strategy"].as_str().unwrap_or("");
    let spec = get_spec(sid).ok_or_else(|| ApiError::code_only(400, "unknown_strategy"))?;
    let empty_obj = json!({});
    let strat = (spec.build)(bot.get("params").unwrap_or(&empty_obj))
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let interval = Interval::parse(spec.timeframe)?;
    let uid = bot["user_id"].as_str().unwrap_or("");
    let sym = bot["symbol"].as_str().unwrap_or("");
    let (_, candles) = market::get_candles(state, uid, market, sym, interval, 200).await?;
    if candles.len() < strat.warmup() + 2 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "분봉 전략 계산에 필요한 과거 데이터가 부족합니다",
        ));
    }
    let bars = Bars::from_candles(&candles[..candles.len() - 1]);
    let latest = candles[candles.len() - 1].clone();
    Ok((candles, bars, latest, strat))
}

async fn apply_market_timing(
    state: &AppState,
    bot: &Value,
    today: &str,
    planned: Vec<Order>,
    holding: bool,
) -> (Vec<Order>, f64, Option<Value>) {
    let mt = bot.get("market_timing").and_then(|v| v.as_object());
    let mode = mt
        .and_then(|m| m.get("mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("off");
    if mode == "off" {
        return (planned, 1.0, None);
    }
    let uid = bot["user_id"].as_str().unwrap_or("");
    let m_str = bot["market"].as_str().unwrap_or("KR");
    let snap = market_timing::current_regime_snapshot(
        state,
        uid,
        m_str,
        mt.and_then(|m| m.get("benchmark")).and_then(|v| v.as_str()),
        mt.and_then(|m| m.get("ma_short")).and_then(|v| v.as_u64()).unwrap_or(20) as usize,
        mt.and_then(|m| m.get("ma_long")).and_then(|v| v.as_u64()).unwrap_or(60) as usize,
        Some(today),
    )
    .await;
    let reg = snap["regime"].as_str().unwrap_or("neutral");
    let scale = market_timing::buy_scale_for(mode, reg);
    if mode == "exit_all" && reg == "bear" {
        let mut sells: Vec<Order> = planned.into_iter().filter(|o| o.side == "sell").collect();
        if holding && sells.is_empty() {
            let lbl = snap["label"].as_str().unwrap_or("하락장");
            sells.push(Order::market("sell", format!("마켓 타이밍 현금 대피 ({lbl})")));
        }
        return (sells, 0.0, Some(snap));
    }
    if scale <= 0.0 {
        let filtered: Vec<Order> = planned.into_iter().filter(|o| o.side != "buy").collect();
        return (filtered, 0.0, Some(snap));
    }
    (planned, scale, Some(snap))
}

struct Decision {
    bar_date: String,
    orders: Vec<Order>,
    today_bar: Option<Candle>,
    position: Value,
    cash: f64,
    last: f64,
    entry_atr: Option<f64>,
    mt_scale: f64,
    market_regime: Option<Value>,
    mt_blocked: bool,
}

async fn decide_with_loaded(
    state: &AppState,
    bot: &Value,
    today: &str,
    bars: &Bars,
    today_bar: Option<Candle>,
    strat: &mut dyn Strategy,
    intraday: bool,
) -> Decision {
    let st = bots::state(bot);
    let pos = &st["position"];
    let qty = pos["qty"].as_f64().unwrap_or(0.0);
    let ep = pos["entry_price"].as_f64().unwrap_or(0.0);
    let entry_index: isize = if qty > 0.0 {
        if intraday {
            let entry_t = st.get("entry_bar_time").and_then(|v| v.as_i64());
            match entry_t {
                Some(t) if t >= bars.time[0] => {
                    bars.time.partition_point(|&x| x < t) as isize
                }
                _ => -1,
            }
        } else {
            let days: Vec<String> = bars.time.iter().map(|&t| day_of(t)).collect();
            match pos.get("entry_date").and_then(|v| v.as_str()) {
                None => (days.len() as isize) - 1,
                Some(ed) if ed < days[0].as_str() => -1,
                Some(ed) => days.partition_point(|d| d.as_str() < ed) as isize,
            }
        }
    } else {
        -1
    };

    let position = Position {
        qty,
        entry_price: ep,
        entry_index,
        entry_atr: st.get("entry_atr").and_then(|v| v.as_f64()),
    };
    let last = today_bar
        .as_ref()
        .map(|b| b.close)
        .unwrap_or(*bars.close.last().unwrap_or(&0.0));
    let cash = if bot["mode"].as_str() == Some("paper") {
        st["cash"].as_f64().unwrap_or(0.0)
    } else {
        (bot["capital"].as_f64().unwrap_or(0.0) - qty * ep).max(0.0)
    };
    strat.prepare(bars);
    let raw_planned = strat.on_bar(
        bars.len() - 1,
        &Context {
            position,
            cash,
            equity: cash + qty * last,
        },
    );
    let had_buy = raw_planned.iter().any(|o| o.side == "buy");
    let (planned, mt_scale, mt_snap) =
        apply_market_timing(state, bot, today, raw_planned, qty > 0.0).await;
    let mt_blocked = !intraday && had_buy && !planned.iter().any(|o| o.side == "buy");

    let last_t = *bars.time.last().unwrap_or(&0);
    let bar_date = if intraday {
        DateTime::from_timestamp(last_t, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| day_of(last_t))
    } else {
        day_of(last_t)
    };

    Decision {
        bar_date,
        orders: planned,
        today_bar,
        position: pos.clone(),
        cash,
        last,
        entry_atr: strat.last_atr(),
        mt_scale,
        market_regime: mt_snap,
        mt_blocked,
    }
}

pub fn describe(o: &Order, today_open: Option<f64>) -> String {
    let side = if o.side == "buy" { "매수" } else { "매도" };
    if o.kind == "market" {
        return format!("{side} (시장가) — {}", o.reason);
    }
    if o.kind == "limit" {
        if let Some(lim) = o.limit {
            let cmp = if o.side == "buy" { "≤" } else { "≥" };
            return format!("{side} 지정가: {cmp} {lim:.2} — {}", o.reason);
        }
    }
    if let Some(st) = o.stop {
        let cmp = if o.side == "buy" { "≥" } else { "≤" };
        return format!("{side} 조건: {cmp} {st:.2} — {}", o.reason);
    }
    let sfo = o.stop_from_open.unwrap_or(0.0);
    let level = match today_open {
        Some(op) => format!("{:.2}", op + sfo),
        None => format!("시가 + {sfo:.2}"),
    };
    format!("{side} 조건: ≥ {level} (시가 + {sfo:.2}) — {}", o.reason)
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    if is_rebalance(bot) {
        return rebalance::preview(state, bot, today).await;
    }
    if is_rotation(bot) {
        return rotation::preview(state, bot, today).await;
    }
    if is_dca(bot) {
        return dca::preview(state, bot, today).await;
    }
    if is_webhook(bot) {
        return webhook::preview(state, bot, today).await;
    }
    let d = if is_intraday(bot) {
        let (_, bars, latest, mut strat) = load_intraday(state, bot).await?;
        decide_with_loaded(state, bot, today, &bars, Some(latest), strat.as_mut(), true).await
    } else {
        let (_, bars, tb, mut strat) = load(state, bot, today).await?;
        decide_with_loaded(state, bot, today, &bars, tb, strat.as_mut(), false).await
    };
    let to = d.today_bar.as_ref().map(|b| b.open);
    let orders_json: Vec<Value> = d
        .orders
        .iter()
        .map(|o| {
            json!({
                "side": o.side,
                "kind": o.kind,
                "stop": o.stop,
                "stop_from_open": o.stop_from_open,
                "limit": o.limit,
                "qty": o.qty,
                "reason": o.reason,
                "text": describe(o, to),
            })
        })
        .collect();
    Ok(json!({
        "date": today,
        "bar_date": d.bar_date,
        "position": d.position,
        "cash": d.cash,
        "today_open": to,
        "last": d.last,
        "market_regime": d.market_regime,
        "orders": orders_json,
    }))
}

fn order_level(o: &Order, today_open: f64) -> f64 {
    o.stop
        .unwrap_or_else(|| today_open + o.stop_from_open.unwrap_or(0.0))
}

fn bot_risk(bot: &Value) -> RiskRules {
    RiskRules::from_value(bot.get("risk")).unwrap_or_default()
}

fn risk_levels(risk: RiskRules, entry_price: f64, peak_price: f64) -> (Option<f64>, String, Option<f64>) {
    let mut eff_stop: Option<f64> = None;
    let mut stop_reason = String::new();
    if risk.stop_loss_pct > 0.0 {
        eff_stop = Some(entry_price * (1.0 - risk.stop_loss_pct / 100.0));
        stop_reason = format!("공통 손절 (-{}%)", protections::fmt_g(risk.stop_loss_pct));
    }
    if risk.trailing_stop_pct > 0.0 && peak_price > 0.0 {
        let tr = peak_price * (1.0 - risk.trailing_stop_pct / 100.0);
        if eff_stop.is_none_or(|es| tr > es) {
            eff_stop = Some(tr);
            stop_reason = format!(
                "트레일링 스톱 (고점 대비 -{}%)",
                protections::fmt_g(risk.trailing_stop_pct)
            );
        }
    }
    let tp_level = if risk.take_profit_pct > 0.0 {
        Some(entry_price * (1.0 + risk.take_profit_pct / 100.0))
    } else {
        None
    };
    (eff_stop, stop_reason, tp_level)
}

pub fn paper_buy(
    st: &mut Value,
    qty: i64,
    price: f64,
    c: Costs,
    date: &str,
    reason: &str,
    entry_atr: Option<f64>,
    bar_high: Option<f64>,
) {
    let px = price * (1.0 + c.slippage_pct / 100.0);
    let value = (qty as f64) * px;
    let fee = value * c.commission_pct / 100.0;
    let cash = st["cash"].as_f64().unwrap_or(0.0);
    st["cash"] = json!(cash - value - fee);
    st["position"] = json!({ "qty": qty, "entry_price": px, "entry_date": date });
    st["peak_price"] = json!(px.max(bar_high.unwrap_or(px)));
    if let Some(a) = entry_atr {
        st["entry_atr"] = json!(a);
    }
    st["trades"].as_array_mut().unwrap().push(json!({
        "entry_date": date,
        "entry_price": px,
        "qty": qty,
        "entry_reason": reason,
        "cost_basis": value + fee,
        "exit_date": Value::Null,
        "exit_price": Value::Null,
        "exit_reason": "",
        "pnl": Value::Null,
        "return_pct": Value::Null,
    }));
}

pub fn paper_sell(st: &mut Value, price: f64, c: Costs, date: &str, reason: &str) {
    let qty = st["position"]["qty"].as_f64().unwrap_or(0.0);
    let px = price * (1.0 - c.slippage_pct / 100.0);
    let value = qty * px;
    let net = value - value * (c.commission_pct + c.tax_pct) / 100.0;
    let cash = st["cash"].as_f64().unwrap_or(0.0);
    st["cash"] = json!(cash + net);
    if let Some(trades) = st["trades"].as_array_mut() {
        if let Some(t) = trades
            .iter_mut()
            .rev()
            .find(|t| t.get("exit_date").is_none_or(|v| v.is_null()))
        {
            let cb = t.get("cost_basis").and_then(|v| v.as_f64()).unwrap_or(value);
            t["exit_date"] = json!(date);
            t["exit_price"] = json!(px);
            t["exit_reason"] = json!(reason);
            t["pnl"] = json!(net - cb);
            t["return_pct"] = json!(if cb > 0.0 { (net / cb - 1.0) * 100.0 } else { 0.0 });
        }
    }
    st["position"] = json!({ "qty": 0, "entry_price": 0.0, "entry_date": Value::Null });
    if let Some(obj) = st.as_object_mut() {
        obj.remove("entry_atr");
        obj.remove("peak_price");
    }
}

fn paper_qty(st: &Value, o_qty: Option<f64>, price: f64, c: Costs, scale: f64) -> i64 {
    let cash = st["cash"].as_f64().unwrap_or(0.0);
    let denom = price * (1.0 + c.slippage_pct / 100.0) * (1.0 + c.commission_pct / 100.0);
    if denom <= 0.0 {
        return 0;
    }
    let qty = ((cash * scale.min(1.0)) / denom).floor() as i64;
    match o_qty {
        Some(oq) => qty.min((oq * scale) as i64),
        None => qty,
    }
}

async fn settle_paper_risk_on_bar(
    state: &AppState,
    bot: &mut Value,
    bar: &Candle,
    bar_date: &str,
    c: Costs,
) -> bool {
    let risk = bot_risk(bot);
    let pos_qty = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let ep = bot["state"]["paper"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    if !risk.enabled() || pos_qty <= 0.0 {
        if pos_qty > 0.0 {
            let pk = bot["state"]["paper"]
                .get("peak_price")
                .and_then(|v| v.as_f64())
                .unwrap_or(ep)
                .max(bar.high);
            bot["state"]["paper"]["peak_price"] = json!(pk);
        }
        return false;
    }
    let peak = bot["state"]["paper"]
        .get("peak_price")
        .and_then(|v| v.as_f64())
        .unwrap_or(ep);
    let (eff_stop, stop_reason, tp_level) = risk_levels(risk, ep, peak);
    if let Some(es) = eff_stop {
        if bar.low <= es {
            let fill = bar.open.min(es);
            paper_sell(&mut bot["state"]["paper"], fill, c, bar_date, &stop_reason);
            bots::log(
                state,
                bot,
                "fill",
                &format!("[모의/보호막] {stop_reason} 매도 @ {fill:.2} ({bar_date})"),
                "info",
                None,
            )
            .await;
            return true;
        }
    }
    if let Some(tp) = tp_level {
        if bar.high >= tp {
            let fill = bar.open.max(tp);
            let reason = format!("공통 익절 (+{}%)", protections::fmt_g(risk.take_profit_pct));
            paper_sell(&mut bot["state"]["paper"], fill, c, bar_date, &reason);
            bots::log(
                state,
                bot,
                "fill",
                &format!("[모의/보호막] {reason} 매도 @ {fill:.2} ({bar_date})"),
                "info",
                None,
            )
            .await;
            return true;
        }
    }
    bot["state"]["paper"]["peak_price"] = json!(peak.max(bar.high));
    false
}

async fn settle_paper(state: &AppState, bot: &mut Value, candles: &[Candle], today: &str, c: Costs) {
    let by_day: HashMap<String, &Candle> = candles.iter().map(|x| (day_of(x.time), x)).collect();
    let last_run = bot["state"]["paper"]
        .get("last_run_date")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    if bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 {
        let entry_d = bot["state"]["paper"]["position"]
            .get("entry_date")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let start_after = match &last_run {
            Some(lr) => entry_d.max(lr.clone()),
            None => entry_d,
        };
        let mut days: Vec<String> = by_day.keys().cloned().collect();
        days.sort();
        for d_str in days {
            if d_str >= start_after && d_str.as_str() < today {
                if settle_paper_risk_on_bar(state, bot, by_day[&d_str], &d_str, c).await {
                    break;
                }
            }
        }
    }

    let pending = bot["state"]["paper"]["pending"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut keep = Vec::new();
    for p in pending {
        let ad = p["active_date"].as_str().unwrap_or("");
        if ad >= today {
            keep.push(p);
            continue;
        }
        let Some(&bar) = by_day.get(ad) else {
            continue;
        };
        let level = p["level"].as_f64().unwrap_or(0.0);
        let holding = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0;
        let kind = p.get("kind").and_then(|v| v.as_str()).unwrap_or("stop");
        let side = p["side"].as_str().unwrap_or("");
        let reason = p.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        let p_qty = p.get("qty").and_then(|v| v.as_f64());
        let p_atr = p.get("entry_atr").and_then(|v| v.as_f64());

        if kind == "limit" {
            if side == "buy" && !holding && bar.low <= level {
                let fill = bar.open.min(level);
                let qty = paper_qty(&bot["state"]["paper"], p_qty, fill, c, 1.0);
                if qty > 0 {
                    paper_buy(&mut bot["state"]["paper"], qty, fill, c, ad, reason, p_atr, Some(bar.high));
                    bots::log(state, bot, "fill", &format!("[모의] 지정가 매수 체결 {qty}주 @ {fill:.2} ({ad})"), "info", None).await;
                }
            } else if side == "sell" && holding && bar.high >= level {
                let fill = bar.open.max(level);
                paper_sell(&mut bot["state"]["paper"], fill, c, ad, reason);
                bots::log(state, bot, "fill", &format!("[모의] 지정가 매도 체결 @ {fill:.2} ({ad})"), "info", None).await;
            }
        } else if side == "buy" && !holding && bar.high >= level {
            let fill = bar.open.max(level);
            let qty = paper_qty(&bot["state"]["paper"], p_qty, fill, c, 1.0);
            if qty > 0 {
                paper_buy(&mut bot["state"]["paper"], qty, fill, c, ad, reason, p_atr, Some(bar.high));
                bots::log(state, bot, "fill", &format!("[모의] 매수 체결 {qty}주 @ {fill:.2} ({ad} 조건 충족)"), "info", None).await;
            }
        } else if side == "sell" && holding && bar.low <= level {
            let fill = bar.open.min(level);
            paper_sell(&mut bot["state"]["paper"], fill, c, ad, reason);
            bots::log(state, bot, "fill", &format!("[모의] 매도 체결 @ {fill:.2} ({ad} 조건 충족)"), "info", None).await;
        }
    }
    bot["state"]["paper"]["pending"] = Value::Array(keep);
}

pub async fn run_paper(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["paper"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let c = costs_for(state, &uid, market).await;
    let (candles, bars, today_bar_opt, mut strat) = load(state, bot, today).await?;
    let today_bar = today_bar_opt.ok_or_else(|| {
        ApiError::conflict(
            "market_not_open",
            "오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다",
        )
    })?;
    settle_paper(state, bot, &candles, today, c).await;

    let risk = bot_risk(bot);
    if bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 && risk.enabled() {
        let ep = bot["state"]["paper"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
        let peak = bot["state"]["paper"]
            .get("peak_price")
            .and_then(|v| v.as_f64())
            .unwrap_or(ep);
        let (eff_stop, stop_reason, tp_level) = risk_levels(risk, ep, peak);
        if eff_stop.is_some_and(|es| today_bar.open <= es) {
            paper_sell(&mut bot["state"]["paper"], today_bar.open, c, today, &stop_reason);
            bots::log(state, bot, "fill", &format!("[모의/보호막] {stop_reason} 시가 매도 @ {:.2}", today_bar.open), "info", None).await;
        } else if tp_level.is_some_and(|tp| today_bar.open >= tp) {
            let reason = format!("공통 익절 (+{}%)", protections::fmt_g(risk.take_profit_pct));
            paper_sell(&mut bot["state"]["paper"], today_bar.open, c, today, &reason);
            bots::log(state, bot, "fill", &format!("[모의/보호막] {reason} 시가 매도 @ {:.2}", today_bar.open), "info", None).await;
        }
    }

    let d = decide_with_loaded(state, bot, today, &bars, Some(today_bar.clone()), strat.as_mut(), false).await;
    if d.mt_blocked {
        let lbl = d
            .market_regime
            .as_ref()
            .and_then(|r| r.get("label"))
            .and_then(|v| v.as_str())
            .unwrap_or("하락장");
        bots::log(
            state,
            bot,
            "skip",
            &format!("[모의] 🚦 마켓 타이밍({lbl}) 필터로 신규 매수 중단"),
            "warn",
            None,
        )
        .await;
    }

    let o_open = today_bar.open;
    let est_eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
        + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * o_open;
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["paper"], today, Some(est_eq));

    for o in &d.orders {
        let holding = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0;
        if (o.side == "buy" && holding) || (o.side == "sell" && !holding) {
            continue;
        }
        if o.side == "buy" {
            if let Some(ref blk) = buy_block {
                bots::log(state, bot, "skip", &format!("[모의] 신규 매수 보류: {blk}"), "warn", None).await;
                continue;
            }
        }
        if o.kind == "market" {
            if o.side == "buy" {
                let qty = paper_qty(&bot["state"]["paper"], o.qty, o_open, c, d.mt_scale);
                if qty <= 0 {
                    bots::log(state, bot, "skip", "[모의] 자금 부족으로 매수하지 못했습니다", "warn", None).await;
                    continue;
                }
                paper_buy(&mut bot["state"]["paper"], qty, o_open, c, today, &o.reason, d.entry_atr, None);
                bots::log(state, bot, "fill", &format!("[모의] 매수 체결 {qty}주 @ 시가 {o_open:.2} — {}", o.reason), "info", None).await;
            } else {
                paper_sell(&mut bot["state"]["paper"], o_open, c, today, &o.reason);
                bots::log(state, bot, "fill", &format!("[모의] 매도 체결 @ 시가 {o_open:.2} — {}", o.reason), "info", None).await;
            }
        } else if o.kind == "limit" && o.limit.is_some() {
            let level = o.limit.unwrap();
            let gapped = (o.side == "buy" && o_open <= level) || (o.side == "sell" && o_open >= level);
            if gapped {
                if o.side == "buy" {
                    let qty = paper_qty(&bot["state"]["paper"], o.qty, o_open, c, 1.0);
                    if qty > 0 {
                        paper_buy(&mut bot["state"]["paper"], qty, o_open, c, today, &o.reason, d.entry_atr, None);
                        bots::log(state, bot, "fill", &format!("[모의] 시가가 지정가 이하라 시가 {o_open:.2}에 매수 {qty}주 — {}", o.reason), "info", None).await;
                    }
                } else {
                    paper_sell(&mut bot["state"]["paper"], o_open, c, today, &o.reason);
                    bots::log(state, bot, "fill", &format!("[모의] 시가가 지정가 이상이라 시가 {o_open:.2}에 매도 — {}", o.reason), "info", None).await;
                }
            } else {
                let mut item = json!({
                    "active_date": today,
                    "kind": "limit",
                    "side": o.side,
                    "level": level,
                    "qty": o.qty,
                    "reason": o.reason,
                });
                if o.side == "buy" {
                    if let Some(a) = d.entry_atr {
                        item["entry_atr"] = json!(a);
                    }
                }
                bot["state"]["paper"]["pending"].as_array_mut().unwrap().push(item);
                bots::log(state, bot, "order", &format!("[모의] {} (오늘 하루 유효)", describe(o, Some(o_open))), "info", None).await;
            }
        } else {
            let level = order_level(o, o_open);
            let gapped = (o.side == "buy" && o_open >= level) || (o.side == "sell" && o_open <= level);
            if gapped {
                if o.side == "buy" {
                    let qty = paper_qty(&bot["state"]["paper"], o.qty, o_open, c, 1.0);
                    if qty > 0 {
                        paper_buy(&mut bot["state"]["paper"], qty, o_open, c, today, &o.reason, d.entry_atr, None);
                        bots::log(state, bot, "fill", &format!("[모의] 시가가 기준가를 넘어 시가 {o_open:.2}에 매수 {qty}주 — {}", o.reason), "info", None).await;
                    }
                } else {
                    paper_sell(&mut bot["state"]["paper"], o_open, c, today, &o.reason);
                    bots::log(state, bot, "fill", &format!("[모의] 시가가 기준가 아래라 시가 {o_open:.2}에 매도 — {}", o.reason), "info", None).await;
                }
            } else {
                let mut item = json!({
                    "active_date": today,
                    "side": o.side,
                    "level": level,
                    "qty": o.qty,
                    "reason": o.reason,
                });
                if o.side == "buy" {
                    if let Some(a) = d.entry_atr {
                        item["entry_atr"] = json!(a);
                    }
                }
                bot["state"]["paper"]["pending"].as_array_mut().unwrap().push(item);
                bots::log(state, bot, "order", &format!("[모의] {} (오늘 하루 유효)", describe(o, Some(o_open))), "info", None).await;
            }
        }
    }
    if d.orders.is_empty() {
        bots::log(state, bot, "decision", &format!("[모의] {} 종가 기준 신호 없음", d.bar_date), "info", None).await;
    }
    bot["state"]["paper"]["last_run_date"] = json!(today);
    bot["state"]["paper"]["last_price"] = json!(today_bar.close);
    let eq_val = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
        + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * today_bar.close;
    bot["state"]["paper"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": eq_val }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today }))
}

// ------------------------------------------------------------------ live
fn client_order_id(bot_id: &str, today: &str, n: usize, tag: &str) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:{today}:{n}:{tag}"))
}

fn apply_live_fill(
    st: &mut Value,
    side: &str,
    qty: f64,
    avg: f64,
    date: &str,
    entry_atr: Option<f64>,
) {
    let old_q = st["position"]["qty"].as_f64().unwrap_or(0.0);
    let old_ep = st["position"]["entry_price"].as_f64().unwrap_or(0.0);
    if side == "BUY" {
        let new_q = old_q + qty;
        let new_ep = if new_q > 0.0 {
            (old_q * old_ep + qty * avg) / new_q
        } else {
            avg
        };
        st["position"]["entry_price"] = json!(new_ep);
        if old_q == 0.0 {
            st["position"]["entry_date"] = json!(date);
            if let Some(a) = entry_atr {
                st["entry_atr"] = json!(a);
            }
            st["trades"].as_array_mut().unwrap().push(json!({
                "entry_date": date,
                "entry_price": avg,
                "qty": qty,
                "entry_reason": "",
                "exit_date": Value::Null,
                "exit_price": Value::Null,
                "exit_reason": "",
                "pnl": Value::Null,
                "return_pct": Value::Null,
            }));
        }
        st["position"]["qty"] = json!(new_q);
        let pk = st.get("peak_price").and_then(|v| v.as_f64()).unwrap_or(0.0).max(avg);
        st["peak_price"] = json!(pk);
    } else {
        let sold = qty.min(old_q);
        let realized = st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
        st["realized"] = json!(realized + (avg - old_ep) * sold);
        let new_q = old_q - sold;
        st["position"]["qty"] = json!(new_q);
        if new_q <= 0.0 {
            if let Some(trades) = st["trades"].as_array_mut() {
                if let Some(t) = trades
                    .iter_mut()
                    .rev()
                    .find(|t| t.get("exit_date").is_none_or(|v| v.is_null()))
                {
                    let tep = t["entry_price"].as_f64().unwrap_or(old_ep);
                    let tq = t["qty"].as_f64().unwrap_or(sold);
                    t["exit_date"] = json!(date);
                    t["exit_price"] = json!(avg);
                    t["pnl"] = json!((avg - tep) * tq);
                    t["return_pct"] = json!(if tep > 0.0 { (avg / tep - 1.0) * 100.0 } else { 0.0 });
                }
            }
            st["position"]["entry_price"] = json!(0.0);
            st["position"]["entry_date"] = Value::Null;
            if let Some(obj) = st.as_object_mut() {
                obj.remove("entry_atr");
                obj.remove("peak_price");
            }
        }
    }
}

pub async fn reconcile_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<bool> {
    if is_rebalance(bot) {
        return rebalance::reconcile_live(state, bot, today).await;
    }
    if is_rotation(bot) {
        return rotation::reconcile_live(state, bot, today).await;
    }
    let pending = bot["state"]["live"]["pending"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if pending.is_empty() {
        return Ok(false);
    }
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_sym = bot["symbol"].as_str().unwrap_or("").to_string();
    let (client, seq) = account_client(state, &uid)?;
    let mut changed = false;
    let mut keep = Vec::new();

    for mut p in pending {
        let p_kind = p["kind"].as_str().unwrap_or("order").to_string();
        let pid = p["id"].as_str().unwrap_or("").to_string();
        let side = p["side"].as_str().unwrap_or("").to_string();

        if p_kind == "conditional" {
            match client
                .request(
                    "GET",
                    &format!("/api/v1/conditional-orders/{pid}"),
                    None,
                    None,
                    Some(seq),
                )
                .await
            {
                Ok(raw) => {
                    let co = normalize(raw.unwrap_or_else(|| json!({})), "");
                    let trig = co
                        .get("first")
                        .and_then(|x| x.get("triggeredOrderId"))
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            co.get("second")
                                .and_then(|x| x.get("triggeredOrderId"))
                                .and_then(|v| v.as_str())
                        })
                        .map(|s| s.to_string());
                    if let Some(trig_id) = trig {
                        p["kind"] = json!("order");
                        p["id"] = json!(trig_id);
                        bots::log(state, bot, "fill", &format!("조건 충족 → 주문 생성 ({side})"), "info", None).await;
                        // Fall through to poll the triggered order below
                    } else {
                        let st_top = co.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        let first_done = matches!(
                            co.get("first").and_then(|x| x.get("status")).and_then(|v| v.as_str()),
                            Some("CANCELED" | "EXPIRED")
                        );
                        let second_done = co.get("second").is_none_or(|s| {
                            s.is_null()
                                || matches!(
                                    s.get("status").and_then(|v| v.as_str()),
                                    Some("CANCELED" | "EXPIRED")
                                )
                        });
                        if matches!(st_top, "COMPLETED" | "EXPIRED" | "CANCELED")
                            || (first_done && second_done)
                        {
                            changed = true;
                            continue;
                        }
                        keep.push(p);
                        continue;
                    }
                }
                Err(e) => {
                    bots::log(state, bot, "error", &format!("체결 확인 실패: {}", e.message), "warn", None).await;
                    keep.push(p);
                    continue;
                }
            }
        }

        let oid = p["id"].as_str().unwrap_or("").to_string();
        match client
            .request("GET", &format!("/api/v1/orders/{oid}"), None, None, Some(seq))
            .await
        {
            Ok(raw) => {
                let o = normalize(raw.unwrap_or_else(|| json!({})), "");
                let ex = o.get("execution").and_then(|v| v.as_object());
                let filled = ex
                    .and_then(|e| e.get("filledQuantity"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let synced = p.get("synced").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let delta = filled - synced;
                if delta > 0.0 {
                    if let Some(avg_px) = ex
                        .and_then(|e| e.get("averageFilledPrice"))
                        .and_then(|v| v.as_f64())
                        .filter(|&x| x > 0.0)
                    {
                        apply_live_fill(
                            &mut bot["state"]["live"],
                            &side,
                            delta,
                            avg_px,
                            today,
                            p.get("entry_atr").and_then(|v| v.as_f64()),
                        );
                        let sym = p
                            .get("symbol")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&bot_sym);
                        bots::record_live_slippage(
                            &mut bot["state"]["live"],
                            sym,
                            &side,
                            delta,
                            p.get("signal_price").and_then(|v| v.as_f64()),
                            avg_px,
                            today,
                        );
                        p["synced"] = json!(filled);
                        changed = true;
                        bots::log(
                            state,
                            bot,
                            "fill",
                            &format!(
                                "{} 체결 {}주 @ {avg_px:.2}",
                                if side == "BUY" { "매수" } else { "매도" },
                                protections::fmt_g(delta)
                            ),
                            "info",
                            None,
                        )
                        .await;
                    }
                }
                let status = o.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if is_terminal(status) {
                    if status == "REJECTED" {
                        bots::log(state, bot, "error", &format!("주문 거부됨 ({side})"), "error", None).await;
                    }
                    changed = true;
                    continue;
                }
                keep.push(p);
            }
            Err(e) => {
                bots::log(state, bot, "error", &format!("체결 확인 실패: {}", e.message), "warn", None).await;
                keep.push(p);
            }
        }
    }

    bot["state"]["live"]["pending"] = Value::Array(keep);
    bot["state"]["live"]["last_reconcile"] = json!(now_iso());
    if bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 && bot_risk(bot).enabled() {
        if let Err(e) = sync_live_protection(state, bot, today, None).await {
            bots::log(state, bot, "error", &format!("보호막 주문 갱신 실패: {}", e.message), "warn", None).await;
        }
    }
    Ok(changed)
}

async fn send_market(
    state: &AppState,
    bot: &Value,
    side: &str,
    qty: i64,
    est_price: f64,
    cid: &str,
) -> ApiResult<Value> {
    let m = bot["market"].as_str().unwrap_or("KR");
    let sym = bot["symbol"].as_str().unwrap_or("");
    let uid = bot["user_id"].as_str().unwrap_or("");
    let bid = bot["id"].as_str().unwrap_or("");
    let payload = json!({
        "clientOrderId": cid,
        "symbol": sym,
        "side": side,
        "orderType": "MARKET",
        "timeInForce": "DAY",
        "quantity": qty.to_string(),
    });
    let built = json!({
        "market": m,
        "payload": payload,
        "checks": { "estimated_value": (qty as f64) * est_price },
    });
    trading::submit(state, &built, uid, &format!("bot:{bid}")).await
}

async fn send_limit(
    state: &AppState,
    bot: &Value,
    side: &str,
    qty: i64,
    limit_price: f64,
    cid: &str,
) -> ApiResult<Value> {
    let m = bot["market"].as_str().unwrap_or("KR");
    let sym = bot["symbol"].as_str().unwrap_or("");
    let uid = bot["user_id"].as_str().unwrap_or("");
    let bid = bot["id"].as_str().unwrap_or("");
    let px = round_price(m, limit_price, if side == "BUY" { "down" } else { "up" });
    let payload = json!({
        "clientOrderId": cid,
        "symbol": sym,
        "side": side,
        "orderType": "LIMIT",
        "timeInForce": "DAY",
        "quantity": qty.to_string(),
        "price": fmt_price(m, px),
    });
    let built = json!({
        "market": m,
        "payload": payload,
        "checks": { "estimated_value": (qty as f64) * px },
    });
    trading::submit(state, &built, uid, &format!("bot:{bid}")).await
}

pub async fn send_conditional(
    state: &AppState,
    bot: &Value,
    side: &str,
    qty: i64,
    level: f64,
    expire_date: &str,
    cid: &str,
) -> ApiResult<Value> {
    let m = bot["market"].as_str().unwrap_or("KR");
    let market = Market::parse(m)?;
    let sym = bot["symbol"].as_str().unwrap_or("");
    let uid = bot["user_id"].as_str().unwrap_or("");
    let bid = bot["id"].as_str().unwrap_or("");
    trading::check_limits(state, uid, market, (qty as f64) * level)?;
    let trigger = round_price(m, level, if side == "BUY" { "up" } else { "down" });
    let payload = json!({
        "clientOrderId": cid,
        "symbol": sym,
        "type": "SINGLE",
        "quantity": qty.to_string(),
        "orderType": "MARKET",
        "expireDate": expire_date,
        "first": { "orderSide": side, "triggerPrice": fmt_price(m, trigger) },
    });
    let (client, seq) = account_client(state, uid)?;
    let result = normalize(
        client
            .request(
                "POST",
                "/api/v1/conditional-orders",
                None,
                Some(&payload),
                Some(seq),
            )
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    );
    state.store.put(
        "orders",
        cid,
        json!({
            "user_id": uid,
            "kind": "conditional_single",
            "source": format!("bot:{bid}"),
            "market": m,
            "symbol": sym,
            "side": side,
            "payload": payload,
            "result": result,
            "date": trading::kst_today(),
            "created_at": now_iso(),
        }),
    );
    Ok(result)
}

pub async fn cancel_protection_conditionals(state: &AppState, bot: &mut Value) {
    let pending = bot["state"]["live"]["pending"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let has_prot = pending
        .iter()
        .any(|p| p.get("kind").and_then(|v| v.as_str()) == Some("conditional") && p.get("role").and_then(|v| v.as_str()) == Some("protection"));
    if !has_prot {
        return;
    }
    let uid = bot["user_id"].as_str().unwrap_or("");
    let Ok((client, seq)) = account_client(state, uid) else {
        return;
    };
    let mut keep = Vec::new();
    for p in pending {
        if p.get("kind").and_then(|v| v.as_str()) == Some("conditional")
            && p.get("role").and_then(|v| v.as_str()) == Some("protection")
        {
            let pid = p["id"].as_str().unwrap_or("");
            if client
                .request("DELETE", &format!("/api/v1/conditional-orders/{pid}"), None, None, Some(seq))
                .await
                .is_err()
            {
                keep.push(p);
            }
        } else {
            keep.push(p);
        }
    }
    bot["state"]["live"]["pending"] = Value::Array(keep);
}

pub async fn sync_live_protection(
    state: &AppState,
    bot: &mut Value,
    today: &str,
    current_opt: Option<f64>,
) -> ApiResult<()> {
    let risk = bot_risk(bot);
    if !risk.enabled() {
        return Ok(());
    }
    let pos_qty = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let ep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    if pos_qty <= 0.0 {
        return Ok(());
    }
    let sell_in_flight = bot["state"]["live"]["pending"]
        .as_array()
        .is_some_and(|a| {
            a.iter().any(|p| {
                p.get("side").and_then(|v| v.as_str()) == Some("SELL")
                    && p.get("role").and_then(|v| v.as_str()) != Some("protection")
            })
        });
    if sell_in_flight {
        return Ok(());
    }

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let sym = bot["symbol"].as_str().unwrap_or("").to_string();
    let market = Market::parse(&m)?;
    let (client, seq) = account_client(state, &uid)?;

    let current = match current_opt {
        Some(v) => v,
        None => {
            let mut pp = HashMap::new();
            pp.insert("symbols".to_string(), sym.clone());
            client
                .request("GET", "/api/v1/prices", Some(&pp), None, None)
                .await?
                .and_then(|v| v.as_array().cloned())
                .and_then(|a| a.first().cloned())
                .and_then(|r| r.get("lastPrice").cloned())
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(ep)
        }
    };
    let peak = bot["state"]["live"]
        .get("peak_price")
        .and_then(|v| v.as_f64())
        .unwrap_or(ep)
        .max(current);
    bot["state"]["live"]["peak_price"] = json!(peak);

    let (eff_stop, stop_reason, tp_level) = risk_levels(risk, ep, peak);
    let fetch_sellable = || async {
        let mut sq = HashMap::new();
        sq.insert("symbol".to_string(), sym.clone());
        client
            .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
            .await
            .map(|opt| {
                opt.and_then(|v| v.get("sellableQuantity").cloned())
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0.0)
            })
    };
    let mut qty = pos_qty.min(fetch_sellable().await?) as i64;

    if eff_stop.is_some_and(|es| current <= es) || tp_level.is_some_and(|tp| current >= tp) {
        cancel_protection_conditionals(state, bot).await;
        qty = pos_qty.min(fetch_sellable().await?) as i64;
        if qty > 0 {
            let reason = if eff_stop.is_some_and(|es| current <= es) {
                stop_reason
            } else {
                format!("공통 익절 (+{}%)", protections::fmt_g(risk.take_profit_pct))
            };
            let cid = client_order_id(&bot_id, today, 901, &format!("protmkt:{peak:.2}"));
            let r = send_market(state, bot, "SELL", qty, current, &cid).await?;
            bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                "kind": "order",
                "id": r["orderId"],
                "side": "SELL",
                "qty": qty,
                "date": today,
                "synced": 0.0,
            }));
            bots::log(state, bot, "order", &format!("[보호막] {reason} 즉시 시장가 매도 {qty}주"), "info", None).await;
        }
        return Ok(());
    }

    let existing = bot["state"]["live"]["pending"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|p| {
                    p.get("kind").and_then(|v| v.as_str()) == Some("conditional")
                        && p.get("role").and_then(|v| v.as_str()) == Some("protection")
                })
                .cloned()
        });
    let sl_rounded = eff_stop.map(|es| round_price(&m, es, "down"));
    let tp_rounded = tp_level.map(|tp| round_price(&m, tp, "up"));

    if let Some(ex) = existing {
        let prev_sl = ex.get("sl_trigger").and_then(|v| v.as_f64());
        if let (Some(slr), Some(psl)) = (sl_rounded, prev_sl) {
            if slr >= psl * 1.005 {
                cancel_protection_conditionals(state, bot).await;
                qty = pos_qty.min(fetch_sellable().await?) as i64;
            } else {
                return Ok(());
            }
        } else {
            return Ok(());
        }
    }
    if qty <= 0 {
        return Ok(());
    }

    let exp_date = NaiveDate::parse_from_str(today, "%Y-%m-%d")
        .map(|d| (d + Duration::days(30)).to_string())
        .unwrap_or_else(|_| today.to_string());
    trading::check_limits(state, &uid, market, (qty as f64) * current)?;
    let cid = client_order_id(
        &bot_id,
        today,
        902,
        &format!(
            "prot:{}:{}:{qty}",
            sl_rounded.unwrap_or(0.0),
            tp_rounded.unwrap_or(0.0)
        ),
    );

    if let (Some(slr), Some(tpr)) = (sl_rounded, tp_rounded) {
        if tpr > current && current > slr {
            let sl_order_px = round_price(&m, slr * 0.99, "down");
            let high_val = m == "KR" && (qty as f64) * current >= HIGH_VALUE_KRW;
            let mut payload = json!({
                "clientOrderId": cid,
                "symbol": sym,
                "type": "OCO",
                "quantity": qty.to_string(),
                "orderType": "LIMIT",
                "expireDate": exp_date,
                "first": { "orderSide": "SELL", "triggerPrice": fmt_price(&m, tpr), "orderPrice": fmt_price(&m, tpr) },
                "second": { "orderSide": "SELL", "triggerPrice": fmt_price(&m, slr), "orderPrice": fmt_price(&m, sl_order_px) },
            });
            if high_val {
                payload["confirmHighValueOrder"] = json!(true);
            }
            let res = normalize(
                client
                    .request("POST", "/api/v1/conditional-orders", None, Some(&payload), Some(seq))
                    .await?
                    .unwrap_or_else(|| json!({})),
                "",
            );
            state.store.put(
                "orders",
                &cid,
                json!({
                    "user_id": uid,
                    "kind": "conditional_oco",
                    "source": format!("bot:{bot_id}"),
                    "market": m,
                    "symbol": sym,
                    "side": "SELL",
                    "payload": payload,
                    "result": res,
                    "date": trading::kst_today(),
                    "created_at": now_iso(),
                }),
            );
            bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                "kind": "conditional",
                "role": "protection",
                "id": res["conditionalOrderId"],
                "side": "SELL",
                "qty": qty,
                "date": today,
                "synced": 0.0,
                "sl_trigger": slr,
                "tp_trigger": tpr,
            }));
            bots::log(
                state,
                bot,
                "order",
                &format!(
                    "[보호막 OCO] 익절 {} / 손절 {} 자동 설정 ({qty}주)",
                    fmt_price(&m, tpr),
                    fmt_price(&m, slr)
                ),
                "info",
                None,
            )
            .await;
            return Ok(());
        }
    }

    if let Some(slr) = sl_rounded {
        if current > slr {
            let res = send_conditional(state, bot, "SELL", qty, slr, &exp_date, &cid).await?;
            bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                "kind": "conditional",
                "role": "protection",
                "id": res["conditionalOrderId"],
                "side": "SELL",
                "qty": qty,
                "date": today,
                "synced": 0.0,
                "sl_trigger": slr,
            }));
            bots::log(
                state,
                bot,
                "order",
                &format!("[보호막] {stop_reason} 조건주문 자동 설정 ({}, {qty}주)", fmt_price(&m, slr)),
                "info",
                None,
            )
            .await;
            return Ok(());
        }
    }

    if let Some(tpr) = tp_rounded {
        if tpr > current {
            let res = send_conditional(state, bot, "SELL", qty, tpr, &exp_date, &cid).await?;
            bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                "kind": "conditional",
                "role": "protection",
                "id": res["conditionalOrderId"],
                "side": "SELL",
                "qty": qty,
                "date": today,
                "synced": 0.0,
                "tp_trigger": tpr,
            }));
            bots::log(
                state,
                bot,
                "order",
                &format!("[보호막] 공통 익절 조건주문 자동 설정 ({}, {qty}주)", fmt_price(&m, tpr)),
                "info",
                None,
            )
            .await;
        }
    }
    Ok(())
}

async fn wait_sell_fill(state: &AppState, bot: &mut Value, today: &str, order_id: &str) {
    for i in 0..SELL_SETTLE_POLLS {
        if i > 0 {
            let wait_ms = if state.toss_override.is_some() {
                0
            } else {
                SELL_SETTLE_INTERVAL_MS
            };
            if wait_ms > 0 {
                tokio::time::sleep(StdDuration::from_millis(wait_ms)).await;
            }
        }
        let _ = reconcile_live(state, bot, today).await;
        let still = bot["state"]["live"]["pending"]
            .as_array()
            .is_some_and(|a| {
                a.iter().any(|p| {
                    p.get("kind").and_then(|v| v.as_str()) == Some("order")
                        && p.get("id").and_then(|v| v.as_str()) == Some(order_id)
                })
            });
        if !still {
            return;
        }
    }
}

fn unsynced_sell_qty(st: &Value) -> f64 {
    st["pending"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| {
                    p.get("kind").and_then(|v| v.as_str()) == Some("order")
                        && p.get("side").and_then(|v| v.as_str()) == Some("SELL")
                })
                .map(|p| {
                    (p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0)
                        - p.get("synced").and_then(|v| v.as_f64()).unwrap_or(0.0))
                    .max(0.0)
                })
                .sum()
        })
        .unwrap_or(0.0)
}

pub async fn run_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["live"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let _ = reconcile_live(state, bot, today).await;
    let (_, bars, today_bar_opt, mut strat) = load(state, bot, today).await?;
    let today_bar = today_bar_opt.ok_or_else(|| {
        ApiError::conflict("market_not_open", "오늘 시가가 아직 없습니다")
    })?;
    let d = decide_with_loaded(state, bot, today, &bars, Some(today_bar.clone()), strat.as_mut(), false).await;

    // Mark day done BEFORE sending anything
    bot["state"]["live"]["last_run_date"] = json!(today);
    bots::save(state, bot);

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let sym = bot["symbol"].as_str().unwrap_or("").to_string();
    let market = Market::parse(&m)?;
    let currency = if m == "KR" { "KRW" } else { "USD" };
    let (client, seq) = account_client(state, &uid)?;

    let mut pp = HashMap::new();
    pp.insert("symbols".to_string(), sym.clone());
    let current = client
        .request("GET", "/api/v1/prices", Some(&pp), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .and_then(|a| a.first().cloned())
        .and_then(|r| r.get("lastPrice").cloned())
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(today_bar.close);
    let c = costs_for(state, &uid, market).await;
    let cap = bot["capital"].as_f64().unwrap_or(0.0);

    let mut expect_holding = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0;
    let est_eq_live = cap
        + bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0)
        + bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0)
            * (current - bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0));
    let buy_block = protections::check_buy_allowed(bot, &bot["state"]["live"], today, Some(est_eq_live));

    for (n, o) in d.orders.iter().enumerate() {
        if (o.side == "buy" && expect_holding) || (o.side == "sell" && !expect_holding) {
            continue;
        }
        if o.side == "buy" {
            if let Some(ref blk) = buy_block {
                bots::log(state, bot, "skip", &format!("신규 매수 보류: {blk}"), "warn", None).await;
                continue;
            }
        }
        let side = if o.side == "buy" { "BUY" } else { "SELL" };
        let level = if o.kind == "limit" {
            o.limit
        } else if o.kind == "stop" {
            Some(order_level(o, today_bar.open))
        } else {
            None
        };
        let crossed = o.kind == "stop"
            && level.is_some_and(|lv| {
                (side == "BUY" && today_bar.open.max(current) >= lv)
                    || (side == "SELL" && today_bar.open.min(current) <= lv)
            });
        let immediate = o.kind == "market" || crossed;

        let step_res: ApiResult<()> = async {
            let qty: i64 = if side == "BUY" {
                let mut bp_p = HashMap::new();
                bp_p.insert("currency".to_string(), currency.to_string());
                let bp = normalize(
                    client
                        .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                        .await?
                        .unwrap_or_else(|| json!({})),
                    "",
                );
                let unsynced = unsynced_sell_qty(&bot["state"]["live"]);
                let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
                let pep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
                let eff_qty = (pq - unsynced).max(0.0);
                let bot_budget = (cap - eff_qty * pep).max(0.0);
                let mut cash_bp = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if !immediate && unsynced > 0.0 {
                    cash_bp += unsynced * current * (1.0 - (c.commission_pct + c.tax_pct) / 100.0);
                }
                let budget = bot_budget.min(cash_bp) * d.mt_scale;
                let est = level.unwrap_or(current);
                let headroom = ((c.slippage_pct + c.commission_pct) / 100.0).max(LIVE_BUY_HEADROOM);
                let mut q = if est > 0.0 {
                    (budget / (est * (1.0 + headroom))).floor() as i64
                } else {
                    0
                };
                if let Some(oq) = o.qty {
                    q = q.min(oq as i64);
                }
                q
            } else {
                if bot["state"]["live"]["pending"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|p| p.get("role").and_then(|v| v.as_str()) == Some("protection")))
                {
                    cancel_protection_conditionals(state, bot).await;
                }
                let mut sq = HashMap::new();
                sq.insert("symbol".to_string(), sym.clone());
                let sellable = client
                    .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
                    .await?
                    .and_then(|v| v.get("sellableQuantity").cloned())
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0.0);
                let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
                pq.min(sellable) as i64
            };

            if qty <= 0 {
                bots::log(
                    state,
                    bot,
                    "skip",
                    &format!("{} 부족으로 주문하지 않았습니다", if side == "BUY" { "자금" } else { "매도 가능 수량" }),
                    "warn",
                    None,
                )
                .await;
                return Ok(());
            }

            if immediate {
                let cid = client_order_id(&bot_id, today, n, "mkt");
                let r = send_market(state, bot, side, qty, current, &cid).await?;
                let oid = r["orderId"].as_str().unwrap_or("").to_string();
                let mut rec = json!({
                    "kind": "order",
                    "id": oid,
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": current,
                });
                if side == "BUY" {
                    if let Some(a) = d.entry_atr {
                        rec["entry_atr"] = json!(a);
                    }
                }
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(rec);
                bots::log(
                    state,
                    bot,
                    "order",
                    &format!("{} 시장가 {qty}주 주문 — {}", if side == "BUY" { "매수" } else { "매도" }, o.reason),
                    "info",
                    None,
                )
                .await;
                if side == "SELL" && d.orders[n + 1..].iter().any(|nxt| nxt.side == "buy") {
                    wait_sell_fill(state, bot, today, &oid).await;
                    let still = bot["state"]["live"]["pending"]
                        .as_array()
                        .is_some_and(|a| {
                            a.iter().any(|p| p.get("kind").and_then(|v| v.as_str()) == Some("order") && p.get("id").and_then(|v| v.as_str()) == Some(oid.as_str()))
                        });
                    expect_holding = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 && !still;
                } else {
                    expect_holding = side == "BUY";
                }
            } else if o.kind == "limit" && level.is_some() {
                let lv = level.unwrap();
                let cid = client_order_id(&bot_id, today, n, "lmt");
                let r = send_limit(state, bot, side, qty, lv, &cid).await?;
                let mut rec = json!({
                    "kind": "order",
                    "id": r["orderId"],
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": lv,
                });
                if side == "BUY" {
                    if let Some(a) = d.entry_atr {
                        rec["entry_atr"] = json!(a);
                    }
                }
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(rec);
                bots::log(state, bot, "order", &format!("{} → 지정가 {qty}주 (오늘 하루 유효)", describe(o, Some(today_bar.open))), "info", None).await;
            } else if let Some(lv) = level {
                let cid = client_order_id(&bot_id, today, n, "cond");
                let r = send_conditional(state, bot, side, qty, lv, today, &cid).await?;
                let mut rec = json!({
                    "kind": "conditional",
                    "id": r["conditionalOrderId"],
                    "side": side,
                    "qty": qty,
                    "date": today,
                    "synced": 0.0,
                    "signal_price": lv,
                });
                if side == "BUY" {
                    if let Some(a) = d.entry_atr {
                        rec["entry_atr"] = json!(a);
                    }
                }
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(rec);
                bots::log(state, bot, "order", &format!("{} → 조건주문 {qty}주 (오늘 하루 유효)", describe(o, Some(today_bar.open))), "info", None).await;
            }
            Ok(())
        }
        .await;

        if let Err(e) = step_res {
            bots::log(
                state,
                bot,
                "error",
                &format!("주문 실패: {}", e.message),
                "error",
                Some(json!({ "code": e.code })),
            )
            .await;
        }
    }

    if d.orders.is_empty() {
        bots::log(state, bot, "decision", &format!("{} 종가 기준 신호 없음", d.bar_date), "info", None).await;
    }
    if bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 && bot_risk(bot).enabled() {
        if let Err(e) = sync_live_protection(state, bot, today, Some(current)).await {
            bots::log(state, bot, "error", &format!("보호막 주문 설정 실패: {}", e.message), "warn", None).await;
        }
    }
    bot["state"]["live"]["last_price"] = json!(current);
    let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let pep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    bot["state"]["live"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": cap + realized + pq * (current - pep) }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today }))
}

pub async fn run(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if is_rebalance(bot) {
        return rebalance::run(state, bot, today).await;
    }
    if is_rotation(bot) {
        return rotation::run(state, bot, today).await;
    }
    if is_dca(bot) {
        return dca::run(state, bot, today).await;
    }
    if bot["mode"].as_str() == Some("paper") {
        run_paper(state, bot, today).await
    } else {
        run_live(state, bot, today).await
    }
}

// ------------------------------------------------------------------ intraday daytrade
pub fn record_day_equity(st: &mut Value, today: &str, value: f64) {
    let arr = st["equity"].as_array_mut().unwrap();
    if let Some(last) = arr.last_mut() {
        if last.get("date").and_then(|v| v.as_str()) == Some(today) {
            last["value"] = json!(value);
            return;
        }
    }
    arr.push(json!({ "date": today, "value": value }));
}

pub async fn run_intraday(
    state: &AppState,
    bot: &mut Value,
    today: &str,
    now: DateTime<Utc>,
    session_end: DateTime<Utc>,
) -> ApiResult<Value> {
    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();
    if bot["state"][&sk].get("intraday_closed_date").and_then(|v| v.as_str()) == Some(today) {
        return Ok(json!({ "skipped": "closed_for_day" }));
    }
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let sym = bot["symbol"].as_str().unwrap_or("").to_string();
    let market = Market::parse(&m)?;
    let c = costs_for(state, &uid, market).await;

    // Mandatory EOD flatten before close
    if now >= session_end - Duration::minutes(EOD_FLATTEN_MINS) {
        if mode == "paper" {
            bot["state"]["paper"]["pending"] = json!([]);
            if bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0 {
                let tf = bots::bot_timeframe(bot);
                let interval = Interval::parse(tf)?;
                let px = market::get_candles(state, &uid, market, &sym, interval, 5)
                    .await
                    .ok()
                    .and_then(|(_, cs)| cs.last().map(|x| x.close))
                    .or_else(|| bot["state"]["paper"].get("last_price").and_then(|v| v.as_f64()))
                    .unwrap_or_else(|| bot["state"]["paper"]["position"]["entry_price"].as_f64().unwrap_or(0.0));
                paper_sell(&mut bot["state"]["paper"], px, c, today, "장 마감 전 당일 강제 청산");
                if let Some(obj) = bot["state"]["paper"].as_object_mut() {
                    obj.remove("entry_bar_time");
                }
                bot["state"]["paper"]["last_price"] = json!(px);
                bots::log(state, bot, "fill", &format!("[모의] 장 마감 전 당일 강제 청산 @ {px:.2}"), "info", None).await;
            }
            let eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
                + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0)
                    * bot["state"]["paper"].get("last_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
            record_day_equity(&mut bot["state"]["paper"], today, eq);
        } else {
            let _ = reconcile_live(state, bot, today).await;
            if bot["state"]["live"]["pending"].as_array().is_some_and(|a| !a.is_empty()) {
                let _ = cancel_live_pending(state, bot).await;
                bot["state"]["live"]["pending"] = json!([]);
            }
            let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
            if pq > 0.0 {
                let (client, seq) = account_client(state, &uid)?;
                let mut sq = HashMap::new();
                sq.insert("symbol".to_string(), sym.clone());
                let sellable = client
                    .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
                    .await?
                    .and_then(|v| v.get("sellableQuantity").cloned())
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0.0);
                let qty = pq.min(sellable) as i64;
                if qty > 0 {
                    let mut pp = HashMap::new();
                    pp.insert("symbols".to_string(), sym.clone());
                    let cur = client
                        .request("GET", "/api/v1/prices", Some(&pp), None, None)
                        .await?
                        .and_then(|v| v.as_array().cloned())
                        .and_then(|a| a.first().cloned())
                        .and_then(|r| r.get("lastPrice").cloned())
                        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                        .unwrap_or_else(|| bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0));
                    let cid = client_order_id(&bot_id, today, 999, "eod");
                    let r = send_market(state, bot, "SELL", qty, cur, &cid).await?;
                    bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                        "kind": "order",
                        "id": r["orderId"],
                        "side": "SELL",
                        "qty": qty,
                        "date": today,
                        "synced": 0.0,
                    }));
                    bots::log(state, bot, "order", &format!("장 마감 전 당일 강제 청산 시장가 {qty}주"), "info", None).await;
                }
            }
        }
        bot["state"][&sk]["intraday_closed_date"] = json!(today);
        bot["state"][&sk]["last_run_date"] = json!(today);
        bots::save(state, bot);
        return Ok(json!({ "flattened": today }));
    }

    let (candles, bars, latest_bar, mut strat) = load_intraday(state, bot).await?;
    let bar_time = *bars.time.last().unwrap_or(&0);
    if bot["state"][&sk].get("last_bar_time").and_then(|v| v.as_i64()) == Some(bar_time) {
        return Ok(json!({ "skipped": "already_ran_bar" }));
    }

    if mode == "paper" {
        let last_done = &candles[candles.len() - 2];
        settle_paper_risk_on_bar(state, bot, last_done, today, c).await;
        let pend = bot["state"]["paper"]["pending"].as_array().cloned().unwrap_or_default();
        for p in pend {
            let lvl = p["level"].as_f64().unwrap_or(0.0);
            let holding = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0;
            let pk = p.get("kind").and_then(|v| v.as_str()).unwrap_or("stop");
            let side = p["side"].as_str().unwrap_or("");
            let p_qty = p.get("qty").and_then(|v| v.as_f64());
            let reason = p.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            if pk == "limit" {
                if side == "buy" && !holding && last_done.low <= lvl {
                    let fill = last_done.open.min(lvl);
                    let qty = paper_qty(&bot["state"]["paper"], p_qty, fill, c, 1.0);
                    if qty > 0 {
                        paper_buy(&mut bot["state"]["paper"], qty, fill, c, today, reason, None, None);
                        bot["state"]["paper"]["entry_bar_time"] = json!(bar_time);
                        bots::log(state, bot, "fill", &format!("[모의/분봉] 지정가 매수 {qty}주 @ {fill:.2}"), "info", None).await;
                    }
                } else if side == "sell" && holding && last_done.high >= lvl {
                    let fill = last_done.open.max(lvl);
                    paper_sell(&mut bot["state"]["paper"], fill, c, today, reason);
                    if let Some(obj) = bot["state"]["paper"].as_object_mut() { obj.remove("entry_bar_time"); }
                    bots::log(state, bot, "fill", &format!("[모의/분봉] 지정가 매도 @ {fill:.2}"), "info", None).await;
                }
            } else if side == "buy" && !holding && last_done.high >= lvl {
                let fill = last_done.open.max(lvl);
                let qty = paper_qty(&bot["state"]["paper"], p_qty, fill, c, 1.0);
                if qty > 0 {
                    paper_buy(&mut bot["state"]["paper"], qty, fill, c, today, reason, None, None);
                    bot["state"]["paper"]["entry_bar_time"] = json!(bar_time);
                    bots::log(state, bot, "fill", &format!("[모의/분봉] 돌파 매수 {qty}주 @ {fill:.2}"), "info", None).await;
                }
            } else if side == "sell" && holding && last_done.low <= lvl {
                let fill = last_done.open.min(lvl);
                paper_sell(&mut bot["state"]["paper"], fill, c, today, reason);
                if let Some(obj) = bot["state"]["paper"].as_object_mut() { obj.remove("entry_bar_time"); }
                bots::log(state, bot, "fill", &format!("[모의/분봉] 스톱 매도 @ {fill:.2}"), "info", None).await;
            }
        }
        bot["state"]["paper"]["pending"] = json!([]);
        let d = decide_with_loaded(state, bot, today, &bars, Some(latest_bar.clone()), strat.as_mut(), true).await;
        let px = latest_bar.open;
        let est_eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
            + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * px;
        let buy_block = protections::check_buy_allowed(bot, &bot["state"]["paper"], today, Some(est_eq));
        for o in &d.orders {
            let holding = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) > 0.0;
            if (o.side == "buy" && holding) || (o.side == "sell" && !holding) {
                continue;
            }
            if o.side == "buy" && buy_block.is_some() {
                continue;
            }
            if o.kind == "market" {
                if o.side == "buy" {
                    let qty = paper_qty(&bot["state"]["paper"], o.qty, px, c, 1.0);
                    if qty > 0 {
                        paper_buy(&mut bot["state"]["paper"], qty, px, c, today, &o.reason, None, None);
                        bot["state"]["paper"]["entry_bar_time"] = json!(bar_time);
                        bots::log(state, bot, "fill", &format!("[모의/분봉] 매수 {qty}주 @ {px:.2} — {}", o.reason), "info", None).await;
                    }
                } else {
                    paper_sell(&mut bot["state"]["paper"], px, c, today, &o.reason);
                    if let Some(obj) = bot["state"]["paper"].as_object_mut() { obj.remove("entry_bar_time"); }
                    bots::log(state, bot, "fill", &format!("[모의/분봉] 매도 @ {px:.2} — {}", o.reason), "info", None).await;
                }
            } else {
                let lvl = if o.kind == "limit" { o.limit.unwrap_or(px) } else { order_level(o, px) };
                bot["state"]["paper"]["pending"].as_array_mut().unwrap().push(json!({
                    "active_date": today,
                    "kind": o.kind,
                    "side": o.side,
                    "level": lvl,
                    "qty": o.qty,
                    "reason": o.reason,
                }));
            }
        }
        bot["state"]["paper"]["last_bar_time"] = json!(bar_time);
        bot["state"]["paper"]["last_run_date"] = json!(today);
        bot["state"]["paper"]["last_price"] = json!(latest_bar.close);
        let eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
            + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * latest_bar.close;
        record_day_equity(&mut bot["state"]["paper"], today, eq);
        protections::evaluate_after_step(state, bot, today).await;
        bots::save(state, bot);
        return Ok(json!({ "ran_bar": bar_time }));
    }

    // Live intraday execution
    let _ = reconcile_live(state, bot, today).await;
    let d = decide_with_loaded(state, bot, today, &bars, Some(latest_bar.clone()), strat.as_mut(), true).await;
    bot["state"]["live"]["last_bar_time"] = json!(bar_time);
    bot["state"]["live"]["last_run_date"] = json!(today);
    bots::save(state, bot);

    let (client, seq) = account_client(state, &uid)?;
    let currency = if m == "KR" { "KRW" } else { "USD" };
    let mut pp = HashMap::new();
    pp.insert("symbols".to_string(), sym.clone());
    let current = client
        .request("GET", "/api/v1/prices", Some(&pp), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .and_then(|a| a.first().cloned())
        .and_then(|r| r.get("lastPrice").cloned())
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(latest_bar.close);

    if !d.orders.is_empty()
        && bot["state"]["live"]["pending"]
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p.get("kind").and_then(|v| v.as_str()) == Some("conditional")))
    {
        let pend = bot["state"]["live"]["pending"].as_array().cloned().unwrap_or_default();
        let mut keep = Vec::new();
        for p in pend {
            if p.get("kind").and_then(|v| v.as_str()) == Some("conditional") {
                let pid = p["id"].as_str().unwrap_or("");
                if client
                    .request("DELETE", &format!("/api/v1/conditional-orders/{pid}"), None, None, Some(seq))
                    .await
                    .is_err()
                {
                    keep.push(p);
                }
            } else {
                keep.push(p);
            }
        }
        bot["state"]["live"]["pending"] = Value::Array(keep);
    }

    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    for (n, o) in d.orders.iter().enumerate() {
        let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
        let pep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
        let holding = pq > 0.0;
        if (o.side == "buy" && holding) || (o.side == "sell" && !holding) {
            continue;
        }
        if o.side == "buy" {
            let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let eq = cap + realized + pq * (current - pep);
            if let Some(blk) = protections::check_buy_allowed(bot, &bot["state"]["live"], today, Some(eq)) {
                bots::log(state, bot, "skip", &format!("[분봉] 신규 매수 보류: {blk}"), "warn", None).await;
                continue;
            }
        }
        let side = if o.side == "buy" { "BUY" } else { "SELL" };
        let level = if o.kind == "limit" {
            o.limit
        } else if o.kind == "stop" {
            Some(order_level(o, latest_bar.open))
        } else {
            None
        };
        let crossed = o.kind == "stop"
            && level.is_some_and(|lv| (side == "BUY" && current >= lv) || (side == "SELL" && current <= lv));
        let res: ApiResult<()> = async {
            let qty: i64 = if side == "BUY" {
                let mut bp_p = HashMap::new();
                bp_p.insert("currency".to_string(), currency.to_string());
                let bp = normalize(
                    client
                        .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                        .await?
                        .unwrap_or_else(|| json!({})),
                    "",
                );
                let budget = (cap - pq * pep)
                    .max(0.0)
                    .min(bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0));
                let headroom = ((c.slippage_pct + c.commission_pct) / 100.0).max(LIVE_BUY_HEADROOM);
                let est = level.unwrap_or(current);
                let mut q = if est > 0.0 {
                    (budget / (est * (1.0 + headroom))).floor() as i64
                } else {
                    0
                };
                if let Some(oq) = o.qty {
                    q = q.min(oq as i64);
                }
                q
            } else {
                if bot["state"]["live"]["pending"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|p| p.get("role").and_then(|v| v.as_str()) == Some("protection")))
                {
                    cancel_protection_conditionals(state, bot).await;
                }
                let mut sq = HashMap::new();
                sq.insert("symbol".to_string(), sym.clone());
                let sellable = client
                    .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
                    .await?
                    .and_then(|v| v.get("sellableQuantity").cloned())
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                    .unwrap_or(0.0);
                pq.min(sellable) as i64
            };
            if qty <= 0 {
                return Ok(());
            }
            let tag_id = format!("{today}:{bar_time}");
            if o.kind == "market" || crossed {
                let r = send_market(state, bot, side, qty, current, &client_order_id(&bot_id, &tag_id, n, "imkt")).await?;
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "order", "id": r["orderId"], "side": side, "qty": qty, "date": today, "synced": 0.0
                }));
                bots::log(state, bot, "order", &format!("[분봉] {} 시장가 {qty}주 — {}", if side == "BUY" { "매수" } else { "매도" }, o.reason), "info", None).await;
            } else if o.kind == "limit" && level.is_some() {
                let lv = level.unwrap();
                let r = send_limit(state, bot, side, qty, lv, &client_order_id(&bot_id, &tag_id, n, "ilmt")).await?;
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "order", "id": r["orderId"], "side": side, "qty": qty, "date": today, "synced": 0.0
                }));
                bots::log(state, bot, "order", &format!("[분봉] {} → 지정가 {qty}주", describe(o, Some(latest_bar.open))), "info", None).await;
            } else if let Some(lv) = level {
                let r = send_conditional(state, bot, side, qty, lv, today, &client_order_id(&bot_id, &tag_id, n, "icond")).await?;
                bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                    "kind": "conditional", "id": r["conditionalOrderId"], "side": side, "qty": qty, "date": today, "synced": 0.0
                }));
                bots::log(state, bot, "order", &format!("[분봉] {} → 조건주문 {qty}주", describe(o, Some(latest_bar.open))), "info", None).await;
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            bots::log(state, bot, "error", &format!("[분봉] 주문 실패: {}", e.message), "error", Some(json!({ "code": e.code }))).await;
        }
    }

    bot["state"]["live"]["last_price"] = json!(current);
    let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let pq = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let pep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    record_day_equity(&mut bot["state"]["live"], today, cap + realized + pq * (current - pep));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran_bar": bar_time }))
}

pub async fn cancel_live_pending(state: &AppState, bot: &Value) -> ApiResult<usize> {
    let pending = bot["state"]["live"]["pending"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if pending.is_empty() {
        return Ok(0);
    }
    let uid = bot["user_id"].as_str().unwrap_or("");
    let (client, seq) = account_client(state, uid)?;
    let mut n = 0usize;
    for p in pending {
        let pid = p["id"].as_str().unwrap_or("");
        let res = if p.get("kind").and_then(|v| v.as_str()) == Some("conditional") {
            client
                .request("DELETE", &format!("/api/v1/conditional-orders/{pid}"), None, None, Some(seq))
                .await
        } else {
            client
                .request("POST", &format!("/api/v1/orders/{pid}/cancel"), None, Some(&json!({})), Some(seq))
                .await
        };
        match res {
            Ok(_) => n += 1,
            Err(e) => {
                bots::log(state, bot, "error", &format!("취소 실패: {}", e.message), "warn", None).await;
            }
        }
    }
    Ok(n)
}
