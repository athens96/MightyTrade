use serde_json::{json, Value};

const PERIODS_PER_YEAR: f64 = 252.0;

fn clean(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

pub fn drawdown_series(equity: &[f64]) -> Vec<f64> {
    let mut peak = f64::NEG_INFINITY;
    let mut out = Vec::with_capacity(equity.len());
    for &e in equity {
        if e > peak {
            peak = e;
        }
        out.push(if peak > 0.0 { e / peak - 1.0 } else { 0.0 });
    }
    out
}

fn periods_per_year(times: &[i64]) -> f64 {
    if times.len() < 2 {
        return PERIODS_PER_YEAR;
    }
    let mut diffs: Vec<i64> = (1..times.len())
        .map(|i| times[i] - times[i - 1])
        .filter(|&d| d > 0)
        .collect();
    if diffs.is_empty() {
        return PERIODS_PER_YEAR;
    }
    diffs.sort_unstable();
    let step = diffs[diffs.len() / 2];
    if step >= 43200 {
        PERIODS_PER_YEAR
    } else {
        PERIODS_PER_YEAR * (6.5 * 3600.0 / step as f64)
    }
}

pub fn performance(times: &[i64], equity: &[f64]) -> Value {
    if times.is_empty() || equity.is_empty() {
        return json!({});
    }
    let start = equity[0];
    let end = equity[equity.len() - 1];
    let years = (times[times.len() - 1] - times[0]) as f64 / 86400.0 / 365.25;
    let ppy = periods_per_year(times);
    let rets: Vec<f64> = (1..equity.len())
        .filter(|&i| equity[i - 1] > 0.0)
        .map(|i| equity[i] / equity[i - 1] - 1.0)
        .collect();
    let mean = if rets.is_empty() {
        0.0
    } else {
        rets.iter().sum::<f64>() / rets.len() as f64
    };
    let sd = if rets.len() > 1 {
        let var = rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64;
        var.sqrt()
    } else {
        0.0
    };
    let downside = if !rets.is_empty() {
        (rets.iter().map(|r| r.min(0.0).powi(2)).sum::<f64>() / rets.len() as f64).sqrt()
    } else {
        0.0
    };
    let dd = drawdown_series(equity);
    let mdd = dd.iter().copied().fold(0.0f64, f64::min);

    let mut longest = 0i64;
    let mut peak = f64::NEG_INFINITY;
    let mut peak_t = times[0];
    let mut underwater = false;
    for (&t, &e) in times.iter().zip(equity.iter()) {
        if e >= peak {
            if underwater {
                longest = longest.max(t - peak_t);
                underwater = false;
            }
            peak = e;
            peak_t = t;
        } else {
            underwater = true;
            longest = longest.max(t - peak_t);
        }
    }

    let cagr = if years > 0.05 && start > 0.0 && end > 0.0 {
        Some((end / start).powf(1.0 / years) - 1.0)
    } else {
        None
    };

    json!({
        "start_equity": start,
        "final_equity": end,
        "total_return_pct": if start != 0.0 { clean(Some((end / start - 1.0) * 100.0)) } else { None },
        "cagr_pct": cagr.and_then(|c| clean(Some(c * 100.0))),
        "mdd_pct": mdd * 100.0,
        "max_dd_days": (longest as f64 / 86400.0).round() as i64,
        "volatility_pct": clean(Some(sd * ppy.sqrt() * 100.0)),
        "sharpe": if sd > 0.0 { clean(Some(mean / sd * ppy.sqrt())) } else { None },
        "sortino": if downside > 0.0 { clean(Some(mean / downside * ppy.sqrt())) } else { None },
        "calmar": cagr.and_then(|c| if mdd < 0.0 { clean(Some(c / mdd.abs())) } else { None }),
    })
}

pub fn trade_stats(trades: &[Value]) -> Value {
    let closed: Vec<&Value> = trades
        .iter()
        .filter(|t| !t.get("open").and_then(|v| v.as_bool()).unwrap_or(true))
        .collect();
    let wins: Vec<&Value> = closed
        .iter()
        .copied()
        .filter(|t| t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0)
        .collect();
    let losses: Vec<&Value> = closed
        .iter()
        .copied()
        .filter(|t| t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0) <= 0.0)
        .collect();

    let gross_win: f64 = wins
        .iter()
        .map(|t| t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0))
        .sum();
    let gross_loss: f64 = -losses
        .iter()
        .map(|t| t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0))
        .sum::<f64>();

    let avg = |xs: Vec<f64>| -> Option<f64> {
        if xs.is_empty() {
            None
        } else {
            Some(xs.iter().sum::<f64>() / xs.len() as f64)
        }
    };
    let ret_of = |ts: &[&Value]| -> Vec<f64> {
        ts.iter()
            .filter_map(|t| t.get("return_pct").and_then(|v| v.as_f64()))
            .collect()
    };
    let closed_rets = ret_of(&closed);
    let best = closed_rets.iter().copied().reduce(f64::max);
    let worst = closed_rets.iter().copied().reduce(f64::min);
    let bars_list: Vec<f64> = closed
        .iter()
        .filter_map(|t| t.get("bars").and_then(|v| v.as_f64()))
        .collect();

    json!({
        "trades": closed.len(),
        "win_rate_pct": if closed.is_empty() { None } else { Some(wins.len() as f64 / closed.len() as f64 * 100.0) },
        "avg_return_pct": avg(closed_rets),
        "avg_win_pct": avg(ret_of(&wins)),
        "avg_loss_pct": avg(ret_of(&losses)),
        "best_pct": best,
        "worst_pct": worst,
        "profit_factor": if gross_loss > 0.0 { clean(Some(gross_win / gross_loss)) } else { None },
        "avg_bars": avg(bars_list),
    })
}
