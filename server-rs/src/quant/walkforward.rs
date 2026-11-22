use serde_json::{json, Value};

use crate::errors::{ApiError, ApiResult};
use crate::quant::engine::{buy_and_hold, run_backtest, Bars, Costs, RiskRules};
use crate::quant::metrics::{drawdown_series, performance, trade_stats};
use crate::quant::optimize::{run_grid, Axis, ComboItem};
use crate::quant::strategies::StrategySpec;

const YEAR: f64 = 365.25 * 86400.0;
const MONTH: f64 = YEAR / 12.0;
const MAX_WINDOWS: usize = 20;
const MIN_OOS_BARS: usize = 20;

fn bisect_left(times: &[i64], target: i64) -> usize {
    times.partition_point(|&t| t < target)
}

pub fn make_windows(
    times: &[i64],
    start: usize,
    is_years: f64,
    oos_months: f64,
    mode: &str,
) -> ApiResult<Vec<(usize, usize, usize)>> {
    let t0 = times[start] as f64;
    let mut out = Vec::new();
    let mut k = 0usize;
    loop {
        let is_start_t = if mode == "anchored" {
            t0
        } else {
            t0 + (k as f64) * oos_months * MONTH
        };
        let is_end_t = t0 + is_years * YEAR + (k as f64) * oos_months * MONTH;
        let oos_end_t = is_end_t + oos_months * MONTH;
        let is_start = bisect_left(times, is_start_t as i64);
        let is_end = bisect_left(times, is_end_t as i64);
        let oos_end = bisect_left(times, oos_end_t as i64).min(times.len());
        if is_end >= times.len() || oos_end.saturating_sub(is_end) < MIN_OOS_BARS {
            break;
        }
        out.push((is_start.max(start), is_end, oos_end));
        k += 1;
        if out.len() > MAX_WINDOWS {
            return Err(ApiError::bad_request(
                "too_many_windows",
                format!("구간이 {MAX_WINDOWS}개를 넘습니다. 검증 기간을 늘리세요"),
            ));
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn walk_forward(
    bars: &Bars,
    spec: &StrategySpec,
    items: &[ComboItem],
    x: &Axis,
    y: Option<&Axis>,
    capital: f64,
    costs: Costs,
    start: usize,
    objective: &str,
    min_trades: usize,
    is_years: f64,
    oos_months: f64,
    mode: &str,
) -> ApiResult<Value> {
    let windows = make_windows(&bars.time, start, is_years, oos_months, mode)?;
    if windows.is_empty() {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "기간이 학습+검증 기간보다 짧습니다. 기간을 늘리거나 학습 기간을 줄이세요",
        ));
    }

    let mut equity_pts: Vec<(i64, f64)> = Vec::new();
    let mut all_trades: Vec<Value> = Vec::new();
    let mut out_windows: Vec<Value> = Vec::new();
    let mut eq = capital;
    let mut is_cagrs: Vec<f64> = Vec::new();

    for &(is_start, is_end, oos_end) in &windows {
        let is_bars = bars.slice(is_end);
        let grid = run_grid(
            &is_bars, spec, items, x, y, capital, costs, is_start, objective, min_trades,
        );
        let best = &grid["best"];
        if best.is_null() {
            for &t in &bars.time[is_end..oos_end] {
                equity_pts.push((t, eq));
            }
            out_windows.push(json!({
                "is_start": bars.time[is_start],
                "is_end": bars.time[is_end - 1],
                "oos_start": bars.time[is_end],
                "oos_end": bars.time[oos_end - 1],
                "params": Value::Null,
                "is_score": Value::Null,
                "is_cagr_pct": Value::Null,
                "oos": Value::Null,
            }));
            continue;
        }

        let mut strat = (spec.build)(&best["params"])
            .map_err(|e| ApiError::bad_request("invalid_params", e))?;
        let oos_bars = bars.slice(oos_end);
        let r = run_backtest(
            &oos_bars,
            strat.as_mut(),
            eq,
            costs,
            is_end,
            true,
            RiskRules::default(),
            None,
            "off",
        )
        .map_err(|e| ApiError::bad_request("backtest_failed", e))?;

        let seg = r["equity"].as_array().cloned().unwrap_or_default();
        for p in &seg {
            equity_pts.push((p["time"].as_i64().unwrap_or(0), p["equity"].as_f64().unwrap_or(eq)));
        }
        if let Some(last_p) = seg.last() {
            eq = last_p["equity"].as_f64().unwrap_or(eq);
        }
        if let Some(trs) = r["trades"].as_array() {
            all_trades.extend(trs.iter().cloned());
        }
        if let Some(cagr) = best["metrics"].get("cagr_pct").and_then(|v| v.as_f64()) {
            is_cagrs.push(cagr);
        }
        let oos_m = json!({
            "total_return_pct": r["metrics"].get("total_return_pct").cloned().unwrap_or(Value::Null),
            "cagr_pct": r["metrics"].get("cagr_pct").cloned().unwrap_or(Value::Null),
            "mdd_pct": r["metrics"].get("mdd_pct").cloned().unwrap_or(Value::Null),
            "sharpe": r["metrics"].get("sharpe").cloned().unwrap_or(Value::Null),
            "trades": r["metrics"].get("trades").cloned().unwrap_or(Value::Null),
        });
        out_windows.push(json!({
            "is_start": bars.time[is_start],
            "is_end": bars.time[is_end - 1],
            "oos_start": bars.time[is_end],
            "oos_end": bars.time[oos_end - 1],
            "params": best["params"],
            "is_score": best["score"],
            "is_cagr_pct": best["metrics"].get("cagr_pct").cloned().unwrap_or(Value::Null),
            "oos": oos_m,
        }));
    }

    let first_oos = windows[0].1;
    let last_oos = windows[windows.len() - 1].2;
    let times: Vec<i64> = equity_pts.iter().map(|x| x.0).collect();
    let values: Vec<f64> = equity_pts.iter().map(|x| x.1).collect();
    let bench = buy_and_hold(bars, first_oos, last_oos, capital, costs, true);
    let dd = drawdown_series(&values);
    let mut metrics = performance(&times, &values);
    let ts = trade_stats(&all_trades);
    if let (Some(m), Some(to)) = (metrics.as_object_mut(), ts.as_object()) {
        for (k, v) in to {
            m.insert(k.clone(), v.clone());
        }
    }
    let mean_is = if is_cagrs.is_empty() {
        None
    } else {
        Some(is_cagrs.iter().sum::<f64>() / is_cagrs.len() as f64)
    };
    let oos_cagr = metrics.get("cagr_pct").and_then(|v| v.as_f64());
    let efficiency = match (oos_cagr, mean_is) {
        (Some(oc), Some(mi)) if mi > 0.0 => Some(oc / mi),
        _ => None,
    };

    let r4 = |x: f64| (x * 10000.0).round() / 10000.0;
    let eq_json: Vec<Value> = (0..times.len())
        .map(|i| {
            json!({
                "time": times[i],
                "equity": r4(values[i]),
                "benchmark": r4(bench[i]),
                "drawdown": r4(dd[i] * 100.0),
            })
        })
        .collect();

    Ok(json!({
        "windows": out_windows,
        "equity": eq_json,
        "metrics": metrics,
        "benchmark": performance(&times, &bench),
        "trades": all_trades,
        "efficiency": efficiency,
        "mean_is_cagr_pct": mean_is,
    }))
}
