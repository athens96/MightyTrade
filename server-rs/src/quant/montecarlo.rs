use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde_json::{json, Map, Value};

use crate::errors::{ApiError, ApiResult};

const PERCENTILES: [f64; 5] = [5.0, 25.0, 50.0, 75.0, 95.0];
const MAX_PATH_POINTS: usize = 200;
const MIN_TRADES: usize = 5;

pub fn trade_impacts(trades: &[Value], equity: &[Value]) -> Vec<f64> {
    let times: Vec<i64> = equity
        .iter()
        .map(|p| p["time"].as_i64().unwrap_or(0))
        .collect();
    let mut out = Vec::new();
    for t in trades {
        if t.get("open").and_then(|v| v.as_bool()).unwrap_or(true) {
            continue;
        }
        let Some(pnl) = t.get("pnl").and_then(|v| v.as_f64()) else {
            continue;
        };
        let et = t.get("entry_time").and_then(|v| v.as_i64()).unwrap_or(0);
        let idx = times.partition_point(|&tm| tm < et).saturating_sub(1);
        let base = equity
            .get(idx)
            .and_then(|p| p["equity"].as_f64())
            .unwrap_or(0.0);
        if base > 0.0 {
            out.push(pnl / base);
        }
    }
    out
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = (sorted.len() - 1) as f64 * p / 100.0;
    let lo = k.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (k - lo as f64)
}

fn rank_pct(sorted: &[f64], v: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = sorted.partition_point(|&x| x < v);
    idx as f64 / sorted.len() as f64 * 100.0
}

fn eval_path(seq: &[f64]) -> (Vec<f64>, f64, f64) {
    let mut eq = 1.0;
    let mut peak = 1.0;
    let mut mdd = 0.0f64;
    let mut path = Vec::with_capacity(seq.len() + 1);
    path.push(1.0);
    for &r in seq {
        eq *= 1.0 + r;
        if eq > peak {
            peak = eq;
        }
        mdd = mdd.min(eq / peak - 1.0);
        path.push(eq);
    }
    (path, eq - 1.0, mdd)
}

fn histogram(values: &[f64], bins: usize) -> Value {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let mut hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if (hi - lo).abs() < 1e-12 {
        hi = lo + 1e-9;
    }
    let width = (hi - lo) / bins as f64;
    let mut counts = vec![0usize; bins];
    for &v in values {
        let idx = (((v - lo) / width).floor() as isize).clamp(0, (bins - 1) as isize) as usize;
        counts[idx] += 1;
    }
    let edges: Vec<f64> = (0..=bins).map(|i| lo + i as f64 * width).collect();
    json!({ "edges": edges, "counts": counts })
}

pub fn simulate(
    impacts: &[f64],
    runs: usize,
    method: &str,
    seed: u64,
    years: Option<f64>,
) -> ApiResult<Value> {
    let n = impacts.len();
    if n < MIN_TRADES {
        return Err(ApiError::bad_request(
            "not_enough_trades",
            format!("몬테카를로에는 청산된 매매가 {MIN_TRADES}회 이상 필요합니다 (현재 {n}회)"),
        ));
    }

    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let step = ((n + 1) / MAX_PATH_POINTS).max(1);
    let mut idxs: Vec<usize> = (0..=n).step_by(step).collect();
    if *idxs.last().unwrap() != n {
        idxs.push(n);
    }

    let mut finals = Vec::with_capacity(runs);
    let mut mdds = Vec::with_capacity(runs);
    let mut paths: Vec<Vec<f64>> = Vec::with_capacity(runs);
    let mut seq = Vec::with_capacity(n);

    for _ in 0..runs {
        seq.clear();
        if method == "shuffle" {
            seq.extend_from_slice(impacts);
            seq.shuffle(&mut rng);
        } else {
            for _ in 0..n {
                let idx = rng.gen_range(0..n);
                seq.push(impacts[idx]);
            }
        }
        let (path, fin, mdd) = eval_path(&seq);
        finals.push(fin * 100.0);
        mdds.push(mdd * 100.0);
        paths.push(idxs.iter().map(|&k| path[k]).collect());
    }

    let (orig_path, orig_fin, orig_mdd) = eval_path(impacts);
    let mut sf = finals.clone();
    sf.sort_by(|a, b| a.total_cmp(b));
    let mut sm = mdds.clone();
    sm.sort_by(|a, b| a.total_cmp(b));

    let mut bands: Map<String, Value> = Map::new();
    let mut band_vecs: [Vec<f64>; 5] = Default::default();
    for j in 0..idxs.len() {
        let mut col: Vec<f64> = paths.iter().map(|p| p[j]).collect();
        col.sort_by(|a, b| a.total_cmp(b));
        for (pi, &p) in PERCENTILES.iter().enumerate() {
            band_vecs[pi].push((percentile(&col, p) - 1.0) * 100.0);
        }
    }
    for (pi, &p) in PERCENTILES.iter().enumerate() {
        bands.insert(format!("p{}", p as i32), json!(band_vecs[pi]));
    }

    let cagr_fn = |final_pct: f64| -> Option<f64> {
        let y = years?;
        if y <= 0.0 || final_pct <= -100.0 {
            return None;
        }
        Some(((1.0 + final_pct / 100.0).powf(1.0 / y) - 1.0) * 100.0)
    };

    let mut final_p = Map::new();
    let mut cagr_p = Map::new();
    let mut mdd_p = Map::new();
    for &p in &PERCENTILES {
        let k = format!("p{}", p as i32);
        let fp = percentile(&sf, p);
        final_p.insert(k.clone(), json!(fp));
        cagr_p.insert(k.clone(), json!(cagr_fn(fp)));
        mdd_p.insert(k, json!(percentile(&sm, p)));
    }

    let prob_loss = finals.iter().filter(|&&f| f < 0.0).count() as f64 / runs as f64 * 100.0;
    let prob_mdd_worse =
        mdds.iter().filter(|&&m| m < orig_mdd * 100.0).count() as f64 / runs as f64 * 100.0;
    let orig_downsampled: Vec<f64> = idxs.iter().map(|&k| (orig_path[k] - 1.0) * 100.0).collect();

    Ok(json!({
        "runs": runs,
        "method": method,
        "trades": n,
        "final_return_pct": Value::Object(final_p),
        "cagr_pct": Value::Object(cagr_p),
        "mdd_pct": Value::Object(mdd_p),
        "prob_loss_pct": prob_loss,
        "prob_mdd_worse_pct": prob_mdd_worse,
        "original": {
            "final_return_pct": orig_fin * 100.0,
            "mdd_pct": orig_mdd * 100.0,
            "final_rank_pct": rank_pct(&sf, orig_fin * 100.0),
            "mdd_rank_pct": rank_pct(&sm, orig_mdd * 100.0),
            "path": orig_downsampled,
        },
        "path_index": idxs,
        "bands": Value::Object(bands),
        "hist_final": histogram(&finals, 30),
        "hist_mdd": histogram(&mdds, 30),
    }))
}
