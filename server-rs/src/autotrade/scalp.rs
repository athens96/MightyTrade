//! Real-world pro scalping bot ('키움 조건검색식 + 실시간 체결강도·호가 스캘핑 봇', MT-24).
//!
//! Inspired by top Korean & global scalpers (한봉호 마하세븐 · 창원개미 · 홍인기 · Ross Cameron):
//! 1. Candidate discovery via Kiwoom HTS Condition Search (`CNSRREQ`) OR a custom leader watchlist.
//! 2. Real-time microstructure confirmation: Execution Strength (체결강도 %), orderbook spread guard,
//!    intraday VWAP support, and daily gain window (leaders only, avoiding upper-limit traps).
//! 3. Fast risk discipline: tight stop-loss (칼손절), quick take-profit, peak trailing stop,
//!    same-day re-entry ban on stopped-out symbols, max trades/day cap, golden opening window,
//!    and mandatory 10-minute pre-close full flatten.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, executor, protections};
use crate::errors::ApiResult;
use crate::integrations::kiwoom::{parse_kiwoom_abs, parse_kiwoom_signed, strip_kr_prefix};
use crate::market::{self, extras::normalize, models::{Interval, Market}};
use crate::quant::engine::Costs;
use crate::quant::indicators as ind;
use crate::quant::market_timing;
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::users::repository::now_iso;

const EOD_FLATTEN_MINS: i64 = 10;
const MAX_CANDIDATES_SCAN: usize = 5;
const MAX_SPREAD_PCT: f64 = 1.25;
const LIVE_BUY_HEADROOM: f64 = 0.005;

pub fn empty_state(capital: f64) -> Value {
    json!({
        "positions": {},
        "cash": capital,
        "realized": 0.0,
        "pending": [],
        "trades": [],
        "equity": [],
        "last_run_date": Value::Null,
        "last_prices": {},
        "last_candidates": [],
        "stopped_today_date": Value::Null,
        "stopped_today": [],
        "today_trades_date": Value::Null,
        "today_trades_count": 0,
        "last_reconcile": Value::Null,
    })
}

fn client_order_id(bot_id: &str, today: &str, sym: &str, side: &str, seq_n: i64) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:{today}:scalp:{sym}:{side}:{seq_n}"))
}

/// Computes real-time Execution Strength (체결강도 %) from recent trade prints:
/// `buy_volume / sell_volume * 100`. Clamped to `[0, 500]`.
pub fn compute_strength_from_trades(trades: &[Value], fallback: Option<f64>) -> f64 {
    let mut buy_vol = 0.0f64;
    let mut sell_vol = 0.0f64;
    for t in trades {
        let vol = t
            .get("volume")
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0)
            .abs();
        if vol <= 0.0 {
            continue;
        }
        let side = t.get("side").and_then(|v| v.as_str()).unwrap_or("BUY");
        if side.eq_ignore_ascii_case("SELL") {
            sell_vol += vol;
        } else {
            buy_vol += vol;
        }
    }
    if buy_vol <= 0.0 && sell_vol <= 0.0 {
        return fallback.unwrap_or(100.0).clamp(0.0, 500.0);
    }
    if sell_vol <= 0.0 {
        return 250.0f64.max(fallback.unwrap_or(0.0)).min(500.0);
    }
    let ratio = (buy_vol / sell_vol) * 100.0;
    ratio.clamp(0.0, 500.0)
}

/// Computes top-of-book bid/ask spread % and bid-to-ask total volume ratio.
pub fn compute_orderbook_metrics(book: &Value) -> (Option<f64>, Option<f64>) {
    let ask1 = book
        .get("asks")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("price"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .filter(|p| *p > 0.0);
    let bid1 = book
        .get("bids")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("price"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .filter(|p| *p > 0.0);
    let spread = match (ask1, bid1) {
        (Some(a), Some(b)) if a >= b => Some((a - b) / b * 100.0),
        _ => None,
    };
    let tot_ask = book
        .get("totalAskVolume")
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);
    let tot_bid = book
        .get("totalBidVolume")
        .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0.0);
    let ba_ratio = if tot_ask > 0.0 {
        Some((tot_bid / tot_ask * 100.0).round() / 100.0)
    } else {
        None
    };
    (spread, ba_ratio)
}

#[derive(Debug, Clone)]
struct RawCandidate {
    symbol: String,
    name: String,
    price_hint: Option<f64>,
    change_pct_hint: Option<f64>,
    strength_hint: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: f64,
    pub strength: f64,
    pub spread_pct: Option<f64>,
    pub bid_ask_ratio: Option<f64>,
    pub vwap: Option<f64>,
    pub above_vwap: bool,
    pub qualified: bool,
    pub reject_reason: Option<String>,
    pub score: f64,
}

impl ScoredCandidate {
    pub fn to_json(&self) -> Value {
        json!({
            "symbol": self.symbol,
            "name": self.name,
            "price": (self.price * 100.0).round() / 100.0,
            "change_pct": (self.change_pct * 100.0).round() / 100.0,
            "strength": (self.strength * 10.0).round() / 10.0,
            "spread_pct": self.spread_pct.map(|x| (x * 1000.0).round() / 1000.0),
            "bid_ask_ratio": self.bid_ask_ratio,
            "vwap": self.vwap.map(|x| (x * 100.0).round() / 100.0),
            "above_vwap": self.above_vwap,
            "qualified": self.qualified,
            "reject_reason": self.reject_reason,
            "score": (self.score * 10.0).round() / 10.0,
        })
    }
}

struct ScalpCfg {
    source: String,
    condition_seq: String,
    custom_symbols: Vec<String>,
    min_strength: f64,
    min_gain_pct: f64,
    max_gain_pct: f64,
    above_vwap: bool,
    take_profit_pct: f64,
    stop_loss_pct: f64,
    trailing_stop_pct: f64,
    max_positions: usize,
    max_trades_per_day: usize,
    entry_window_mins: i64,
}

fn cfg_of(bot: &Value) -> ScalpCfg {
    ScalpCfg {
        source: bot
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("watchlist")
            .to_string(),
        condition_seq: bot
            .get("condition_seq")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        custom_symbols: bot
            .get("custom_symbols")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_uppercase()))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        min_strength: bot.get("min_strength").and_then(|v| v.as_f64()).unwrap_or(120.0),
        min_gain_pct: bot.get("min_gain_pct").and_then(|v| v.as_f64()).unwrap_or(1.0),
        max_gain_pct: bot.get("max_gain_pct").and_then(|v| v.as_f64()).unwrap_or(22.0),
        above_vwap: bot.get("above_vwap").and_then(|v| v.as_bool()).unwrap_or(true),
        take_profit_pct: bot.get("take_profit_pct").and_then(|v| v.as_f64()).unwrap_or(2.0),
        stop_loss_pct: bot.get("stop_loss_pct").and_then(|v| v.as_f64()).unwrap_or(1.0),
        trailing_stop_pct: bot.get("trailing_stop_pct").and_then(|v| v.as_f64()).unwrap_or(0.8),
        max_positions: bot.get("max_positions").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, 5) as usize,
        max_trades_per_day: bot.get("max_trades_per_day").and_then(|v| v.as_u64()).unwrap_or(5).clamp(1, 30) as usize,
        entry_window_mins: bot.get("entry_window_mins").and_then(|v| v.as_i64()).unwrap_or(90).clamp(0, 360),
    }
}

async fn discover_raw_candidates(state: &AppState, bot: &Value, cfg: &ScalpCfg) -> ApiResult<Vec<RawCandidate>> {
    let uid = bot["user_id"].as_str().unwrap_or("");
    if cfg.source == "kiwoom_condition" {
        let kw = state.get_kiwoom_client(uid)?;
        let rows = kw.query_condition_matches(&cfg.condition_seq).await?;
        let mut out = Vec::new();
        for r in rows {
            let raw_code = r
                .get("9001")
                .or_else(|| r.get("stk_cd"))
                .or_else(|| r.get("code"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let sym = strip_kr_prefix(raw_code);
            if sym.is_empty() {
                continue;
            }
            let name = r
                .get("302")
                .or_else(|| r.get("stk_nm"))
                .or_else(|| r.get("name"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or(&sym)
                .to_string();
            let px = parse_kiwoom_abs(r.get("10").or_else(|| r.get("cur_prc"))).filter(|x| *x > 0.0);
            let chg = parse_kiwoom_signed(r.get("12").or_else(|| r.get("flu_rt")));
            let str_hint = parse_kiwoom_abs(r.get("228").or_else(|| r.get("strength"))).filter(|x| *x > 0.0);
            out.push(RawCandidate {
                symbol: sym,
                name,
                price_hint: px,
                change_pct_hint: chg,
                strength_hint: str_hint,
            });
        }
        out.truncate(MAX_CANDIDATES_SCAN);
        return Ok(out);
    }

    Ok(cfg
        .custom_symbols
        .iter()
        .take(MAX_CANDIDATES_SCAN)
        .map(|s| RawCandidate {
            symbol: s.clone(),
            name: s.clone(),
            price_hint: None,
            change_pct_hint: None,
            strength_hint: None,
        })
        .collect())
}

async fn score_candidates(state: &AppState, bot: &Value, cfg: &ScalpCfg) -> ApiResult<Vec<ScoredCandidate>> {
    let uid = bot["user_id"].as_str().unwrap_or("");
    let m_str = bot["market"].as_str().unwrap_or("KR");
    let market = Market::parse(m_str)?;
    let raw = discover_raw_candidates(state, bot, cfg).await?;
    let client_opt = state.get_broker_client(uid).ok();

    let mut out = Vec::with_capacity(raw.len());
    for cand in raw {
        // 1. Quote (price + daily change %)
        let quote_opt = market::get_quote(state, uid, market, &cand.symbol).await.ok();
        let price = quote_opt
            .as_ref()
            .map(|q| q.price)
            .filter(|p| *p > 0.0)
            .or(cand.price_hint)
            .unwrap_or(0.0);
        if price <= 0.0 {
            continue;
        }
        let change_pct = cand
            .change_pct_hint
            .or_else(|| quote_opt.as_ref().map(|q| q.change_pct))
            .unwrap_or(0.0);

        // 2. Execution strength from recent prints (/api/v1/trades) + orderbook spread (/api/v1/orderbook)
        let mut trades_vec: Vec<Value> = Vec::new();
        let mut book_val = json!({});
        if let Some(ref client) = client_opt {
            let mut tp = HashMap::new();
            tp.insert("symbol".to_string(), cand.symbol.clone());
            tp.insert("count".to_string(), "30".to_string());
            if let Ok(Some(Value::Array(arr))) = client
                .request("GET", "/api/v1/trades", Some(&tp), None, None)
                .await
            {
                trades_vec = arr;
            }
            let mut bp = HashMap::new();
            bp.insert("symbol".to_string(), cand.symbol.clone());
            if let Ok(Some(v)) = client
                .request("GET", "/api/v1/orderbook", Some(&bp), None, None)
                .await
            {
                book_val = normalize(v, "");
            }
        }
        let strength = compute_strength_from_trades(&trades_vec, cand.strength_hint);
        let (spread_pct, bid_ask_ratio) = compute_orderbook_metrics(&book_val);

        // 3. Intraday VWAP check via 5m candles
        let vwap = match market::get_candles(state, uid, market, &cand.symbol, Interval::M5, 60).await {
            Ok((_, cs)) if !cs.is_empty() => {
                let times: Vec<i64> = cs.iter().map(|c| c.time).collect();
                let highs: Vec<f64> = cs.iter().map(|c| c.high).collect();
                let lows: Vec<f64> = cs.iter().map(|c| c.low).collect();
                let closes: Vec<f64> = cs.iter().map(|c| c.close).collect();
                let vols: Vec<f64> = cs.iter().map(|c| c.volume).collect();
                ind::intraday_vwap(&times, &highs, &lows, &closes, &vols)
                    .vwap
                    .last()
                    .copied()
                    .flatten()
            }
            _ => None,
        };
        let above_vwap = vwap.is_none_or(|vw| price >= vw * 0.998);

        let mut reject: Option<String> = None;
        if strength < cfg.min_strength {
            reject = Some(format!("체결강도 부족 ({strength:.0}% < {:.0}%)", cfg.min_strength));
        } else if change_pct < cfg.min_gain_pct {
            reject = Some(format!("상승 탄력 약함 ({change_pct:+.1}% < +{:.1}%)", cfg.min_gain_pct));
        } else if change_pct > cfg.max_gain_pct {
            reject = Some(format!("과열 추격 차단 ({change_pct:+.1}% > +{:.1}%)", cfg.max_gain_pct));
        } else if cfg.above_vwap && !above_vwap {
            reject = Some("VWAP 하단 위치".to_string());
        } else if spread_pct.is_some_and(|sp| sp > MAX_SPREAD_PCT) {
            reject = Some(format!("호가 스프레드 과다 ({:.2}%)", spread_pct.unwrap()));
        }

        let score = strength * 0.65 + change_pct.max(0.0) * 6.0 + bid_ask_ratio.unwrap_or(1.0).min(3.0) * 5.0;
        out.push(ScoredCandidate {
            symbol: cand.symbol,
            name: cand.name,
            price,
            change_pct,
            strength,
            spread_pct,
            bid_ask_ratio,
            vwap,
            above_vwap,
            qualified: reject.is_none(),
            reject_reason: reject,
            score,
        });
    }

    out.sort_by(|a, b| {
        b.qualified
            .cmp(&a.qualified)
            .then_with(|| b.score.total_cmp(&a.score))
    });
    Ok(out)
}

async fn current_price(state: &AppState, uid: &str, market: Market, sym: &str, fallback: f64) -> f64 {
    if let Ok(q) = market::get_quote(state, uid, market, sym).await {
        if q.price > 0.0 {
            return q.price;
        }
    }
    fallback
}

fn check_exit_reason(cfg: &ScalpCfg, entry: f64, peak: f64, price: f64) -> Option<(String, bool)> {
    if entry <= 0.0 || price <= 0.0 {
        return None;
    }
    let ret_pct = (price / entry - 1.0) * 100.0;
    if ret_pct <= -cfg.stop_loss_pct {
        return Some((
            format!("스캘핑 칼손절 ({ret_pct:.2}% ≤ -{}%)", protections::fmt_g(cfg.stop_loss_pct)),
            true,
        ));
    }
    if ret_pct >= cfg.take_profit_pct {
        return Some((
            format!("스캘핑 목표 익절 (+{ret_pct:.2}% ≥ +{}%)", protections::fmt_g(cfg.take_profit_pct)),
            false,
        ));
    }
    if cfg.trailing_stop_pct > 0.0 && peak > entry {
        let drop_from_peak = (1.0 - price / peak) * 100.0;
        if drop_from_peak >= cfg.trailing_stop_pct {
            return Some((
                format!(
                    "스캘핑 트레일링 익절 (고점 {peak:.2} 대비 -{drop_from_peak:.2}%, 수익 {ret_pct:+.2}%)"
                ),
                ret_pct < 0.0,
            ));
        }
    }
    None
}

fn open_positions_vec(st: &Value) -> Vec<(String, f64, f64, f64, String)> {
    let mut out = Vec::new();
    if let Some(mp) = st.get("positions").and_then(|v| v.as_object()) {
        for (sym, p) in mp {
            let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
            if q > 0.0 {
                let ep = p.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let pk = p.get("peak_price").and_then(|v| v.as_f64()).unwrap_or(ep);
                let nm = p.get("name").and_then(|v| v.as_str()).unwrap_or(sym).to_string();
                out.push((sym.clone(), q, ep, pk, nm));
            }
        }
    }
    out
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    let uid = bot["user_id"].as_str().unwrap_or("");
    let m_str = bot["market"].as_str().unwrap_or("KR");
    let market = Market::parse(m_str)?;
    let cfg = cfg_of(bot);
    let st = bots::state(bot);
    let held = open_positions_vec(st);
    let held_set: HashSet<String> = held.iter().map(|x| x.0.clone()).collect();

    let mut orders: Vec<Value> = Vec::new();
    let mut last_prices = Map::new();
    let mut total_pos_val = 0.0;
    for (sym, qty, ep, pk, _) in &held {
        let px = current_price(state, uid, market, sym, *ep).await;
        last_prices.insert(sym.clone(), json!(px));
        total_pos_val += qty * px;
        let eff_pk = pk.max(px);
        if let Some((reason, _)) = check_exit_reason(&cfg, *ep, eff_pk, px) {
            orders.push(json!({
                "symbol": sym,
                "side": "sell",
                "kind": "market",
                "qty": *qty as i64,
                "price": px,
                "reason": reason,
                "text": format!("{sym} 매도 (시장가) — {reason}"),
            }));
        }
    }

    let cands = score_candidates(state, bot, &cfg).await.unwrap_or_default();
    let open_slots = cfg.max_positions.saturating_sub(held.len());
    let stopped_set: HashSet<String> = if st.get("stopped_today_date").and_then(|v| v.as_str()) == Some(today) {
        st.get("stopped_today")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    } else {
        HashSet::new()
    };

    if open_slots > 0 {
        for c in cands.iter().filter(|x| x.qualified && !held_set.contains(&x.symbol) && !stopped_set.contains(&x.symbol)).take(open_slots) {
            let reason = format!(
                "체결강도 {:.0}% · 등락 {:+.1}%{}",
                c.strength,
                c.change_pct,
                if c.vwap.is_some() { " · VWAP↑" } else { "" }
            );
            orders.push(json!({
                "symbol": c.symbol,
                "side": "buy",
                "kind": "market",
                "price": c.price,
                "reason": reason,
                "text": format!("{} ({}) 스캘핑 매수 — {reason}", c.name, c.symbol),
            }));
        }
    }

    let cash = st.get("cash").and_then(|v| v.as_f64()).unwrap_or_else(|| bot["capital"].as_f64().unwrap_or(0.0));
    let cand_json: Vec<Value> = cands.iter().map(|c| c.to_json()).collect();

    Ok(json!({
        "date": today,
        "bar_date": Utc::now().format("%Y-%m-%d %H:%M").to_string(),
        "position": { "qty": held.iter().map(|x| x.1 as i64).sum::<i64>(), "entry_price": 0.0 },
        "positions": st.get("positions").cloned().unwrap_or_else(|| json!({})),
        "cash": cash,
        "today_open": Value::Null,
        "last": total_pos_val,
        "candidates": cand_json,
        "orders": orders,
    }))
}

fn paper_scalp_buy(
    st: &mut Value,
    sym: &str,
    name: &str,
    qty: i64,
    price: f64,
    c: Costs,
    date: &str,
    reason: &str,
) {
    let px = price * (1.0 + c.slippage_pct / 100.0);
    let value = (qty as f64) * px;
    let fee = value * c.commission_pct / 100.0;
    let cash = st["cash"].as_f64().unwrap_or(0.0);
    st["cash"] = json!(cash - value - fee);
    if !st["positions"].is_object() {
        st["positions"] = json!({});
    }
    st["positions"][sym] = json!({
        "qty": qty,
        "entry_price": px,
        "peak_price": px,
        "entry_date": date,
        "name": name,
    });
    if !st["last_prices"].is_object() {
        st["last_prices"] = json!({});
    }
    st["last_prices"][sym] = json!(price);
    st["trades"].as_array_mut().unwrap().push(json!({
        "symbol": sym,
        "entry_date": date,
        "entry_price": px,
        "qty": qty,
        "entry_reason": format!("[{sym}] {reason}"),
        "cost_basis": value + fee,
        "exit_date": Value::Null,
        "exit_price": Value::Null,
        "exit_reason": "",
        "pnl": Value::Null,
        "return_pct": Value::Null,
    }));
}

fn paper_scalp_sell(
    st: &mut Value,
    sym: &str,
    price: f64,
    c: Costs,
    date: &str,
    reason: &str,
) -> f64 {
    let qty = st["positions"]
        .get(sym)
        .and_then(|p| p.get("qty"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if qty <= 0.0 {
        return 0.0;
    }
    let px = price * (1.0 - c.slippage_pct / 100.0);
    let value = qty * px;
    let net = value - value * (c.commission_pct + c.tax_pct) / 100.0;
    let cash = st["cash"].as_f64().unwrap_or(0.0);
    st["cash"] = json!(cash + net);
    let mut pnl_out = 0.0;
    if let Some(trades) = st["trades"].as_array_mut() {
        if let Some(t) = trades.iter_mut().rev().find(|t| {
            t.get("symbol").and_then(|v| v.as_str()) == Some(sym)
                && t.get("exit_date").is_none_or(|v| v.is_null())
        }) {
            let cb = t.get("cost_basis").and_then(|v| v.as_f64()).unwrap_or(value);
            pnl_out = net - cb;
            t["exit_date"] = json!(date);
            t["exit_price"] = json!(px);
            t["exit_reason"] = json!(reason);
            t["pnl"] = json!(pnl_out);
            t["return_pct"] = json!(if cb > 0.0 { (net / cb - 1.0) * 100.0 } else { 0.0 });
        }
    }
    if let Some(pm) = st["positions"].as_object_mut() {
        pm.remove(sym);
    }
    if !st["last_prices"].is_object() {
        st["last_prices"] = json!({});
    }
    st["last_prices"][sym] = json!(price);
    pnl_out
}

fn compute_scalp_equity(bot: &Value, sk: &str) -> f64 {
    let st = &bot["state"][sk];
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let positions = st.get("positions").and_then(|v| v.as_object());
    let last_prices = st.get("last_prices").and_then(|v| v.as_object());
    let mut pos_val = 0.0;
    let mut unreal = 0.0;
    if let Some(pm) = positions {
        for (sym, p) in pm {
            let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let ep = p.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let px = last_prices
                .and_then(|lp| lp.get(sym))
                .and_then(|v| v.as_f64())
                .filter(|x| *x > 0.0)
                .unwrap_or(ep);
            pos_val += q * px;
            unreal += q * (px - ep);
        }
    }
    if sk == "paper" {
        st.get("cash").and_then(|v| v.as_f64()).unwrap_or(cap) + pos_val
    } else {
        cap + st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0) + unreal
    }
}

fn mark_stopped_today(st: &mut Value, today: &str, sym: &str) {
    if st.get("stopped_today_date").and_then(|v| v.as_str()) != Some(today) {
        st["stopped_today_date"] = json!(today);
        st["stopped_today"] = json!([]);
    }
    if let Some(arr) = st["stopped_today"].as_array_mut() {
        if !arr.iter().any(|v| v.as_str() == Some(sym)) {
            arr.push(json!(sym));
        }
    }
}

fn bump_today_trades(st: &mut Value, today: &str) -> i64 {
    if st.get("today_trades_date").and_then(|v| v.as_str()) != Some(today) {
        st["today_trades_date"] = json!(today);
        st["today_trades_count"] = json!(0);
    }
    let next = st["today_trades_count"].as_i64().unwrap_or(0) + 1;
    st["today_trades_count"] = json!(next);
    next
}

pub async fn run_intraday(
    state: &AppState,
    bot: &mut Value,
    today: &str,
    now: DateTime<Utc>,
    session_start: DateTime<Utc>,
    session_end: DateTime<Utc>,
) -> ApiResult<Value> {
    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();
    if bot["state"][&sk]
        .get("intraday_closed_date")
        .and_then(|v| v.as_str())
        == Some(today)
    {
        return Ok(json!({ "skipped": "closed_for_day" }));
    }

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let market = Market::parse(&m)?;
    let cfg = cfg_of(bot);
    let c = executor::costs_for(state, &uid, market).await;

    if bot["state"][&sk]
        .get("today_trades_date")
        .and_then(|v| v.as_str())
        != Some(today)
    {
        bot["state"][&sk]["today_trades_date"] = json!(today);
        bot["state"][&sk]["today_trades_count"] = json!(0);
        bot["state"][&sk]["stopped_today_date"] = json!(today);
        bot["state"][&sk]["stopped_today"] = json!([]);
    }

    if mode == "live" {
        let _ = reconcile_live(state, bot, today).await;
    }

    // 1. Mandatory EOD flatten 10 mins before market close
    if now >= session_end - Duration::minutes(EOD_FLATTEN_MINS) {
        let held = open_positions_vec(&bot["state"][&sk]);
        for (sym, qty, ep, _, _) in held {
            let px = current_price(state, &uid, market, &sym, ep).await;
            if mode == "paper" {
                paper_scalp_sell(&mut bot["state"]["paper"], &sym, px, c, today, "장 마감 전 당일 강제 청산");
                bots::log(
                    state,
                    bot,
                    "fill",
                    &format!("[모의/스캘핑] {sym} 마감 10분 전 강제 청산 @ {px:.2}"),
                    "info",
                    None,
                )
                .await;
            } else {
                let seq_n = bump_today_trades(&mut bot["state"]["live"], today);
                let cid = client_order_id(&bot_id, today, &sym, "EOD", seq_n);
                let _ = send_live_scalp_order(state, bot, &sym, "SELL", qty as i64, px, &cid, today, "장 마감 전 강제 청산").await;
            }
        }
        let eq = compute_scalp_equity(bot, &sk);
        executor::record_day_equity(&mut bot["state"][&sk], today, eq);
        bot["state"][&sk]["intraday_closed_date"] = json!(today);
        bot["state"][&sk]["last_run_date"] = json!(today);
        bots::save(state, bot);
        return Ok(json!({ "flattened": today }));
    }

    // 2. Fast exit monitoring on open positions (stop-loss / take-profit / trailing stop)
    let held = open_positions_vec(&bot["state"][&sk]);
    let mut exits = 0usize;
    for (sym, qty, ep, pk, _) in held {
        let px = current_price(state, &uid, market, &sym, ep).await;
        if !bot["state"][&sk]["last_prices"].is_object() {
            bot["state"][&sk]["last_prices"] = json!({});
        }
        bot["state"][&sk]["last_prices"][&sym] = json!(px);
        let new_pk = pk.max(px);
        if let Some(p_obj) = bot["state"][&sk]["positions"].get_mut(&sym).and_then(|v| v.as_object_mut()) {
            p_obj.insert("peak_price".to_string(), json!(new_pk));
        }

        if let Some((reason, is_loss)) = check_exit_reason(&cfg, ep, new_pk, px) {
            exits += 1;
            if mode == "paper" {
                let pnl = paper_scalp_sell(&mut bot["state"]["paper"], &sym, px, c, today, &reason);
                if is_loss || pnl < 0.0 {
                    mark_stopped_today(&mut bot["state"]["paper"], today, &sym);
                }
                bots::log(
                    state,
                    bot,
                    "fill",
                    &format!("[모의/스캘핑] {sym} 매도 @ {px:.2} — {reason}"),
                    if is_loss { "warn" } else { "info" },
                    None,
                )
                .await;
            } else {
                let seq_n = bump_today_trades(&mut bot["state"]["live"], today);
                let cid = client_order_id(&bot_id, today, &sym, "SELL", seq_n);
                if send_live_scalp_order(state, bot, &sym, "SELL", qty as i64, px, &cid, today, &reason).await.is_ok()
                    && is_loss
                {
                    mark_stopped_today(&mut bot["state"]["live"], today, &sym);
                }
            }
        }
    }

    // 3. New entry scan if slot available, within golden entry window, under daily trade cap, and guards pass
    let cur_held = open_positions_vec(&bot["state"][&sk]);
    let open_slots = cfg.max_positions.saturating_sub(cur_held.len());
    let used_trades = bot["state"][&sk]["today_trades_count"].as_u64().unwrap_or(0) as usize;
    let in_entry_window = cfg.entry_window_mins == 0
        || now <= session_start + Duration::minutes(cfg.entry_window_mins);
    let mut entries = 0usize;

    let cands = score_candidates(state, bot, &cfg).await.unwrap_or_default();
    bot["state"][&sk]["last_candidates"] = Value::Array(cands.iter().map(|c| c.to_json()).collect());

    if open_slots > 0 && used_trades < cfg.max_trades_per_day && in_entry_window {
        let est_eq = compute_scalp_equity(bot, &sk);
        let buy_block = protections::check_buy_allowed(bot, &bot["state"][&sk], today, Some(est_eq));
        let mt_scale = check_market_timing_scale(state, bot, today).await;
        if buy_block.is_none() && mt_scale > 0.0 {
            let held_set: HashSet<String> = cur_held.iter().map(|x| x.0.clone()).collect();
            let stopped_set: HashSet<String> = bot["state"][&sk]["stopped_today"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();
            let cap = bot["capital"].as_f64().unwrap_or(0.0);
            let slot_budget = (cap / cfg.max_positions as f64) * mt_scale;

            for cand in cands
                .iter()
                .filter(|x| x.qualified && !held_set.contains(&x.symbol) && !stopped_set.contains(&x.symbol))
                .take(open_slots)
            {
                let cur_trades = bot["state"][&sk]["today_trades_count"].as_u64().unwrap_or(0) as usize;
                if cur_trades >= cfg.max_trades_per_day {
                    break;
                }
                let reason = format!(
                    "체결강도 {:.0}% · 등락 {:+.1}%{}",
                    cand.strength,
                    cand.change_pct,
                    if cand.vwap.is_some() { " · VWAP↑" } else { "" }
                );
                if mode == "paper" {
                    let cash = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                    let denom = cand.price * (1.0 + c.slippage_pct / 100.0) * (1.0 + c.commission_pct / 100.0);
                    let qty = if denom > 0.0 {
                        (cash.min(slot_budget) / denom).floor() as i64
                    } else {
                        0
                    };
                    if qty <= 0 {
                        continue;
                    }
                    paper_scalp_buy(
                        &mut bot["state"]["paper"],
                        &cand.symbol,
                        &cand.name,
                        qty,
                        cand.price,
                        c,
                        today,
                        &reason,
                    );
                    bump_today_trades(&mut bot["state"]["paper"], today);
                    entries += 1;
                    bots::log(
                        state,
                        bot,
                        "fill",
                        &format!("[모의/스캘핑] {} ({}) 매수 {qty}주 @ {:.2} — {reason}", cand.name, cand.symbol, cand.price),
                        "info",
                        None,
                    )
                    .await;
                } else {
                    let (client, seq) = account_client(state, &uid)?;
                    let currency = if m == "KR" { "KRW" } else { "USD" };
                    let mut bp_p = HashMap::new();
                    bp_p.insert("currency".to_string(), currency.to_string());
                    let bp = normalize(
                        client
                            .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                            .await?
                            .unwrap_or_else(|| json!({})),
                        "",
                    );
                    let avail = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let headroom = ((c.slippage_pct + c.commission_pct) / 100.0).max(LIVE_BUY_HEADROOM);
                    let qty = if cand.price > 0.0 {
                        (avail.min(slot_budget) / (cand.price * (1.0 + headroom))).floor() as i64
                    } else {
                        0
                    };
                    if qty <= 0 {
                        continue;
                    }
                    let seq_n = bump_today_trades(&mut bot["state"]["live"], today);
                    let cid = client_order_id(&bot_id, today, &cand.symbol, "BUY", seq_n);
                    if send_live_scalp_order(state, bot, &cand.symbol, "BUY", qty, cand.price, &cid, today, &reason).await.is_ok() {
                        entries += 1;
                    }
                }
            }
        }
    }

    let eq = compute_scalp_equity(bot, &sk);
    executor::record_day_equity(&mut bot["state"][&sk], today, eq);
    bot["state"][&sk]["last_run_date"] = json!(today);
    bot["state"][&sk]["last_scalp_at"] = json!(now_iso());
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({
        "ran": today,
        "entries": entries,
        "exits": exits,
        "candidates": cands.len(),
    }))
}

async fn check_market_timing_scale(state: &AppState, bot: &Value, today: &str) -> f64 {
    let mt = bot.get("market_timing").and_then(|v| v.as_object());
    let mode = mt.and_then(|m| m.get("mode")).and_then(|v| v.as_str()).unwrap_or("off");
    if mode == "off" {
        return 1.0;
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
    market_timing::buy_scale_for(mode, snap["regime"].as_str().unwrap_or("bull"))
}

async fn send_live_scalp_order(
    state: &AppState,
    bot: &mut Value,
    sym: &str,
    side: &str,
    qty: i64,
    signal_price: f64,
    cid: &str,
    today: &str,
    reason: &str,
) -> ApiResult<Value> {
    let m = bot["market"].as_str().unwrap_or("KR").to_string();
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
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
        "checks": { "estimated_value": (qty as f64) * signal_price.max(1.0) },
    });
    let r = trading::submit(state, &built, &uid, &format!("bot:{bot_id}")).await?;
    bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
        "kind": "order",
        "id": r["orderId"],
        "symbol": sym,
        "side": side,
        "qty": qty,
        "date": today,
        "synced": 0.0,
        "signal_price": signal_price,
        "reason": reason,
    }));
    bots::log(
        state,
        bot,
        "order",
        &format!("[스캘핑] {sym} {} 시장가 {qty}주 — {reason}", if side == "BUY" { "매수" } else { "매도" }),
        "info",
        None,
    )
    .await;
    Ok(r)
}

pub async fn reconcile_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<bool> {
    let pending = bot["state"]["live"]["pending"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if pending.is_empty() {
        return Ok(false);
    }
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let (client, seq) = account_client(state, &uid)?;
    let mut changed = false;
    let mut keep = Vec::new();

    for mut p in pending {
        let oid = p["id"].as_str().unwrap_or("").to_string();
        let sym = p["symbol"].as_str().unwrap_or("").to_string();
        let side = p["side"].as_str().unwrap_or("BUY").to_string();
        let reason = p.get("reason").and_then(|v| v.as_str()).unwrap_or("").to_string();
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
                    .or_else(|| o.get("filledQuantity").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);
                let synced = p.get("synced").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let delta = filled - synced;
                if delta > 0.0 {
                    if let Some(avg_px) = ex
                        .and_then(|e| e.get("averageFilledPrice"))
                        .and_then(|v| v.as_f64())
                        .or_else(|| o.get("averageFilledPrice").and_then(|v| v.as_f64()))
                        .or_else(|| p.get("signal_price").and_then(|v| v.as_f64()))
                        .filter(|&x| x > 0.0)
                    {
                        apply_live_scalp_fill(&mut bot["state"]["live"], &sym, &side, delta, avg_px, today, &reason);
                        bots::record_live_slippage(
                            &mut bot["state"]["live"],
                            &sym,
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
                            &format!("[스캘핑] {sym} {} 체결 {}주 @ {avg_px:.2}", if side == "BUY" { "매수" } else { "매도" }, protections::fmt_g(delta)),
                            "info",
                            None,
                        )
                        .await;
                    }
                }
                let status = o.get("status").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(status, "FILLED" | "CANCELED" | "REJECTED" | "REPLACED") {
                    changed = true;
                    continue;
                }
                keep.push(p);
            }
            Err(e) => {
                bots::log(state, bot, "error", &format!("[스캘핑] 체결 확인 실패: {}", e.message), "warn", None).await;
                keep.push(p);
            }
        }
    }
    bot["state"]["live"]["pending"] = Value::Array(keep);
    bot["state"]["live"]["last_reconcile"] = json!(now_iso());
    Ok(changed)
}

fn apply_live_scalp_fill(
    st: &mut Value,
    sym: &str,
    side: &str,
    qty: f64,
    avg: f64,
    date: &str,
    reason: &str,
) {
    if !st["positions"].is_object() {
        st["positions"] = json!({});
    }
    if side == "BUY" {
        let old_q = st["positions"].get(sym).and_then(|p| p.get("qty")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let old_ep = st["positions"].get(sym).and_then(|p| p.get("entry_price")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let new_q = old_q + qty;
        let new_ep = if new_q > 0.0 { (old_q * old_ep + qty * avg) / new_q } else { avg };
        st["positions"][sym] = json!({
            "qty": new_q,
            "entry_price": new_ep,
            "peak_price": avg.max(new_ep),
            "entry_date": date,
            "name": sym,
        });
        if old_q <= 0.0 {
            st["trades"].as_array_mut().unwrap().push(json!({
                "symbol": sym,
                "entry_date": date,
                "entry_price": avg,
                "qty": qty,
                "entry_reason": format!("[{sym}] {reason}"),
                "exit_date": Value::Null,
                "exit_price": Value::Null,
                "exit_reason": "",
                "pnl": Value::Null,
                "return_pct": Value::Null,
            }));
        }
    } else {
        let old_q = st["positions"].get(sym).and_then(|p| p.get("qty")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let old_ep = st["positions"].get(sym).and_then(|p| p.get("entry_price")).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let sold = qty.min(old_q);
        let realized = st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
        st["realized"] = json!(realized + (avg - old_ep) * sold);
        let new_q = old_q - sold;
        if new_q <= 0.0 {
            if let Some(pm) = st["positions"].as_object_mut() {
                pm.remove(sym);
            }
            if let Some(trades) = st["trades"].as_array_mut() {
                if let Some(t) = trades.iter_mut().rev().find(|t| {
                    t.get("symbol").and_then(|v| v.as_str()) == Some(sym)
                        && t.get("exit_date").is_none_or(|v| v.is_null())
                }) {
                    let tep = t["entry_price"].as_f64().unwrap_or(old_ep);
                    let tq = t["qty"].as_f64().unwrap_or(sold);
                    t["exit_date"] = json!(date);
                    t["exit_price"] = json!(avg);
                    t["exit_reason"] = json!(reason);
                    t["pnl"] = json!((avg - tep) * tq);
                    t["return_pct"] = json!(if tep > 0.0 { (avg / tep - 1.0) * 100.0 } else { 0.0 });
                }
            }
        } else if let Some(p) = st["positions"].get_mut(sym).and_then(|v| v.as_object_mut()) {
            p.insert("qty".to_string(), json!(new_q));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_execution_strength_and_orderbook_spread() {
        let trades = vec![
            json!({ "price": 72000, "volume": 150, "side": "BUY" }),
            json!({ "price": 71900, "volume": 100, "side": "SELL" }),
        ];
        assert!((compute_strength_from_trades(&trades, None) - 150.0).abs() < 1e-6);

        let book = json!({
            "asks": [{ "price": 72100.0, "volume": 500 }],
            "bids": [{ "price": 72000.0, "volume": 750 }],
            "totalAskVolume": 1000.0,
            "totalBidVolume": 1500.0
        });
        let (sp, ratio) = compute_orderbook_metrics(&book);
        assert!(sp.unwrap() < 0.2);
        assert_eq!(ratio, Some(1.5));
    }

    #[test]
    fn exit_rules_trigger_stop_tp_and_trailing() {
        let cfg = ScalpCfg {
            source: "watchlist".to_string(),
            condition_seq: "".to_string(),
            custom_symbols: vec![],
            min_strength: 120.0,
            min_gain_pct: 1.0,
            max_gain_pct: 22.0,
            above_vwap: true,
            take_profit_pct: 2.0,
            stop_loss_pct: 1.0,
            trailing_stop_pct: 0.8,
            max_positions: 1,
            max_trades_per_day: 5,
            entry_window_mins: 90,
        };
        assert!(check_exit_reason(&cfg, 100.0, 100.0, 98.9).unwrap().1);
        assert!(!check_exit_reason(&cfg, 100.0, 102.1, 102.1).unwrap().1);
        // Peak 101.8 -> dropped to 100.9 (-0.88% from peak >= 0.8% trailing)
        let tr = check_exit_reason(&cfg, 100.0, 101.8, 100.9).unwrap();
        assert!(tr.0.contains("트레일링"));
        assert!(!tr.1);
    }
}
