//! Unified multi-bot control tower analytics & Paper-vs-Live execution scorecard (MT-9).

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::autotrade::{bots, semi};
use crate::state::AppState;

pub const USD_KRW_REF: f64 = 1380.0;

fn rebal_label(k: &str) -> &'static str {
    match k {
        "daily" => "매일",
        "weekly" => "매주",
        "biweekly" => "격주(2주)",
        "monthly" => "매월 초",
        "quarterly" => "분기 초",
        "yearly" => "매년 초",
        "drift" => "비중 편차 이탈 시",
        _ => "매월 초",
    }
}

const WEEKDAY_LABELS: [&str; 5] = ["월요일", "화요일", "수요일", "목요일", "금요일"];

fn fx(market: &str) -> f64 {
    if market == "US" {
        USD_KRW_REF
    } else {
        1.0
    }
}

fn round_n(v: f64, dec: i32) -> f64 {
    let f = 10_f64.powi(dec);
    (v * f).round() / f
}

fn state_equity_and_invested(bot: &Value, ledger: &str) -> (f64, f64) {
    let st = &bot["state"][ledger];
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let kind = bots::bot_kind(bot);
    if kind == "rebalance" || kind == "rotation" {
        let positions = st.get("positions").and_then(|v| v.as_object());
        let last_prices = st.get("last_prices").and_then(|v| v.as_object());
        let pos_val: f64 = positions
            .map(|m| {
                m.iter()
                    .map(|(sym, p)| {
                        let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let px = last_prices
                            .and_then(|lp| lp.get(sym))
                            .and_then(|v| v.as_f64())
                            .or_else(|| p.get("entry_price").and_then(|v| v.as_f64()))
                            .unwrap_or(0.0);
                        q * px
                    })
                    .sum()
            })
            .unwrap_or(0.0);
        let eq = if ledger == "paper" {
            st.get("cash").and_then(|v| v.as_f64()).unwrap_or(cap) + pos_val
        } else {
            st.get("equity")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|e| e.get("value"))
                .and_then(|v| v.as_f64())
                .unwrap_or_else(|| cap + st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0))
        };
        return (eq, pos_val);
    }

    let pos = &st["position"];
    let qty = pos.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let ep = pos.get("entry_price").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let px = st
        .get("last_price")
        .and_then(|v| v.as_f64())
        .filter(|&x| x > 0.0)
        .unwrap_or(ep);
    let pos_val = qty * px;
    let eq = if ledger == "paper" {
        st.get("cash").and_then(|v| v.as_f64()).unwrap_or(cap) + pos_val
    } else {
        st.get("equity")
            .and_then(|v| v.as_array())
            .and_then(|a| a.last())
            .and_then(|e| e.get("value"))
            .and_then(|v| v.as_f64())
            .unwrap_or_else(|| {
                cap + st.get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0) + qty * (px - ep)
            })
    };
    (eq, pos_val)
}

struct LedgerStats {
    return_pct: Option<f64>,
    trades: usize,
    win_rate_pct: Option<f64>,
}

fn ledger_trade_stats(bot: &Value, ledger: &str) -> LedgerStats {
    let st = &bot["state"][ledger];
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let has_run = st.get("last_run_date").is_some_and(|v| !v.is_null())
        || st.get("equity").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty())
        || st.get("trades").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty())
        || st.get("rebalances").and_then(|v| v.as_array()).is_some_and(|a| !a.is_empty());
    let (eq, _) = state_equity_and_invested(bot, ledger);
    let ret_pct = if cap > 0.0 && has_run {
        Some(round_n((eq / cap - 1.0) * 100.0, 2))
    } else {
        None
    };

    let kind = bots::bot_kind(bot);
    if kind == "rebalance" || kind == "rotation" {
        let rebs = st
            .get("rebalances")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        return LedgerStats {
            return_pct: ret_pct,
            trades: rebs,
            win_rate_pct: None,
        };
    }

    let trades_arr = st.get("trades").and_then(|v| v.as_array());
    let closed: Vec<&Value> = trades_arr
        .map(|arr| {
            arr.iter()
                .filter(|t| t.get("exit_date").is_some_and(|v| !v.is_null()))
                .collect()
        })
        .unwrap_or_default();
    let wins = closed
        .iter()
        .filter(|t| t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0)
        .count();
    let win_rate_pct = if !closed.is_empty() {
        Some(round_n(wins as f64 / closed.len() as f64 * 100.0, 1))
    } else {
        None
    };
    let trade_cnt = if kind != "dca" {
        closed.len()
    } else {
        st.get("dca_steps")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or_else(|| trades_arr.map(|a| a.len()).unwrap_or(0))
    };

    LedgerStats {
        return_pct: ret_pct,
        trades: trade_cnt,
        win_rate_pct,
    }
}

fn summarize_group(group: &[&Value], ledger: &str) -> Value {
    let mut by_market = Map::new();
    let mut kr_eq = 0.0;
    let mut us_eq = 0.0;
    let mut cap_krw = 0.0;
    let mut eq_krw = 0.0;
    let mut inv_krw = 0.0;

    for m in ["KR", "US"] {
        let sub: Vec<&&Value> = group
            .iter()
            .filter(|b| b["market"].as_str() == Some(m))
            .collect();
        let cap: f64 = sub.iter().map(|b| b["capital"].as_f64().unwrap_or(0.0)).sum();
        let mut eq = 0.0;
        let mut inv = 0.0;
        let mut enabled = 0usize;
        for b in &sub {
            let (e, i) = state_equity_and_invested(b, ledger);
            eq += e;
            inv += i;
            if b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                enabled += 1;
            }
        }
        let pnl = eq - cap;
        let f = fx(m);
        cap_krw += cap * f;
        eq_krw += eq * f;
        inv_krw += inv * f;
        if m == "KR" {
            kr_eq = eq;
        } else {
            us_eq = eq * USD_KRW_REF;
        }
        by_market.insert(
            m.to_string(),
            json!({
                "bots": sub.len(),
                "enabled": enabled,
                "capital": round_n(cap, 2),
                "equity": round_n(eq, 2),
                "invested": round_n(inv, 2),
                "pnl": round_n(pnl, 2),
                "return_pct": if cap > 0.0 { round_n(pnl / cap * 100.0, 2) } else { 0.0 },
            }),
        );
    }

    let pnl_krw = eq_krw - cap_krw;
    let denom = kr_eq + us_eq;
    let enabled_total = group
        .iter()
        .filter(|b| b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();

    json!({
        "bots": group.len(),
        "enabled": enabled_total,
        "capital_krw": cap_krw.round() as i64,
        "equity_krw": eq_krw.round() as i64,
        "invested_krw": inv_krw.round() as i64,
        "pnl_krw": pnl_krw.round() as i64,
        "return_pct": if cap_krw > 0.0 { round_n(pnl_krw / cap_krw * 100.0, 2) } else { 0.0 },
        "kr_share_pct": if denom > 0.0 { round_n(kr_eq / denom * 100.0, 1) } else { 0.0 },
        "us_share_pct": if denom > 0.0 { round_n(us_eq / denom * 100.0, 1) } else { 0.0 },
        "by_market": Value::Object(by_market),
    })
}

fn bot_symbols(bot: &Value) -> Vec<String> {
    let kind = bots::bot_kind(bot);
    if kind == "rebalance" {
        return bot
            .get("assets")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("symbol").and_then(|s| s.as_str()))
                    .map(|s| s.to_uppercase())
                    .collect()
            })
            .unwrap_or_default();
    }
    if kind == "rotation" {
        let st = bots::state(bot);
        let mut out = Vec::new();
        if let Some(pos) = st.get("positions").and_then(|v| v.as_object()) {
            for (s, p) in pos {
                if p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0 && !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        if let Some(picks) = st.get("selected_picks").and_then(|v| v.as_array()) {
            for x in picks {
                if let Some(s) = x.get("symbol").and_then(|v| v.as_str()) {
                    let up = s.to_uppercase();
                    if !out.contains(&up) {
                        out.push(up);
                    }
                }
            }
        }
        return out;
    }
    let sym = bot
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if !sym.is_empty() && !sym.contains(':') && !sym.contains(',') {
        vec![sym]
    } else {
        vec![]
    }
}

fn next_schedule_label(bot: &Value) -> String {
    let kind = bots::bot_kind(bot);
    if kind == "webhook" {
        return "외부 웹훅 신호 수신 시 즉시 실행".to_string();
    }
    if kind == "dca" {
        let raw = bot.get("rebalance").and_then(|v| v.as_str()).unwrap_or("weekly");
        let cad = rebal_label(raw);
        if raw == "weekly" || raw == "biweekly" {
            let wd = WEEKDAY_LABELS[(bot.get("weekday").and_then(|v| v.as_u64()).unwrap_or(0) as usize) % 5];
            return format!("{cad} {wd} 장 시작 1분 뒤 적립");
        }
        return format!("{cad} 장 시작 1분 뒤 적립");
    }
    if kind == "rotation" {
        let cad = rebal_label(bot.get("rebalance").and_then(|v| v.as_str()).unwrap_or("weekly"));
        let top_n = bot.get("top_n").and_then(|v| v.as_u64()).unwrap_or(5);
        return format!("{cad} 상위 {top_n}종목 순환 점검");
    }
    if kind == "rebalance" {
        let cad = rebal_label(bot.get("rebalance").and_then(|v| v.as_str()).unwrap_or("monthly"));
        return format!("{cad} 비중 리밸런싱 점검");
    }
    let tf = bots::bot_timeframe(bot);
    if tf != "1d" {
        return format!("장중 {tf} 봉 마감마다 · 마감 10분 전 청산");
    }
    "매일 장 시작 1분 뒤 일봉 신호 실행".to_string()
}

fn combined_return_series(user_bots: &[Value]) -> Vec<Value> {
    let paper_bots: Vec<&Value> = user_bots
        .iter()
        .filter(|b| b["mode"].as_str() == Some("paper"))
        .collect();
    let live_bots: Vec<&Value> = user_bots
        .iter()
        .filter(|b| matches!(b["mode"].as_str(), Some("live" | "semi")))
        .collect();

    let mut all_dates = BTreeSet::new();
    for b in &paper_bots {
        if let Some(arr) = b["state"]["paper"]["equity"].as_array() {
            for pt in arr {
                if let Some(d) = pt.get("date").and_then(|v| v.as_str()) {
                    all_dates.insert(d.to_string());
                }
            }
        }
    }
    for b in &live_bots {
        if let Some(arr) = b["state"]["live"]["equity"].as_array() {
            for pt in arr {
                if let Some(d) = pt.get("date").and_then(|v| v.as_str()) {
                    all_dates.insert(d.to_string());
                }
            }
        }
    }
    if all_dates.is_empty() {
        return vec![];
    }
    let all_vec: Vec<String> = all_dates.into_iter().collect();
    let start = all_vec.len().saturating_sub(180);
    let sorted_dates = &all_vec[start..];

    let paper_maps: Vec<HashMap<String, f64>> = paper_bots
        .iter()
        .map(|b| {
            let mut mp = HashMap::new();
            if let Some(arr) = b["state"]["paper"]["equity"].as_array() {
                for pt in arr {
                    if let (Some(d), Some(v)) = (
                        pt.get("date").and_then(|x| x.as_str()),
                        pt.get("value").and_then(|x| x.as_f64()),
                    ) {
                        mp.insert(d.to_string(), v);
                    }
                }
            }
            mp
        })
        .collect();
    let live_maps: Vec<HashMap<String, f64>> = live_bots
        .iter()
        .map(|b| {
            let mut mp = HashMap::new();
            if let Some(arr) = b["state"]["live"]["equity"].as_array() {
                for pt in arr {
                    if let (Some(d), Some(v)) = (
                        pt.get("date").and_then(|x| x.as_str()),
                        pt.get("value").and_then(|x| x.as_f64()),
                    ) {
                        mp.insert(d.to_string(), v);
                    }
                }
            }
            mp
        })
        .collect();

    let mut paper_last: Vec<f64> = paper_bots
        .iter()
        .map(|b| b["capital"].as_f64().unwrap_or(0.0))
        .collect();
    let mut live_last: Vec<f64> = live_bots
        .iter()
        .map(|b| b["capital"].as_f64().unwrap_or(0.0))
        .collect();
    let paper_cap_krw: f64 = paper_bots
        .iter()
        .map(|b| b["capital"].as_f64().unwrap_or(0.0) * fx(b["market"].as_str().unwrap_or("KR")))
        .sum();
    let live_cap_krw: f64 = live_bots
        .iter()
        .map(|b| b["capital"].as_f64().unwrap_or(0.0) * fx(b["market"].as_str().unwrap_or("KR")))
        .sum();

    let mut out = Vec::with_capacity(sorted_dates.len());
    for d in sorted_dates {
        for (i, mp) in paper_maps.iter().enumerate() {
            if let Some(&v) = mp.get(d) {
                paper_last[i] = v;
            }
        }
        for (i, mp) in live_maps.iter().enumerate() {
            if let Some(&v) = mp.get(d) {
                live_last[i] = v;
            }
        }
        let p_eq: f64 = paper_last
            .iter()
            .enumerate()
            .map(|(i, &v)| v * fx(paper_bots[i]["market"].as_str().unwrap_or("KR")))
            .sum();
        let l_eq: f64 = live_last
            .iter()
            .enumerate()
            .map(|(i, &v)| v * fx(live_bots[i]["market"].as_str().unwrap_or("KR")))
            .sum();
        out.push(json!({
            "date": d,
            "paper_return_pct": if paper_cap_krw > 0.0 { Some(round_n((p_eq / paper_cap_krw - 1.0) * 100.0, 2)) } else { None },
            "live_return_pct": if live_cap_krw > 0.0 { Some(round_n((l_eq / live_cap_krw - 1.0) * 100.0, 2)) } else { None },
        }));
    }
    out
}

pub fn build_overview(state: &AppState, uid: &str) -> Value {
    let user_bots = bots::list_for(state, uid);
    let paper_group: Vec<&Value> = user_bots
        .iter()
        .filter(|b| b["mode"].as_str() == Some("paper"))
        .collect();
    let live_group: Vec<&Value> = user_bots
        .iter()
        .filter(|b| matches!(b["mode"].as_str(), Some("live" | "semi")))
        .collect();

    let mut sym_buckets: BTreeMap<(String, String), Vec<&Value>> = BTreeMap::new();
    for b in &user_bots {
        let mkt = b["market"].as_str().unwrap_or("").to_string();
        for sym in bot_symbols(b) {
            sym_buckets.entry((mkt.clone(), sym)).or_default().push(b);
        }
    }

    let mut duplicate_symbols = Vec::new();
    for ((mkt, sym), shared) in sym_buckets {
        if shared.len() < 2 {
            continue;
        }
        let live_active = shared
            .iter()
            .filter(|b| {
                matches!(b["mode"].as_str(), Some("live" | "semi"))
                    && b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
            })
            .count();
        let b_list: Vec<Value> = shared
            .iter()
            .map(|b| {
                json!({
                    "id": b["id"],
                    "name": b["name"],
                    "mode": b["mode"],
                    "enabled": b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                    "category": bots::bot_category(b),
                })
            })
            .collect();
        duplicate_symbols.push(json!({
            "market": mkt,
            "symbol": sym,
            "live_conflict": live_active >= 2,
            "bots": b_list,
        }));
    }

    let mut sorted_bots: Vec<&Value> = user_bots.iter().collect();
    sorted_bots.sort_by(|a, b| {
        let ea = !a.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let eb = !b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
        let ca = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let cb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        (ea, ca).cmp(&(eb, cb))
    });

    let mut upcoming = Vec::new();
    let mut pending_approvals_count = 0usize;
    let mut tripped_count = 0usize;
    for b in sorted_bots {
        let st = bots::state(b);
        let prop = semi::active_proposal(st, None);
        if prop.is_some() {
            pending_approvals_count += 1;
        }
        if st.get("tripped").is_some_and(|v| !v.is_null()) {
            tripped_count += 1;
        }
        upcoming.push(json!({
            "bot_id": b["id"],
            "name": b["name"],
            "market": b["market"],
            "category": bots::bot_category(b),
            "mode": b["mode"],
            "enabled": b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "schedule": next_schedule_label(b),
            "last_run_date": st.get("last_run_date").cloned().unwrap_or(Value::Null),
            "pending_approval_orders": prop.as_ref().and_then(|p| p["orders"].as_array()).map(|a| a.len()).unwrap_or(0),
            "tripped": st.get("tripped").is_some_and(|v| !v.is_null()),
        }));
    }

    let mut all_bps: Vec<f64> = Vec::new();
    let mut scorecard = Vec::new();
    for b in &user_bots {
        let p_stats = ledger_trade_stats(b, "paper");
        let l_stats = ledger_trade_stats(b, "live");
        let fills = b["state"]["live"]
            .get("fills")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let bot_bps: Vec<f64> = fills
            .iter()
            .filter_map(|f| f.get("slippage_bps").and_then(|v| v.as_f64()))
            .collect();
        all_bps.extend(bot_bps.iter().copied());
        let avg_bps = if !bot_bps.is_empty() {
            Some(round_n(
                bot_bps.iter().sum::<f64>() / bot_bps.len() as f64,
                2,
            ))
        } else {
            None
        };
        let gap_pct = match (l_stats.return_pct, p_stats.return_pct) {
            (Some(l), Some(p)) => Some(round_n(l - p, 2)),
            _ => None,
        };
        scorecard.push(json!({
            "bot_id": b["id"],
            "name": b["name"],
            "market": b["market"],
            "category": bots::bot_category(b),
            "mode": b["mode"],
            "enabled": b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "paper_return_pct": p_stats.return_pct,
            "paper_trades": p_stats.trades,
            "paper_win_rate_pct": p_stats.win_rate_pct,
            "live_return_pct": l_stats.return_pct,
            "live_trades": l_stats.trades,
            "live_win_rate_pct": l_stats.win_rate_pct,
            "return_gap_pct": gap_pct,
            "fills_count": fills.len(),
            "avg_slippage_bps": avg_bps,
        }));
    }

    let enabled_bots = user_bots
        .iter()
        .filter(|b| b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false))
        .count();
    let total_avg_bps = if !all_bps.is_empty() {
        Some(round_n(
            all_bps.iter().sum::<f64>() / all_bps.len() as f64,
            2,
        ))
    } else {
        None
    };

    json!({
        "total_bots": user_bots.len(),
        "enabled_bots": enabled_bots,
        "pending_approvals_count": pending_approvals_count,
        "tripped_count": tripped_count,
        "usd_krw_ref": USD_KRW_REF,
        "paper": summarize_group(&paper_group, "paper"),
        "live": summarize_group(&live_group, "live"),
        "combined_equity": combined_return_series(&user_bots),
        "duplicate_symbols": duplicate_symbols,
        "upcoming": upcoming,
        "execution": {
            "total_fills": all_bps.len(),
            "avg_slippage_bps": total_avg_bps,
            "items": scorecard,
        },
    })
}
