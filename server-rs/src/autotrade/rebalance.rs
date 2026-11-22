//! Multi-asset portfolio rebalancing bot executor (paper & live).

use chrono::Utc;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, protections};
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras::normalize, models::{Candle, Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{Bars, Costs};
use crate::quant::portfolio::{self, AssetItem, PortfolioModelSpec};
use crate::quant::strategies::ResolvedParams;
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::users::repository::now_iso;

const LIVE_BUY_HEADROOM: f64 = 0.005;
const SELL_SETTLE_POLLS: usize = 5;
const SELL_SETTLE_INTERVAL_MS: u64 = 500;

fn is_terminal(status: &str) -> bool {
    matches!(status, "FILLED" | "CANCELED" | "REJECTED" | "REPLACED")
}

fn client_order_id(bot_id: &str, today: &str, symbol: &str, side: &str) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:{today}:rebal:{symbol}:{side}"))
}

fn parse_assets(bot: &Value) -> ApiResult<Vec<AssetItem>> {
    serde_json::from_value(bot["assets"].clone())
        .map_err(|e| ApiError::bad_request("invalid_assets", e.to_string()))
}

struct LoadedRebalance {
    times: Vec<i64>,
    closes: HashMap<String, Vec<f64>>,
    today_bars: HashMap<String, Candle>,
    model: &'static PortfolioModelSpec,
    params: ResolvedParams,
    assets: Vec<AssetItem>,
}

async fn load(state: &AppState, bot: &Value, today: &str) -> ApiResult<LoadedRebalance> {
    let model_id = bot["model"].as_str().unwrap_or("");
    let model = portfolio::get_model(model_id)
        .ok_or_else(|| ApiError::bad_request("unknown_model", "알 수 없는 리밸런싱 이론입니다"))?;
    let params = model
        .resolve(bot.get("params"))
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let uid = bot["user_id"].as_str().unwrap_or("");
    let assets = parse_assets(bot)?;
    let warm = (model.warmup_fn)(&params);
    let start = Utc::now().timestamp() - ((warm as f64 * 1.6 + 45.0) * 86400.0) as i64;

    let mut past_bars: HashMap<String, Bars> = HashMap::new();
    let mut today_bars: HashMap<String, Candle> = HashMap::new();
    for a in &assets {
        let candles =
            market::get_history(state, uid, market, &a.symbol, Interval::D1, start).await?;
        let past: Vec<Candle> = candles
            .iter()
            .filter(|c| portfolio::day_str(c.time).as_str() < today)
            .cloned()
            .collect();
        let tb = candles
            .iter()
            .find(|c| portfolio::day_str(c.time).as_str() == today)
            .cloned();
        if past.len() < warm + 1 {
            return Err(ApiError::bad_request(
                "not_enough_data",
                format!("{}: 포트폴리오 계산에 필요한 과거 데이터가 부족합니다", a.symbol),
            ));
        }
        past_bars.insert(a.symbol.clone(), Bars::from_candles(&past));
        if let Some(t) = tb {
            today_bars.insert(a.symbol.clone(), t);
        }
    }
    let (times, _, closes) = portfolio::align_bars(&past_bars);
    if times.len() < warm + 1 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "자산 간 공통 거래일이 부족합니다",
        ));
    }
    Ok(LoadedRebalance {
        times,
        closes,
        today_bars,
        model,
        params,
        assets,
    })
}

struct RebalanceDecision {
    date: String,
    bar_date: String,
    due: bool,
    reason: Option<String>,
    total_value: f64,
    last_prices: HashMap<String, f64>,
    open_prices: HashMap<String, f64>,
    current_weights: Map<String, Value>,
    target_weights: Map<String, Value>,
    target_weights_raw: HashMap<String, f64>,
    orders: Vec<Value>,
}

async fn decide_internal(
    state: &AppState,
    bot: &Value,
    today: &str,
    loaded: &LoadedRebalance,
) -> ApiResult<RebalanceDecision> {
    let st = bots::state(bot);
    let uid = bot["user_id"].as_str().unwrap_or("");
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let syms: Vec<String> = loaded.assets.iter().map(|a| a.symbol.clone()).collect();
    let positions = st.get("positions").and_then(|v| v.as_object());

    let mut last_prices = HashMap::new();
    let mut open_prices = HashMap::new();
    for s in &syms {
        let lp = loaded
            .today_bars
            .get(s)
            .map(|c| c.close)
            .or_else(|| loaded.closes.get(s).and_then(|v| v.last().copied()))
            .unwrap_or(0.0);
        let op = loaded.today_bars.get(s).map(|c| c.open).unwrap_or(lp);
        last_prices.insert(s.clone(), lp);
        open_prices.insert(s.clone(), op);
    }

    let pos_qty = |s: &str| -> i64 {
        positions
            .and_then(|m| m.get(s))
            .and_then(|p| p.get("qty"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64
    };
    let pos_val: f64 = syms
        .iter()
        .map(|s| (pos_qty(s) as f64) * last_prices[s])
        .sum();
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let total_val = if bot["mode"].as_str() == Some("paper") {
        st.get("cash").and_then(|v| v.as_f64()).unwrap_or(cap) + pos_val
    } else {
        cap.max(pos_val)
    };

    let mut cur_w = HashMap::new();
    for s in &syms {
        let w = if total_val > 0.0 {
            (pos_qty(s) as f64) * last_prices[s] / total_val
        } else {
            0.0
        };
        cur_w.insert(s.clone(), w);
    }
    let tgt_w = (loaded.model.weight_fn)(
        &loaded.closes,
        loaded.times.len() - 1,
        &loaded.assets,
        &loaded.params,
    );
    let reason = portfolio::should_rebalance(
        today,
        st.get("last_rebalance_date").and_then(|v| v.as_str()),
        bot["rebalance"].as_str().unwrap_or("monthly"),
        &cur_w,
        &tgt_w,
        bot.get("drift_pct").and_then(|v| v.as_f64()).unwrap_or(5.0),
    );

    let d_cost = cost_defaults(state, uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);
    let buy_only = bot.get("buy_only").and_then(|v| v.as_bool()).unwrap_or(false);

    let mut sells = Vec::new();
    let mut buys = Vec::new();
    if buy_only {
        let avail_cash = if bot["mode"].as_str() == Some("paper") {
            st.get("cash").and_then(|v| v.as_f64()).unwrap_or(0.0)
        } else {
            (cap - pos_val).max(0.0)
        };
        let tot_after = pos_val + avail_cash;
        let mut deficits = HashMap::new();
        let mut sum_def = 0.0;
        for s in &syms {
            let w = tgt_w.get(s).copied().unwrap_or(0.0);
            let def = (tot_after * w - (pos_qty(s) as f64) * open_prices[s]).max(0.0);
            deficits.insert(s.clone(), def);
            sum_def += def;
        }
        if sum_def > 0.0 && avail_cash > 0.0 {
            for s in &syms {
                let px = open_prices[s];
                let alloc = avail_cash * (deficits[s] / sum_def);
                let add_q = if px > 0.0 && alloc > 0.0 {
                    (alloc / (px * (1.0 + headroom))).floor() as i64
                } else {
                    0
                };
                if add_q > 0 {
                    let cw = cur_w[s] * 100.0;
                    let tw = tgt_w.get(s).copied().unwrap_or(0.0) * 100.0;
                    buys.push(json!({
                        "symbol": s,
                        "side": "buy",
                        "kind": "market",
                        "qty": add_q,
                        "price": px,
                        "current_pct": (cw * 10.0).round() / 10.0,
                        "target_pct": (tw * 10.0).round() / 10.0,
                        "text": format!("{s} 무매도 부족분 매수 {add_q}주 (현재 {cw:.1}% → 목표 {tw:.1}%)"),
                    }));
                }
            }
        }
    } else {
        for s in &syms {
            let cur_q = pos_qty(s);
            let w = tgt_w.get(s).copied().unwrap_or(0.0);
            let px = open_prices[s];
            let tgt_q = if w > 0.0 && px > 0.0 {
                (total_val * w / (px * (1.0 + headroom))).floor() as i64
            } else {
                0
            };
            let diff = tgt_q - cur_q;
            let cw = cur_w[s] * 100.0;
            let tw = w * 100.0;
            if diff < 0 {
                sells.push(json!({
                    "symbol": s,
                    "side": "sell",
                    "kind": "market",
                    "qty": -diff,
                    "price": px,
                    "current_pct": (cw * 10.0).round() / 10.0,
                    "target_pct": (tw * 10.0).round() / 10.0,
                    "text": format!("{s} 매도 {}주 (현재 {cw:.1}% → 목표 {tw:.1}%)", -diff),
                }));
            } else if diff > 0 {
                buys.push(json!({
                    "symbol": s,
                    "side": "buy",
                    "kind": "market",
                    "qty": diff,
                    "price": px,
                    "current_pct": (cw * 10.0).round() / 10.0,
                    "target_pct": (tw * 10.0).round() / 10.0,
                    "text": format!("{s} 매수 {diff}주 (현재 {cw:.1}% → 목표 {tw:.1}%)"),
                }));
            }
        }
    }

    let mut cw_map = Map::new();
    let mut tw_map = Map::new();
    for s in &syms {
        cw_map.insert(s.clone(), json!((cur_w[s] * 10000.0).round() / 100.0));
        tw_map.insert(
            s.clone(),
            json!((tgt_w.get(s).copied().unwrap_or(0.0) * 10000.0).round() / 100.0),
        );
    }

    sells.extend(buys);
    Ok(RebalanceDecision {
        date: today.to_string(),
        bar_date: portfolio::day_str(*loaded.times.last().unwrap_or(&0)),
        due: reason.is_some(),
        reason,
        total_value: total_val,
        last_prices,
        open_prices,
        current_weights: cw_map,
        target_weights: tw_map,
        target_weights_raw: tgt_w,
        orders: sells,
    })
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    let loaded = load(state, bot, today).await?;
    let d = decide_internal(state, bot, today, &loaded).await?;
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
        "bar_date": d.bar_date,
        "due": d.due,
        "reason": d.reason,
        "position": { "qty": tot_qty, "entry_price": 0 },
        "cash": st.get("cash").cloned().unwrap_or_else(|| bot["capital"].clone()),
        "today_open": Value::Null,
        "last": d.total_value,
        "current_weights": d.current_weights,
        "target_weights": d.target_weights,
        "orders": if d.due { d.orders.clone() } else { vec![] },
        "planned_orders": d.orders,
    }))
}

pub async fn run_paper(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["paper"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let loaded = load(state, bot, today).await?;
    if loaded.today_bars.len() < loaded.assets.len() {
        return Err(ApiError::conflict(
            "market_not_open",
            "일부 종목의 오늘 시가가 아직 없습니다. 정규장 시작 후 실행됩니다",
        ));
    }
    let d = decide_internal(state, bot, today, &loaded).await?;
    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    let d_cost = cost_defaults(state, &uid, market).await;
    let c = Costs {
        commission_pct: d_cost["commission_pct"].as_f64().unwrap_or(0.015),
        tax_pct: d_cost["tax_pct"].as_f64().unwrap_or(0.0),
        slippage_pct: d_cost["slippage_pct"].as_f64().unwrap_or(0.05),
    };
    let (comm, tax, slip) = (
        c.commission_pct / 100.0,
        c.tax_pct / 100.0,
        c.slippage_pct / 100.0,
    );
    let syms: Vec<String> = loaded.assets.iter().map(|a| a.symbol.clone()).collect();

    if d.due {
        let buy_only = bot.get("buy_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let cash_init = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
        let pos_qty_of = |b: &Value, s: &str| -> i64 {
            b["state"]["paper"]["positions"]
                .get(s)
                .and_then(|p| p.get("qty"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i64
        };
        let val_open = cash_init
            + syms
                .iter()
                .map(|s| (pos_qty_of(bot, s) as f64) * d.open_prices[s])
                .sum::<f64>();

        let mut desired = HashMap::new();
        if buy_only {
            let mut deficits = HashMap::new();
            let mut sum_def = 0.0;
            for s in &syms {
                let w = d.target_weights_raw.get(s).copied().unwrap_or(0.0);
                let def = (val_open * w - (pos_qty_of(bot, s) as f64) * d.open_prices[s]).max(0.0);
                deficits.insert(s.clone(), def);
                sum_def += def;
            }
            for s in &syms {
                let cur_q = pos_qty_of(bot, s);
                let px_buy = d.open_prices[s] * (1.0 + slip) * (1.0 + comm);
                let alloc = if sum_def > 0.0 {
                    cash_init * deficits[s] / sum_def
                } else {
                    0.0
                };
                let add_q = if px_buy > 0.0 && alloc > 0.0 {
                    (alloc / px_buy).floor() as i64
                } else {
                    0
                };
                desired.insert(s.clone(), cur_q + add_q);
            }
        } else {
            for s in &syms {
                let w = d.target_weights_raw.get(s).copied().unwrap_or(0.0);
                let px_buy = d.open_prices[s] * (1.0 + slip) * (1.0 + comm);
                let q = if w > 0.0 && px_buy > 0.0 {
                    (val_open * w / px_buy).floor() as i64
                } else {
                    0
                };
                desired.insert(s.clone(), q);
            }
        }

        let mut n_orders = 0usize;
        // 1) Sells first
        for s in &syms {
            let cur_q = pos_qty_of(bot, s);
            let diff = desired[s] - cur_q;
            if diff < 0 {
                let qty = -diff;
                let px = d.open_prices[s] * (1.0 - slip);
                let net = (qty as f64) * px * (1.0 - comm - tax);
                let cash_now = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                bot["state"]["paper"]["cash"] = json!(cash_now + net);
                let new_q = cur_q - qty;
                if new_q <= 0 {
                    bot["state"]["paper"]["positions"].as_object_mut().unwrap().remove(s);
                } else {
                    bot["state"]["paper"]["positions"][s]["qty"] = json!(new_q);
                }
                n_orders += 1;
                bots::log(
                    state,
                    bot,
                    "fill",
                    &format!("[모의/리밸런싱] {s} 매도 {qty}주 @ {px:.2}"),
                    "info",
                    None,
                )
                .await;
            }
        }
        // 2) Buys second
        for s in &syms {
            let cur_q = pos_qty_of(bot, s);
            let diff = desired[s] - cur_q;
            if diff > 0 {
                let px = d.open_prices[s] * (1.0 + slip);
                let unit_cost = px * (1.0 + comm);
                let cash_now = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                let max_q = if unit_cost > 0.0 {
                    (cash_now / unit_cost).floor() as i64
                } else {
                    0
                };
                let qty = diff.min(max_q);
                if qty > 0 {
                    bot["state"]["paper"]["cash"] = json!(cash_now - (qty as f64) * unit_cost);
                    let old_ep = bot["state"]["paper"]["positions"]
                        .get(s)
                        .and_then(|p| p.get("entry_price"))
                        .and_then(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let new_q = cur_q + qty;
                    let avg = ((cur_q as f64) * old_ep + (qty as f64) * px) / (new_q as f64);
                    bot["state"]["paper"]["positions"][s] = json!({ "qty": new_q, "entry_price": avg });
                    n_orders += 1;
                    bots::log(
                        state,
                        bot,
                        "fill",
                        &format!("[모의/리밸런싱] {s} 매수 {qty}주 @ {px:.2}"),
                        "info",
                        None,
                    )
                    .await;
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
            &format!("[모의] {} 기준 리밸런싱 조건 미해당 (유지)", d.bar_date),
            "info",
            None,
        )
        .await;
    }

    bot["state"]["paper"]["last_run_date"] = json!(today);
    bot["state"]["paper"]["last_prices"] = json!(d.last_prices);
    bot["state"]["paper"]["target_weights"] = Value::Object(d.target_weights);
    let cash_end = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
    let eq_now = cash_end
        + syms
            .iter()
            .map(|s| {
                let q = bot["state"]["paper"]["positions"]
                    .get(s)
                    .and_then(|p| p.get("qty"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                q * d.last_prices[s]
            })
            .sum::<f64>();
    bot["state"]["paper"]["equity"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "date": today, "value": eq_now }));
    protections::evaluate_after_step(state, bot, today).await;
    bots::save(state, bot);
    Ok(json!({ "ran": today, "rebalanced": d.due }))
}

fn apply_fill(st: &mut Value, symbol: &str, side: &str, qty: f64, avg: f64) {
    let pos_obj = st["positions"].as_object_mut().unwrap();
    let old_q = pos_obj
        .get(symbol)
        .and_then(|p| p.get("qty"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let old_ep = pos_obj
        .get(symbol)
        .and_then(|p| p.get("entry_price"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if side == "BUY" {
        let new_q = old_q + qty;
        let new_ep = if new_q > 0.0 {
            (old_q * old_ep + qty * avg) / new_q
        } else {
            avg
        };
        pos_obj.insert(
            symbol.to_string(),
            json!({ "qty": new_q, "entry_price": new_ep }),
        );
    } else {
        let sold = qty.min(old_q);
        let realized = st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);
        st["realized"] = json!(realized + (avg - old_ep) * sold);
        let new_q = old_q - sold;
        if new_q <= 0.0 {
            st["positions"].as_object_mut().unwrap().remove(symbol);
        } else {
            st["positions"][symbol] = json!({ "qty": new_q, "entry_price": old_ep });
        }
    }
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
        let side = p["side"].as_str().unwrap_or("").to_string();
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
                        apply_fill(&mut bot["state"]["live"], &sym, &side, delta, avg_px);
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
                            &format!(
                                "[리밸런싱] {sym} {} 체결 {}주 @ {avg_px:.2}",
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
                        bots::log(
                            state,
                            bot,
                            "error",
                            &format!("[리밸런싱] {sym} 주문 거부됨 ({side})"),
                            "error",
                            None,
                        )
                        .await;
                    }
                    changed = true;
                    continue;
                }
                keep.push(p);
            }
            Err(e) => {
                bots::log(
                    state,
                    bot,
                    "error",
                    &format!("체결 확인 실패: {}", e.message),
                    "warn",
                    None,
                )
                .await;
                keep.push(p);
            }
        }
    }
    bot["state"]["live"]["pending"] = Value::Array(keep);
    bot["state"]["live"]["last_reconcile"] = json!(now_iso());
    Ok(changed)
}

pub async fn wait_sell_fills(state: &AppState, bot: &mut Value, today: &str, sell_ids: &HashSet<String>) {
    for i in 0..SELL_SETTLE_POLLS {
        if i > 0 {
            let wait_ms = if state.toss_override.is_some() {
                0
            } else {
                SELL_SETTLE_INTERVAL_MS
            };
            if wait_ms > 0 {
                tokio::time::sleep(Duration::from_millis(wait_ms)).await;
            }
        }
        let _ = reconcile_live(state, bot, today).await;
        let still_pending = bot["state"]["live"]["pending"]
            .as_array()
            .is_some_and(|arr| {
                arr.iter()
                    .any(|p| p.get("id").and_then(|v| v.as_str()).is_some_and(|id| sell_ids.contains(id)))
            });
        if !still_pending {
            return;
        }
    }
}

pub async fn send_market(
    state: &AppState,
    bot: &Value,
    symbol: &str,
    side: &str,
    qty: i64,
    est_price: f64,
    cid: &str,
) -> ApiResult<Value> {
    let m = bot["market"].as_str().unwrap_or("KR");
    let uid = bot["user_id"].as_str().unwrap_or("");
    let bid = bot["id"].as_str().unwrap_or("");
    let payload = json!({
        "clientOrderId": cid,
        "symbol": symbol,
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

pub async fn run_live(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["state"]["live"]["last_run_date"].as_str() == Some(today) {
        return Ok(json!({ "skipped": "already_ran" }));
    }
    let _ = reconcile_live(state, bot, today).await;
    let loaded = load(state, bot, today).await?;
    if loaded.today_bars.len() < loaded.assets.len() {
        return Err(ApiError::conflict(
            "market_not_open",
            "일부 종목의 오늘 시가가 아직 없습니다",
        ));
    }
    let d = decide_internal(state, bot, today, &loaded).await?;

    // Mark day done BEFORE sending orders
    bot["state"]["live"]["last_run_date"] = json!(today);
    bots::save(state, bot);

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let bot_id = bot["id"].as_str().unwrap_or("").to_string();
    let m_str = bot["market"].as_str().unwrap_or("KR").to_string();
    let market = Market::parse(&m_str)?;
    let currency = if m_str == "KR" { "KRW" } else { "USD" };
    let (client, seq) = account_client(state, &uid)?;
    let syms: Vec<String> = loaded.assets.iter().map(|a| a.symbol.clone()).collect();

    let mut pp = HashMap::new();
    pp.insert("symbols".to_string(), syms.join(","));
    let rows = client
        .request("GET", "/api/v1/prices", Some(&pp), None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let mut live_prices = HashMap::new();
    for r in rows {
        if let (Some(sym), Some(lp)) = (
            r.get("symbol").and_then(|v| v.as_str()),
            r.get("lastPrice").and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))),
        ) {
            live_prices.insert(sym.to_string(), lp);
        }
    }
    let mut prices = HashMap::new();
    for s in &syms {
        prices.insert(s.clone(), live_prices.get(s).copied().unwrap_or(d.last_prices[s]));
    }

    let d_cost = cost_defaults(state, &uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);
    let cap = bot["capital"].as_f64().unwrap_or(0.0);

    let pos_qty_of = |b: &Value, s: &str| -> i64 {
        b["state"]["live"]["positions"]
            .get(s)
            .and_then(|p| p.get("qty"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as i64
    };

    if d.due {
        let buy_only = bot.get("buy_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let pos_val: f64 = syms.iter().map(|s| (pos_qty_of(bot, s) as f64) * prices[s]).sum();
        let target_cap = cap.max(pos_val);
        let mut desired = HashMap::new();
        if buy_only {
            let mut bp_p = HashMap::new();
            bp_p.insert("currency".to_string(), currency.to_string());
            let bp_init = normalize(
                client
                    .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                    .await?
                    .unwrap_or_else(|| json!({})),
                "",
            );
            let avail_init = bp_init
                .get("cashBuyingPower")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                .min((cap - pos_val).max(0.0));
            let tot_after = pos_val + avail_init;
            let mut deficits = HashMap::new();
            let mut sum_def = 0.0;
            for s in &syms {
                let w = d.target_weights_raw.get(s).copied().unwrap_or(0.0);
                let def = (tot_after * w - (pos_qty_of(bot, s) as f64) * prices[s]).max(0.0);
                deficits.insert(s.clone(), def);
                sum_def += def;
            }
            for s in &syms {
                let cur_q = pos_qty_of(bot, s);
                let alloc = if sum_def > 0.0 {
                    avail_init * deficits[s] / sum_def
                } else {
                    0.0
                };
                let unit = prices[s] * (1.0 + headroom);
                let add_q = if unit > 0.0 && alloc > 0.0 {
                    (alloc / unit).floor() as i64
                } else {
                    0
                };
                desired.insert(s.clone(), cur_q + add_q);
            }
        } else {
            for s in &syms {
                let w = d.target_weights_raw.get(s).copied().unwrap_or(0.0);
                let q = if w > 0.0 && prices[s] > 0.0 {
                    (target_cap * w / (prices[s] * (1.0 + headroom))).floor() as i64
                } else {
                    0
                };
                desired.insert(s.clone(), q);
            }
        }

        let mut sent_sells = HashSet::new();
        let mut n_sent = 0usize;
        for s in &syms {
            let cur_q = pos_qty_of(bot, s);
            let diff = desired[s] - cur_q;
            if diff < 0 {
                let mut sq = HashMap::new();
                sq.insert("symbol".to_string(), s.clone());
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
                    let r = send_market(state, bot, s, "SELL", qty, prices[s], &cid).await?;
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
                            "signal_price": prices[s],
                        }));
                        sent_sells.insert(oid);
                        n_sent += 1;
                        bots::log(
                            state,
                            bot,
                            "order",
                            &format!("[리밸런싱] {s} 매도 시장가 {qty}주"),
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
                            &format!("[리밸런싱] {s} 매도 실패: {}", e.message),
                            "error",
                            Some(json!({ "code": e.code })),
                        )
                        .await;
                    }
                }
            }
        }

        let has_buys = syms.iter().any(|s| desired[s] > pos_qty_of(bot, s));
        if !sent_sells.is_empty() && has_buys {
            wait_sell_fills(state, bot, today, &sent_sells).await;
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
            let invested: f64 = syms.iter().map(|s| (pos_qty_of(bot, s) as f64) * prices[s]).sum();
            let mut budget = avail_cash.min((cap - invested).max(0.0));
            for s in &syms {
                let cur_q = pos_qty_of(bot, s);
                let diff = desired[s] - cur_q;
                if diff <= 0 {
                    continue;
                }
                let unit = prices[s] * (1.0 + headroom);
                let qty = if unit > 0.0 {
                    diff.min((budget / unit).floor() as i64)
                } else {
                    0
                };
                if qty <= 0 {
                    bots::log(
                        state,
                        bot,
                        "skip",
                        &format!("[리밸런싱] {s} 자금 부족으로 매수 생략"),
                        "warn",
                        None,
                    )
                    .await;
                    continue;
                }
                let cid = client_order_id(&bot_id, today, s, "BUY");
                match send_market(state, bot, s, "BUY", qty, prices[s], &cid).await {
                    Ok(r) => {
                        bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
                            "kind": "order",
                            "id": r["orderId"],
                            "symbol": s,
                            "side": "BUY",
                            "qty": qty,
                            "date": today,
                            "synced": 0.0,
                            "signal_price": prices[s],
                        }));
                        budget = (budget - (qty as f64) * unit).max(0.0);
                        n_sent += 1;
                        bots::log(
                            state,
                            bot,
                            "order",
                            &format!("[리밸런싱] {s} 매수 시장가 {qty}주"),
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
                            &format!("[리밸런싱] {s} 매수 실패: {}", e.message),
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
    } else {
        bots::log(
            state,
            bot,
            "decision",
            &format!("{} 기준 리밸런싱 조건 미해당", d.bar_date),
            "info",
            None,
        )
        .await;
    }

    bot["state"]["live"]["last_prices"] = json!(prices);
    bot["state"]["live"]["target_weights"] = Value::Object(d.target_weights);
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
    Ok(json!({ "ran": today, "rebalanced": d.due }))
}

pub async fn run(state: &AppState, bot: &mut Value, today: &str) -> ApiResult<Value> {
    if bot["mode"].as_str() == Some("paper") {
        run_paper(state, bot, today).await
    } else {
        run_live(state, bot, today).await
    }
}
