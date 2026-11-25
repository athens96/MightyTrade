//! Multi-asset portfolio allocation theories (12 models incl. All Weather, 60/40, 1/N,
//! Risk Parity, GEM, VAA, DAA, BAA, LAA, Permanent, HRP, Sector Rotation) & rebalancing engine.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::errors::{ApiError, ApiResult};
use crate::market::models::Market;
use crate::quant::engine::{Bars, Costs};
use crate::quant::metrics::{drawdown_series, performance};
use crate::quant::strategies::{
    params_to_json, quant151::solve_linear, resolve_params, Param, ResolvedParams,
};

pub const MIN_ASSETS: usize = 2;
pub const MAX_ASSETS: usize = 16;
pub const VALID_ROLES: [&str; 4] = ["offensive", "defensive", "canary", "core"];

pub fn rebalance_modes_json() -> Value {
    json!({
        "monthly": "매월 초",
        "quarterly": "분기 초",
        "yearly": "매년 초",
        "drift": "비중 편차 이탈 시",
    })
}

pub fn rebalance_mode_label(mode: &str) -> &'static str {
    match mode {
        "monthly" => "매월 초",
        "quarterly" => "분기 초",
        "yearly" => "매년 초",
        "drift" => "비중 편차 이탈 시",
        _ => "정기",
    }
}

pub fn is_valid_rebalance_mode(mode: &str) -> bool {
    matches!(mode, "monthly" | "quarterly" | "yearly" | "drift")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetItem {
    pub symbol: String,
    pub name: String,
    pub role: String,
    pub weight: f64,
}

pub fn day_str(ts: i64) -> String {
    DateTime::from_timestamp(ts, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_else(|| "1970-01-01".to_string())
}

fn period_key(day: &str, mode: &str) -> (i32, i32) {
    let y: i32 = day.get(0..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let m: i32 = day.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1);
    match mode {
        "monthly" => (y, m),
        "quarterly" => (y, (m - 1) / 3 + 1),
        "yearly" => (y, 1),
        _ => (0, 0),
    }
}

pub fn should_rebalance(
    today: &str,
    last_rebalance_date: Option<&str>,
    mode: &str,
    current_weights: &HashMap<String, f64>,
    target_weights: &HashMap<String, f64>,
    drift_pct: f64,
) -> Option<String> {
    let Some(last) = last_rebalance_date else {
        return Some("최초 포트폴리오 구성".to_string());
    };
    if matches!(mode, "monthly" | "quarterly" | "yearly")
        && period_key(today, mode) != period_key(last, mode)
    {
        return Some(format!("정기 리밸런싱 ({})", rebalance_mode_label(mode)));
    }
    if mode == "drift" {
        let mut syms: HashSet<&String> = current_weights.keys().collect();
        syms.extend(target_weights.keys());
        let max_drift = syms
            .into_iter()
            .map(|s| {
                let cw = current_weights.get(s).copied().unwrap_or(0.0);
                let tw = target_weights.get(s).copied().unwrap_or(0.0);
                (cw - tw).abs() * 100.0
            })
            .fold(0.0f64, f64::max);
        if max_drift >= drift_pct {
            return Some(format!("비중 편차 {max_drift:.1}% ≥ {drift_pct}% 리밸런싱"));
        }
    }
    None
}

// ------------------------------------------------------------------ allocation algorithms
fn fixed_weights(
    _closes: &HashMap<String, Vec<f64>>,
    _i: usize,
    assets: &[AssetItem],
    _params: &ResolvedParams,
) -> HashMap<String, f64> {
    let total: f64 = assets.iter().map(|a| a.weight.max(0.0)).sum();
    if total <= 0.0 {
        let eq = 1.0 / assets.len() as f64;
        return assets.iter().map(|a| (a.symbol.clone(), eq)).collect();
    }
    assets
        .iter()
        .map(|a| (a.symbol.clone(), a.weight.max(0.0) / total))
        .collect()
}

fn equal_weights(
    _closes: &HashMap<String, Vec<f64>>,
    _i: usize,
    assets: &[AssetItem],
    _params: &ResolvedParams,
) -> HashMap<String, f64> {
    let eq = 1.0 / assets.len() as f64;
    assets.iter().map(|a| (a.symbol.clone(), eq)).collect()
}

fn risk_parity_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let w = params.get("vol_window").copied().unwrap_or(60.0) as usize;
    let mut inv = HashMap::new();
    for a in assets {
        let cs = &closes[&a.symbol];
        let start = (i + 1).saturating_sub(w).max(1);
        let rets: Vec<f64> = (start..=i)
            .filter(|&k| cs[k - 1] > 0.0)
            .map(|k| cs[k] / cs[k - 1] - 1.0)
            .collect();
        if rets.len() < 2 {
            inv.insert(a.symbol.clone(), 1.0);
            continue;
        }
        let mean = rets.iter().sum::<f64>() / rets.len() as f64;
        let sd = (rets.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (rets.len() - 1) as f64).sqrt();
        inv.insert(a.symbol.clone(), 1.0 / sd.max(1e-6));
    }
    let total: f64 = inv.values().sum();
    let denom = if total > 0.0 { total } else { 1.0 };
    inv.into_iter().map(|(s, v)| (s, v / denom)).collect()
}

fn dual_momentum_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let lb = params.get("lookback").copied().unwrap_or(252.0) as usize;
    let top_n = (params.get("top_n").copied().unwrap_or(1.0) as usize).max(1);
    let mut off: Vec<&str> = assets
        .iter()
        .filter(|a| a.role != "defensive")
        .map(|a| a.symbol.as_str())
        .collect();
    let dfn: Vec<&str> = assets
        .iter()
        .filter(|a| a.role == "defensive")
        .map(|a| a.symbol.as_str())
        .collect();
    if off.is_empty() {
        off = assets.iter().map(|a| a.symbol.as_str()).collect();
    }
    let ret = |sym: &str| -> f64 {
        let cs = &closes[sym];
        if i < lb || cs[i - lb] <= 0.0 {
            f64::NEG_INFINITY
        } else {
            cs[i] / cs[i - lb] - 1.0
        }
    };
    let mut ranked_off: Vec<(f64, &str)> = off.iter().map(|&s| (ret(s), s)).collect();
    ranked_off.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
    let best_def = dfn
        .iter()
        .map(|&s| (ret(s), s))
        .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|x| x.1);

    let picks = &ranked_off[..top_n.min(ranked_off.len())];
    let slice_w = if picks.is_empty() { 0.0 } else { 1.0 / picks.len() as f64 };
    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    for &(r, sym) in picks {
        if r > 0.0 {
            *out.entry(sym.to_string()).or_insert(0.0) += slice_w;
        } else if let Some(bd) = best_def {
            *out.entry(bd.to_string()).or_insert(0.0) += slice_w;
        }
    }
    out
}

fn vaa_score(cs: &[f64], i: usize) -> f64 {
    if i < 252 {
        return f64::NEG_INFINITY;
    }
    let c = cs[i];
    let r = |d: usize| if cs[i - d] > 0.0 { c / cs[i - d] - 1.0 } else { 0.0 };
    12.0 * r(21) + 4.0 * r(63) + 2.0 * r(126) + r(252)
}

fn sma12_ratio(cs: &[f64], i: usize) -> f64 {
    if i < 252 {
        return f64::NEG_INFINITY;
    }
    let avg: f64 = (0..13).map(|k| cs[i - k * 21]).sum::<f64>() / 13.0;
    if avg > 0.0 { cs[i] / avg - 1.0 } else { f64::NEG_INFINITY }
}

fn vaa_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let top_off = (params.get("top_offensive").copied().unwrap_or(1.0) as usize).max(1);
    let mut off: Vec<&str> = assets.iter().filter(|a| a.role != "defensive").map(|a| a.symbol.as_str()).collect();
    let dfn: Vec<&str> = assets.iter().filter(|a| a.role == "defensive").map(|a| a.symbol.as_str()).collect();
    if off.is_empty() {
        off = assets.iter().map(|a| a.symbol.as_str()).collect();
    }
    let mut off_scores: Vec<(f64, &str)> = off.iter().map(|&s| (vaa_score(&closes[s], i), s)).collect();
    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    if off_scores.iter().all(|(sc, _)| *sc > 0.0) {
        off_scores.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
        let picks = &off_scores[..top_off.min(off_scores.len())];
        let w = 1.0 / picks.len() as f64;
        for &(_, sym) in picks {
            out.insert(sym.to_string(), w);
        }
    } else if !dfn.is_empty() {
        let best_def = dfn
            .iter()
            .map(|&s| (vaa_score(&closes[s], i), s))
            .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .unwrap()
            .1;
        out.insert(best_def.to_string(), 1.0);
    }
    out
}

fn daa_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let top_off = (params.get("top_offensive").copied().unwrap_or(2.0) as usize).max(1);
    let threshold = (params.get("canary_threshold").copied().unwrap_or(2.0) as usize).max(1);
    let mut canaries: Vec<&str> = assets.iter().filter(|a| a.role == "canary").map(|a| a.symbol.as_str()).collect();
    let mut off: Vec<&str> = assets.iter().filter(|a| a.role == "offensive").map(|a| a.symbol.as_str()).collect();
    let dfn: Vec<&str> = assets.iter().filter(|a| a.role == "defensive").map(|a| a.symbol.as_str()).collect();
    if off.is_empty() {
        off = assets.iter().filter(|a| a.role != "defensive").map(|a| a.symbol.as_str()).collect();
    }
    if canaries.is_empty() {
        canaries = off.clone();
    }

    let bad = canaries.iter().filter(|&&s| vaa_score(&closes[s], i) <= 0.0).count();
    let b_cap = threshold.min(canaries.len().max(1));
    let protect = (bad as f64 / b_cap as f64).min(1.0);

    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    if protect > 0.0 && !dfn.is_empty() {
        let best_def = dfn
            .iter()
            .map(|&s| (vaa_score(&closes[s], i), s))
            .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
            .unwrap()
            .1;
        out.insert(best_def.to_string(), protect);
    }
    let off_w = 1.0 - if dfn.is_empty() { 0.0 } else { protect };
    if off_w > 0.0 && !off.is_empty() {
        let mut ranked: Vec<(f64, &str)> = off.iter().map(|&s| (vaa_score(&closes[s], i), s)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
        let picks = &ranked[..top_off.min(ranked.len())];
        let each = off_w / picks.len() as f64;
        for &(_, sym) in picks {
            *out.entry(sym.to_string()).or_insert(0.0) += each;
        }
    }
    out
}

fn baa_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let top_off = (params.get("top_offensive").copied().unwrap_or(1.0) as usize).max(1);
    let top_def = (params.get("top_defensive").copied().unwrap_or(3.0) as usize).max(1);
    let mut canaries: Vec<&str> = assets.iter().filter(|a| a.role == "canary").map(|a| a.symbol.as_str()).collect();
    let off: Vec<&str> = assets.iter().filter(|a| a.role == "offensive").map(|a| a.symbol.as_str()).collect();
    let dfn: Vec<&str> = assets.iter().filter(|a| a.role == "defensive").map(|a| a.symbol.as_str()).collect();
    if canaries.is_empty() {
        canaries = off.clone();
    }

    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    if !canaries.is_empty() && canaries.iter().all(|&s| vaa_score(&closes[s], i) > 0.0) && !off.is_empty() {
        let mut ranked: Vec<(f64, &str)> = off.iter().map(|&s| (sma12_ratio(&closes[s], i), s)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
        let picks = &ranked[..top_off.min(ranked.len())];
        let each = 1.0 / picks.len() as f64;
        for &(_, sym) in picks {
            out.insert(sym.to_string(), each);
        }
    } else if !dfn.is_empty() {
        let mut ranked_def: Vec<(f64, &str)> = dfn.iter().map(|&s| (sma12_ratio(&closes[s], i), s)).collect();
        ranked_def.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
        let picks = &ranked_def[..top_def.min(ranked_def.len())];
        let safest = ranked_def[0].1;
        let each = 1.0 / picks.len() as f64;
        for &(ratio, sym) in picks {
            let target = if ratio >= 0.0 { sym } else { safest };
            *out.entry(target.to_string()).or_insert(0.0) += each;
        }
    }
    out
}

fn laa_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let trend_days = (params.get("trend_days").copied().unwrap_or(200.0) as usize).max(20);
    let cores: Vec<&AssetItem> = assets.iter().filter(|a| a.role == "core").collect();
    let off: Vec<&str> = assets.iter().filter(|a| a.role == "offensive").map(|a| a.symbol.as_str()).collect();
    let dfn: Vec<&str> = assets.iter().filter(|a| a.role == "defensive").map(|a| a.symbol.as_str()).collect();

    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    let core_sum: f64 = cores.iter().map(|a| a.weight.max(0.0)).sum();
    if core_sum >= 100.0 && !cores.is_empty() {
        for a in &cores {
            out.insert(a.symbol.clone(), a.weight.max(0.0) / core_sum);
        }
        return out;
    }
    for a in &cores {
        out.insert(a.symbol.clone(), a.weight.max(0.0) / 100.0);
    }
    let timing_w = (1.0 - core_sum / 100.0).max(0.0);
    if timing_w <= 0.0 {
        return out;
    }

    let gauge = cores
        .first()
        .map(|a| a.symbol.as_str())
        .or_else(|| off.first().copied())
        .unwrap_or(assets[0].symbol.as_str());
    let cs = &closes[gauge];
    let bull = if i >= trend_days {
        let sma = cs[i + 1 - trend_days..=i].iter().sum::<f64>() / trend_days as f64;
        cs[i] > sma
    } else {
        true
    };
    let sleeve = if bull && !off.is_empty() {
        &off
    } else if !dfn.is_empty() {
        &dfn
    } else {
        &off
    };
    if !sleeve.is_empty() {
        let each = timing_w / sleeve.len() as f64;
        for &s in sleeve {
            *out.entry(s.to_string()).or_insert(0.0) += each;
        }
    }
    out
}

#[derive(Clone)]
struct HrpNode {
    leaves: Vec<usize>,
    left: Option<Box<HrpNode>>,
    right: Option<Box<HrpNode>>,
}

fn hrp_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let n = assets.len();
    if n == 1 {
        return [(assets[0].symbol.clone(), 1.0)].into_iter().collect();
    }
    let w = (params.get("vol_window").copied().unwrap_or(60.0) as usize).max(10);
    let start = (i + 1).saturating_sub(w).max(1);
    let rets: Vec<Vec<f64>> = assets
        .iter()
        .map(|a| {
            let cs = &closes[&a.symbol];
            (start..=i)
                .map(|k| if cs[k - 1] > 0.0 { cs[k] / cs[k - 1] - 1.0 } else { 0.0 })
                .collect()
        })
        .collect();
    let m = rets[0].len();
    if m < 3 {
        let eq = 1.0 / n as f64;
        return assets.iter().map(|a| (a.symbol.clone(), eq)).collect();
    }
    let means: Vec<f64> = rets.iter().map(|r| r.iter().sum::<f64>() / m as f64).collect();
    let mut cov = vec![vec![0.0; n]; n];
    for a in 0..n {
        for b in a..n {
            let mut v: f64 = (0..m)
                .map(|t| (rets[a][t] - means[a]) * (rets[b][t] - means[b]))
                .sum::<f64>()
                / (m - 1) as f64;
            if a == b {
                v = v.max(1e-8);
            }
            cov[a][b] = v;
            cov[b][a] = v;
        }
    }
    let mut dist = vec![vec![0.0; n]; n];
    for a in 0..n {
        for b in (a + 1)..n {
            let corr = (cov[a][b] / (cov[a][a] * cov[b][b]).sqrt()).clamp(-1.0, 1.0);
            let d = (0.5 * (1.0 - corr)).max(0.0).sqrt();
            dist[a][b] = d;
            dist[b][a] = d;
        }
    }

    let mut nodes: Vec<HrpNode> = (0..n)
        .map(|k| HrpNode {
            leaves: vec![k],
            left: None,
            right: None,
        })
        .collect();
    while nodes.len() > 1 {
        let mut best_d = f64::INFINITY;
        let mut bi = 0;
        let mut bj = 1;
        for a in 0..nodes.len() {
            for b in (a + 1)..nodes.len() {
                let mut d = f64::INFINITY;
                for &u in &nodes[a].leaves {
                    for &v in &nodes[b].leaves {
                        if dist[u][v] < d {
                            d = dist[u][v];
                        }
                    }
                }
                if d < best_d {
                    best_d = d;
                    bi = a;
                    bj = b;
                }
            }
        }
        let right = nodes.remove(bj);
        let left = nodes.remove(bi);
        let mut leaves = left.leaves.clone();
        leaves.extend_from_slice(&right.leaves);
        nodes.insert(
            bi,
            HrpNode {
                leaves,
                left: Some(Box::new(left)),
                right: Some(Box::new(right)),
            },
        );
    }

    fn cluster_var(idxs: &[usize], cov: &[Vec<f64>]) -> f64 {
        let iv: Vec<f64> = idxs.iter().map(|&k| 1.0 / cov[k][k]).collect();
        let s_iv: f64 = iv.iter().sum();
        let denom = if s_iv > 0.0 { s_iv } else { 1.0 };
        let cw: Vec<f64> = iv.iter().map(|&x| x / denom).collect();
        let mut total = 0.0;
        for (p, &kp) in idxs.iter().enumerate() {
            for (q, &kq) in idxs.iter().enumerate() {
                total += cw[p] * cw[q] * cov[kp][kq];
            }
        }
        total.max(1e-10)
    }

    fn bisect(node: &HrpNode, cov: &[Vec<f64>], weights: &mut [f64]) {
        let (Some(ref l), Some(ref r)) = (&node.left, &node.right) else {
            return;
        };
        let vl = cluster_var(&l.leaves, cov);
        let vr = cluster_var(&r.leaves, cov);
        let alpha = if vl + vr > 0.0 { 1.0 - vl / (vl + vr) } else { 0.5 };
        for &k in &l.leaves {
            weights[k] *= alpha;
        }
        for &k in &r.leaves {
            weights[k] *= 1.0 - alpha;
        }
        bisect(l, cov, weights);
        bisect(r, cov, weights);
    }

    let mut weights = vec![1.0; n];
    bisect(&nodes[0], &cov, &mut weights);
    let tot: f64 = weights.iter().sum();
    let denom = if tot > 0.0 { tot } else { 1.0 };
    assets
        .iter()
        .enumerate()
        .map(|(k, a)| (a.symbol.clone(), weights[k] / denom))
        .collect()
}

fn sector_rotation_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let lb = (params.get("lookback").copied().unwrap_or(126.0) as usize).max(20);
    let ma_days = (params.get("ma_days").copied().unwrap_or(200.0) as usize).max(20);
    let top_n = (params.get("top_n").copied().unwrap_or(3.0) as usize).max(1);
    let mut off: Vec<&str> = assets.iter().filter(|a| a.role != "defensive").map(|a| a.symbol.as_str()).collect();
    let dfn: Vec<&str> = assets.iter().filter(|a| a.role == "defensive").map(|a| a.symbol.as_str()).collect();
    if off.is_empty() {
        off = assets.iter().map(|a| a.symbol.as_str()).collect();
    }

    let ret_of = |sym: &str| -> f64 {
        let cs = &closes[sym];
        if i >= lb && cs[i - lb] > 0.0 { cs[i] / cs[i - lb] - 1.0 } else { f64::NEG_INFINITY }
    };
    let risk_adj = |sym: &str| -> f64 {
        let r = ret_of(sym);
        if !r.is_finite() {
            return f64::NEG_INFINITY;
        }
        let cs = &closes[sym];
        let vw = lb.min(60);
        let start = (i + 1).saturating_sub(vw).max(1);
        let rs: Vec<f64> = (start..=i)
            .filter(|&k| cs[k - 1] > 0.0)
            .map(|k| cs[k] / cs[k - 1] - 1.0)
            .collect();
        if rs.len() < 2 {
            return r;
        }
        let mu = rs.iter().sum::<f64>() / rs.len() as f64;
        let vol = (rs.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / (rs.len() - 1) as f64).sqrt()
            * 252.0f64.sqrt();
        r / vol.max(0.05)
    };
    let above_ma = |sym: &str| -> bool {
        let cs = &closes[sym];
        if i < ma_days {
            return true;
        }
        cs[i] > cs[i + 1 - ma_days..=i].iter().sum::<f64>() / ma_days as f64
    };

    let mut ranked: Vec<(f64, &str)> = off.iter().map(|&s| (risk_adj(s), s)).collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(a.1)));
    let best_def = dfn
        .iter()
        .map(|&s| (ret_of(s), s))
        .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|x| x.1);

    let picks = &ranked[..top_n.min(ranked.len())];
    let slice_w = if picks.is_empty() { 0.0 } else { 1.0 / picks.len() as f64 };
    let mut out: HashMap<String, f64> = assets.iter().map(|a| (a.symbol.clone(), 0.0)).collect();
    for &(_, sym) in picks {
        if ret_of(sym) > 0.0 && above_ma(sym) {
            *out.entry(sym.to_string()).or_insert(0.0) += slice_w;
        } else if let Some(bd) = best_def {
            *out.entry(bd.to_string()).or_insert(0.0) += slice_w;
        }
    }
    out
}

/// Long-only covariance-inverse Max-Sharpe optimizer adapted from *151 Trading Strategies*
/// Section 3.18 (`w ∝ C⁻¹ E`) with ridge stabilization, GMV bear-market fallback (`C⁻¹ 1`),
/// and iterative single-asset weight cap (`max_weight_pct`).
fn cov_max_sharpe_weights(
    closes: &HashMap<String, Vec<f64>>,
    i: usize,
    assets: &[AssetItem],
    params: &ResolvedParams,
) -> HashMap<String, f64> {
    let n = assets.len();
    if n == 0 {
        return HashMap::new();
    }
    if n == 1 {
        return [(assets[0].symbol.clone(), 1.0)].into_iter().collect();
    }
    let cov_w = (params.get("cov_window").copied().unwrap_or(120.0) as usize).max(20);
    let mom_d = (params.get("mom_days").copied().unwrap_or(60.0) as usize).max(5);
    let max_w = (params.get("max_weight_pct").copied().unwrap_or(45.0) / 100.0)
        .clamp(1.0 / n as f64, 1.0);

    let start = (i + 1).saturating_sub(cov_w).max(1);
    let rets: Vec<Vec<f64>> = assets
        .iter()
        .map(|a| {
            let cs = &closes[&a.symbol];
            (start..=i)
                .map(|k| if cs[k - 1] > 0.0 { cs[k] / cs[k - 1] - 1.0 } else { 0.0 })
                .collect()
        })
        .collect();
    let m = rets[0].len();
    if m < 5 {
        let eq = 1.0 / n as f64;
        return assets.iter().map(|a| (a.symbol.clone(), eq)).collect();
    }

    let means: Vec<f64> = rets.iter().map(|r| r.iter().sum::<f64>() / m as f64).collect();
    let mut cov = vec![vec![0.0; n]; n];
    let mut diag_sum = 0.0;
    for a in 0..n {
        for b in a..n {
            let v: f64 = (0..m)
                .map(|t| (rets[a][t] - means[a]) * (rets[b][t] - means[b]))
                .sum::<f64>()
                / (m - 1) as f64;
            cov[a][b] = v;
            cov[b][a] = v;
        }
        diag_sum += cov[a][a].max(0.0);
    }
    let avg_var = (diag_sum / n as f64).max(1e-8);
    let ridge = (avg_var * 0.05).max(1e-6);
    for a in 0..n {
        cov[a][a] += ridge;
    }

    // Expected returns E_i from recent momentum window
    let exp_ret: Vec<f64> = assets
        .iter()
        .map(|a| {
            let cs = &closes[&a.symbol];
            if i >= mom_d && cs[i - mom_d] > 0.0 {
                cs[i] / cs[i - mom_d] - 1.0
            } else {
                0.0
            }
        })
        .collect();

    // Unconstrained Max-Sharpe: C⁻¹ E -> project to spot long-only (w >= 0)
    let mut w: Vec<f64> = solve_linear(&cov, &exp_ret)
        .unwrap_or_else(|| vec![0.0; n])
        .into_iter()
        .map(|x| x.max(0.0))
        .collect();

    let mut sum_w: f64 = w.iter().sum();
    // Bear-market / all-negative fallback: Global Minimum Variance (C⁻¹ 1)
    if sum_w <= 1e-9 {
        let ones = vec![1.0; n];
        w = solve_linear(&cov, &ones)
            .unwrap_or_else(|| (0..n).map(|a| 1.0 / cov[a][a].max(1e-8)).collect())
            .into_iter()
            .map(|x| x.max(0.0))
            .collect();
        sum_w = w.iter().sum();
    }
    if sum_w <= 1e-9 {
        w = (0..n).map(|a| 1.0 / cov[a][a].max(1e-8)).collect();
        sum_w = w.iter().sum();
    }
    for x in &mut w {
        *x /= sum_w;
    }

    // Iterative water-filling single-asset cap so low-vol assets don't dominate 95%+
    let mut capped = vec![false; n];
    for _ in 0..n {
        let mut excess = 0.0;
        let mut any_new = false;
        for a in 0..n {
            if !capped[a] && w[a] > max_w + 1e-12 {
                excess += w[a] - max_w;
                w[a] = max_w;
                capped[a] = true;
                any_new = true;
            }
        }
        if !any_new || excess <= 1e-12 {
            break;
        }
        let free_sum: f64 = (0..n).filter(|&a| !capped[a]).map(|a| w[a]).sum();
        let free_cnt = (0..n).filter(|&a| !capped[a]).count();
        if free_sum > 1e-12 {
            for a in 0..n {
                if !capped[a] {
                    w[a] += excess * (w[a] / free_sum);
                }
            }
        } else if free_cnt > 0 {
            let share = excess / free_cnt as f64;
            for a in 0..n {
                if !capped[a] {
                    w[a] += share;
                }
            }
        } else {
            break;
        }
    }
    let norm: f64 = w.iter().sum();
    let denom = if norm > 0.0 { norm } else { 1.0 };
    assets
        .iter()
        .enumerate()
        .map(|(idx, a)| (a.symbol.clone(), w[idx] / denom))
        .collect()
}

// ------------------------------------------------------------------ model registry
pub struct PortfolioModelSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub theory: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub uses_roles: bool,
    pub allowed_roles: &'static [&'static str],
    pub uses_weights: bool,
    pub default_rebalance: &'static str,
    pub params: &'static [Param],
    pub warmup_fn: fn(&ResolvedParams) -> usize,
    pub weight_fn: fn(&HashMap<String, Vec<f64>>, usize, &[AssetItem], &ResolvedParams) -> HashMap<String, f64>,
    pub presets: fn() -> Value,
    pub guide: fn() -> Value,
}

impl PortfolioModelSpec {
    pub fn resolve(&self, raw: Option<&Value>) -> Result<ResolvedParams, String> {
        let v = raw.cloned().unwrap_or_else(|| json!({}));
        resolve_params(self.params, &v)
    }

    pub fn params_json(&self, resolved: &ResolvedParams) -> Value {
        params_to_json(self.params, resolved)
    }

    pub fn meta(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "theory": self.theory,
            "summary": self.summary,
            "description": self.description,
            "guide": (self.guide)(),
            "uses_roles": self.uses_roles,
            "allowed_roles": self.allowed_roles,
            "uses_weights": self.uses_weights,
            "default_rebalance": self.default_rebalance,
            "params": self.params,
            "presets": (self.presets)(),
        })
    }
}

const EMPTY_PARAMS: &[Param] = &[];
const RP_PARAMS: &[Param] = &[Param::int("vol_window", "변동성 기간 (일)", 60.0, 10.0, 252.0)];
const GEM_PARAMS: &[Param] = &[
    Param::int("lookback", "모멘텀 기간 (일)", 252.0, 20.0, 360.0),
    Param::int("top_n", "상위 선택 개수", 1.0, 1.0, 5.0),
];
const VAA_PARAMS: &[Param] = &[Param::int("top_offensive", "공격 상위 종목 수", 1.0, 1.0, 4.0)];
const DAA_PARAMS: &[Param] = &[
    Param::int("top_offensive", "공격 상위 종목 수", 2.0, 1.0, 6.0),
    Param::int("canary_threshold", "100% 대피 감시병 수", 2.0, 1.0, 4.0),
];
const BAA_PARAMS: &[Param] = &[
    Param::int("top_offensive", "공격 상위 종목 수", 1.0, 1.0, 4.0),
    Param::int("top_defensive", "방어 분산 종목 수", 3.0, 1.0, 5.0),
];
const LAA_PARAMS: &[Param] = &[Param::int("trend_days", "시장 추세 기준선 (일)", 200.0, 50.0, 300.0)];
const HRP_PARAMS: &[Param] = &[Param::int("vol_window", "상관·변동성 기간 (일)", 60.0, 20.0, 252.0)];
const SECTOR_PARAMS: &[Param] = &[
    Param::int("lookback", "수익률 비교 기간 (일)", 126.0, 20.0, 252.0),
    Param::int("ma_days", "상승 추세 확인선 (일)", 200.0, 20.0, 252.0),
    Param::int("top_n", "상위 선택 종목 수", 3.0, 1.0, 6.0),
];
const COV_SHARPE_PARAMS: &[Param] = &[
    Param::int("cov_window", "공분산 측정 기간 (일)", 120.0, 30.0, 252.0),
    Param::int("mom_days", "기대수익 기간 (일)", 60.0, 10.0, 252.0),
    Param::int("max_weight_pct", "단일 종목 최대 비중 (%)", 45.0, 20.0, 100.0),
];

const ROLES_OD: &[&str] = &["offensive", "defensive"];
const ROLES_ODC: &[&str] = &["offensive", "defensive", "canary"];
const ROLES_COD: &[&str] = &["core", "offensive", "defensive"];

pub static PORTFOLIO_MODELS: &[PortfolioModelSpec] = &[
    PortfolioModelSpec {
        id: "all_weather",
        name: "올웨더 (All Weather)",
        theory: "레이 달리오 · 경제가 4계절(호황·불황·물가상승·물가하락) 중 어느 쪽으로 가도 버티는 분산 투자",
        summary: "주식(30%) · 장기국채(40%) · 중기국채(15%) · 금(7.5%) · 원자재(7.5%)에 나눠 담고 비율이 틀어지면 자동 정리합니다",
        description: "세계 최대 펀드를 만든 레이 달리오의 '사계절 투자법'입니다. 어떤 경제 날씨에서도 잘 버티는 자산 5가지를 정해진 비율로 섞어 두고 정기적으로 원래 비율로 되돌립니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: true,
        default_rebalance: "quarterly",
        params: EMPTY_PARAMS,
        warmup_fn: |_| 2,
        weight_fn: fixed_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 30},
                {"symbol": "TLT", "name": "미국 장기국채", "role": "defensive", "weight": 40},
                {"symbol": "IEF", "name": "미국 중기국채", "role": "defensive", "weight": 15},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 7.5},
                {"symbol": "DBC", "name": "원자재", "role": "offensive", "weight": 7.5}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 30},
                {"symbol": "148070", "name": "KOSEF 국고채10년", "role": "defensive", "weight": 40},
                {"symbol": "114260", "name": "KODEX 국고채3년", "role": "defensive", "weight": 15},
                {"symbol": "132030", "name": "KODEX 골드선물", "role": "defensive", "weight": 7.5},
                {"symbol": "261220", "name": "KODEX WTI원유", "role": "offensive", "weight": 7.5}
            ]
        }),
        guide: || json!({
            "background": "경제 4계절(호황·침체·물가상승·물가하락)에 모두 대응하도록 주식·국채·금·원자재의 위험 비중을 고르게 배분한 분산 전략입니다.",
            "setup": ["기본 비율 30/40/15/7.5/7.5, 분기 리밸런싱 또는 ±5% 편차 리밸런싱 권장"],
            "execution": ["목표 비율을 넘긴 자산을 먼저 팔고 덜 오른 자산을 매수해 원래 비율로 되돌립니다."],
            "caution": "금리 급등기(예: 2022년)에는 주식과 장기 국채가 일시 동반 하락할 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "classic_60_40",
        name: "60/40 클래식 균형",
        theory: "해리 마코위츠 · 노벨경제학상 분산 투자 이론의 가장 쉬운 정석 (주식 60 : 채권 40)",
        summary: "돈의 60%는 주식, 40%는 채권에 넣어두고 비율이 흐트러질 때마다 60:40으로 다시 맞춥니다",
        description: "수익을 키우는 주식(60%)과 하락장 쿠션 역할을 하는 채권(40%)을 함께 보유해 출렁임을 줄이면서 꾸준히 자산을 불립니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: true,
        default_rebalance: "quarterly",
        params: EMPTY_PARAMS,
        warmup_fn: |_| 2,
        weight_fn: fixed_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 60},
                {"symbol": "AGG", "name": "미국 종합채권", "role": "defensive", "weight": 40}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 60},
                {"symbol": "148070", "name": "KOSEF 국고채10년", "role": "defensive", "weight": 40}
            ]
        }),
        guide: || json!({
            "background": "서로 다르게 움직이는 주식과 채권을 60:40으로 섞어 폭락장 충격을 줄이는 전통적 분산 투자입니다.",
            "setup": ["주식 60% + 채권 40%, 분기 또는 연 1회 리밸런싱 권장"],
            "execution": ["초과한 자산부터 매도해 부족한 자산을 매수하여 60:40으로 맞춥니다."],
            "caution": "대세 상승장에서는 주식 100%보다 수익률이 낮지만 하락장 방어력이 우수합니다."
        }),
    },
    PortfolioModelSpec {
        id: "equal_weight",
        name: "동일 비중 (1/N)",
        theory: "단순함의 힘 · 복잡한 예측 대신 모든 종목을 똑같이 N분의 1씩 나누어 담기",
        summary: "담은 종목이 4개면 25%씩, 5개면 20%씩 똑같이 나누고 정기적으로 다시 N분의 1로 맞춥니다",
        description: "고른 여러 자산에 동일하게 1/N씩 배분하고 정기 리밸런싱으로 자연스럽게 고점 매도·저점 매수 효과를 얻습니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: EMPTY_PARAMS,
        warmup_fn: |_| 2,
        weight_fn: equal_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "S&P500", "role": "offensive", "weight": 25},
                {"symbol": "QQQ", "name": "나스닥100", "role": "offensive", "weight": 25},
                {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 25},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 25},
                {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25}
            ]
        }),
        guide: || json!({
            "background": "DeMiguel(2009) 연구에서 복잡한 최적화보다 강건함이 입증된 1/N 동일 비중 전략입니다.",
            "setup": ["성격이 다른 3~6개 자산 조합, 매월 또는 분기 리밸런싱 추천"],
            "execution": ["전체 평가금액을 종목 수로 균등 분할해 초과분 매도 후 부족분 매수"],
            "caution": "레버리지·초고변동성 종목을 섞으면 해당 종목에 계좌가 좌우될 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "risk_parity",
        name: "리스크 패리티 (역변동성)",
        theory: "에드워드 치엔 · 금액이 아니라 '출렁이는 크기(위험)'를 똑같이 나누기",
        summary: "최근에 많이 출렁인 종목은 적게 담고, 잔잔한 종목은 많이 담아 계좌 흔들림을 고르게 맞춥니다",
        description: "최근 N일 변동성에 반비례하도록 비중을 자동 배분해 모든 자산의 위험 기여도를 균등하게 맞춥니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: RP_PARAMS,
        warmup_fn: |p| p.get("vol_window").copied().unwrap_or(60.0) as usize + 1,
        weight_fn: risk_parity_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "S&P500", "role": "offensive", "weight": 25},
                {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 25},
                {"symbol": "IEF", "name": "중기국채", "role": "defensive", "weight": 25},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                {"symbol": "114260", "name": "국고채3년", "role": "defensive", "weight": 25},
                {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25}
            ]
        }),
        guide: || json!({
            "background": "변동성이 큰 자산은 적게, 안정적인 자산은 많이 담아 계좌 흔들림을 균등화합니다.",
            "setup": ["변동성 기간 60일, 매월 리밸런싱 권장"],
            "execution": ["최근 60일 일간 변동성의 역수 비율로 목표 비중을 계산해 리밸런싱합니다."],
            "caution": "초단기 파킹형 ETF를 넣으면 변동성이 너무 낮아 비중이 과도하게 몰릴 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "dual_momentum",
        name: "글로벌 듀얼 모멘텀 (GEM)",
        theory: "게리 안토나치 · ① 잘나가는 1등 주식 고르기 + ② 분위기 나쁘면 안전 채권으로 대피하기",
        summary: "공격 자산 중 최근 수익률 1등을 고르고, 그마저 마이너스면 안전한 채권으로 100% 피신합니다",
        description: "공격 자산 간 상대 모멘텀으로 승자를 고르고, 절대 모멘텀이 마이너스면 방어 채권으로 대피하는 게리 안토나치 전략입니다.",
        uses_roles: true,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: GEM_PARAMS,
        warmup_fn: |p| p.get("lookback").copied().unwrap_or(252.0) as usize + 1,
        weight_fn: dual_momentum_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 주식", "role": "offensive", "weight": 50},
                {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 50},
                {"symbol": "AGG", "name": "미국 종합채권 (방어)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "069500", "name": "국내 코스피200", "role": "offensive", "weight": 50},
                {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 50},
                {"symbol": "148070", "name": "국고채10년 (방어)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "상대 모멘텀(1등 공격자산)과 절대 모멘텀(하락장 채권 대피)을 결합해 큰 폭락장을 회피합니다.",
            "setup": ["공격 2개 이상 + 방어 1개 이상 지정, 모멘텀 기간 252일, 매월 초 리밸런싱 권장"],
            "execution": ["매월 초 최근 1년 수익률 1등 공격자산이 양수면 투자, 음수면 방어자산으로 100% 대피"],
            "caution": "급락 후 V자 급반등 구간에서는 복귀가 한 달 늦어질 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "vaa",
        name: "VAA 경계 자산배분",
        theory: "켈러 & 큐닝 · '광산의 카나리아'처럼 위험 신호가 하나라도 울리면 즉시 안전지대로 대피",
        summary: "공격 자산 4개가 모두 상승세일 때만 투자하고, 1개라도 꺾이면 즉시 안전 채권으로 100% 대피합니다",
        description: "13612W 모멘텀 점수로 공격 자산 전체가 양수일 때만 공격 1등에 투자하고, 하나라도 음수면 즉시 최고 방어 채권으로 대피합니다.",
        uses_roles: true,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: VAA_PARAMS,
        warmup_fn: |_| 253,
        weight_fn: vaa_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 주식", "role": "offensive", "weight": 25},
                {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 25},
                {"symbol": "VWO", "name": "신흥국 주식", "role": "offensive", "weight": 25},
                {"symbol": "AGG", "name": "종합채권", "role": "offensive", "weight": 25},
                {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "LQD", "name": "회사채 (방어)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 25},
                {"symbol": "148070", "name": "국고채10년", "role": "offensive", "weight": 25},
                {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 25},
                {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "1·3·6·12개월 가중 모멘텀(13612W)으로 위험을 조기 포착해 하락장 최대 낙폭을 강하게 억제합니다.",
            "setup": ["공격 4종 + 방어 2~3종, 매월 초 리밸런싱 고정 권장"],
            "execution": ["공격 자산 전부가 양수면 1등 공격자산 투자, 1개라도 음수면 1등 방어채권으로 100% 대피"],
            "caution": "전량 교체가 잦을 수 있으므로 수수료가 낮고 유동성이 풍부한 대표 ETF를 사용하세요."
        }),
    },
    PortfolioModelSpec {
        id: "daa",
        name: "DAA 방어적 자산배분",
        theory: "켈러 & 큐닝 · 감시병 2명의 건강 상태에 따라 0% → 50% → 100% 단계별 대피",
        summary: "감시병(신흥국·채권)이 1개 꺾이면 절반만 대피, 2개 다 꺾이면 안전 채권으로 100% 피신합니다",
        description: "카나리아 감시병 2종과 투자 종목을 분리하고, 감시병 훼손 개수에 따라 단계적으로 안전 채권 비중을 늘리는 방어형 전략입니다.",
        uses_roles: true,
        allowed_roles: ROLES_ODC,
        uses_weights: false,
        default_rebalance: "monthly",
        params: DAA_PARAMS,
        warmup_fn: |_| 253,
        weight_fn: daa_weights,
        presets: || json!({
            "US": [
                {"symbol": "VWO", "name": "신흥국 주식 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "BND", "name": "미국 종합채권 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 0},
                {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                {"symbol": "IWM", "name": "미국 소형주", "role": "offensive", "weight": 0},
                {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 0},
                {"symbol": "GLD", "name": "금", "role": "offensive", "weight": 0},
                {"symbol": "VNQ", "name": "미국 리츠", "role": "offensive", "weight": 0},
                {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "TLT", "name": "장기국채 (방어)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "360750", "name": "미국 S&P500 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "148070", "name": "국고채10년 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 0},
                {"symbol": "133690", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 0},
                {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
                {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "감시병(canary) 자산의 음수 전환 개수에 비례해 0%→50%→100% 단계적으로 방어 자산에 배분합니다.",
            "setup": ["감시병 2종, 공격 4~8종, 방어 2~3종 권장"],
            "execution": ["매월 초 감시병 점수를 확인해 공격 상위 종목과 방어 1등 자산 비중을 조절합니다."],
            "caution": "감시병 자산 자체는 매수 대상이 아닙니다."
        }),
    },
    PortfolioModelSpec {
        id: "baa",
        name: "BAA 대담한 자산배분",
        theory: "켈러 & 큐닝 (2022) · 금리 급등기에도 단기채·원자재로 피하는 최신 방어 모델",
        summary: "감시병 4종이 모두 좋을 때만 공격 투자, 1개라도 꺾이면 1년 평균보다 강한 안전자산 3곳으로 대피합니다",
        description: "감시병 4종 중 하나라도 음수면 초단기채·물가연동채·원자재 등 방어 자산 상위 3종으로 분산 대피하는 2022년 최신 모델입니다.",
        uses_roles: true,
        allowed_roles: ROLES_ODC,
        uses_weights: false,
        default_rebalance: "monthly",
        params: BAA_PARAMS,
        warmup_fn: |_| 253,
        weight_fn: baa_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국주식 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "VEA", "name": "선진국 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "VWO", "name": "신흥국 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "BND", "name": "미국채권 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                {"symbol": "VTI", "name": "미국 전체주식", "role": "offensive", "weight": 0},
                {"symbol": "VXUS", "name": "글로벌 해외주식", "role": "offensive", "weight": 0},
                {"symbol": "VNQ", "name": "미국 리츠", "role": "offensive", "weight": 0},
                {"symbol": "BIL", "name": "초단기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "TIP", "name": "물가연동채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "DBC", "name": "원자재 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "TLT", "name": "장기국채 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "LQD", "name": "회사채 (방어)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "360750", "name": "미국 S&P500 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "148070", "name": "국고채10년 (감시병)", "role": "canary", "weight": 0},
                {"symbol": "133690", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 0},
                {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
                {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "261220", "name": "WTI원유 (방어)", "role": "defensive", "weight": 0},
                {"symbol": "305080", "name": "미국채10년선물 (방어)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "금리 급등·고물가 장세에서도 초단기채·물가채·원자재 등으로 피신해 방어력을 극대화한 BAA 모델입니다.",
            "setup": ["감시병 4종, 공격 3~4종, 방어 4~6종 구성 권장"],
            "execution": ["감시병 전부 양수면 SMA12 비율 1등 공격자산, 하나라도 음수면 방어 상위 3종에 분산 투자"],
            "caution": "급격한 반등장 초기에는 한 달 늦게 공격 자산으로 복귀할 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "laa",
        name: "LAA 게으른 자산배분",
        theory: "켈러 & 브룩스 · 75%는 가만히 두고, 25%만 시장 분위기에 따라 갈아타기",
        summary: "75%(주식·채권·금 각 25%)는 고정하고, 나머지 25%만 상승장엔 나스닥·하락장엔 단기채로 바꿉니다",
        description: "자산의 75%는 고정 보유하고 나머지 25%만 대표 지수의 200일 평균선 위치에 따라 공격/방어로 전환하는 저회전율 전략입니다.",
        uses_roles: true,
        allowed_roles: ROLES_COD,
        uses_weights: true,
        default_rebalance: "monthly",
        params: LAA_PARAMS,
        warmup_fn: |p| p.get("trend_days").copied().unwrap_or(200.0) as usize + 1,
        weight_fn: laa_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500 (고정·기준)", "role": "core", "weight": 25},
                {"symbol": "IEF", "name": "미국 중기국채 (고정)", "role": "core", "weight": 25},
                {"symbol": "GLD", "name": "금 (고정)", "role": "core", "weight": 25},
                {"symbol": "QQQ", "name": "나스닥100 (상승장 25%)", "role": "offensive", "weight": 0},
                {"symbol": "SHY", "name": "단기국채 (하락장 25%)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "360750", "name": "미국 S&P500 (고정·기준)", "role": "core", "weight": 25},
                {"symbol": "148070", "name": "국고채10년 (고정)", "role": "core", "weight": 25},
                {"symbol": "132030", "name": "골드선물 (고정)", "role": "core", "weight": 25},
                {"symbol": "133690", "name": "미국 나스닥100 (상승장 25%)", "role": "offensive", "weight": 0},
                {"symbol": "153130", "name": "단기채권 (하락장 25%)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "75% 영구 분산 코어 + 25% 추세 타이밍 조합으로 매매 횟수를 최소화합니다.",
            "setup": ["고정(core) 합계 75%, 첫 번째 고정 종목이 200일 추세 판별 기준"],
            "execution": ["매월 초 기준 지수가 200일선 위면 공격 25%, 아래면 단기채 25%로 배분"],
            "caution": "200일선 부근 횡보 시 25% 몫이 몇 달 연속 바뀔 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "permanent",
        name: "영구 포트폴리오 (25×4)",
        theory: "해리 브라운 · 주식·장기채·금·현금(단기채) 딱 4등분으로 어떤 위기에도 버티기",
        summary: "주식 25% · 장기국채 25% · 금 25% · 초단기채(현금성) 25%를 똑같이 나누어 유지합니다",
        description: "주식·장기국채·금·초단기채 4개 자산을 정확히 25%씩 유지해 어떤 경제 위기에도 낮은 낙폭으로 버티는 방어형 포트폴리오입니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: true,
        default_rebalance: "yearly",
        params: EMPTY_PARAMS,
        warmup_fn: |_| 2,
        weight_fn: fixed_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 25},
                {"symbol": "TLT", "name": "미국 장기국채", "role": "defensive", "weight": 25},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25},
                {"symbol": "SHY", "name": "초단기국채 (현금성)", "role": "defensive", "weight": 25}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25},
                {"symbol": "153130", "name": "단기채권 (현금성)", "role": "defensive", "weight": 25}
            ]
        }),
        guide: || json!({
            "background": "해리 브라운의 25%×4 등분 영구 포트폴리오로, 현금성 단기채와 금이 50%를 차지해 체감 낙폭이 매우 작습니다.",
            "setup": ["4개 자산 각 25%, 매년 초 또는 편차 ±10% 리밸런싱 권장"],
            "execution": ["연 1회 25%를 넘긴 자산을 팔아 부족한 자산을 보충합니다."],
            "caution": "주식 강세장에서는 수익률이 완만할 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "hrp",
        name: "HRP 계층적 위험 분산",
        theory: "로페스 데 프라도 (2016) · 비슷하게 움직이는 종목끼리 묶어서 중복 위험 차단",
        summary: "출렁임 크기뿐 아니라 '같이 떨어지는 성질'까지 계산해 비슷한 종목에 돈이 쏠리지 않게 자동 배분합니다",
        description: "상관관계 기반 계층적 클러스터링으로 비슷하게 움직이는 자산끼리 묶은 뒤 재귀 이분할로 진정한 위험 분산을 수행합니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: HRP_PARAMS,
        warmup_fn: |p| p.get("vol_window").copied().unwrap_or(60.0) as usize + 1,
        weight_fn: hrp_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 20},
                {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 20},
                {"symbol": "IEF", "name": "중기국채", "role": "defensive", "weight": 20},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 20}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 20},
                {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 20},
                {"symbol": "114260", "name": "국고채3년", "role": "defensive", "weight": 20},
                {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 20}
            ]
        }),
        guide: || json!({
            "background": "López de Prado(2016)의 HRP 알고리즘으로 상관관계 높은 종목들의 중복 위험을 자동 억제합니다.",
            "setup": ["상관·변동성 기간 60일, 성격이 겹치는 종목과 방어 자산 혼합 시 효과 극대화"],
            "execution": ["매월 초 상관행렬 클러스터링 + 역분산 재귀 이분할로 목표 비중을 산출합니다."],
            "caution": "초단기 파킹형 ETF를 넣으면 그쪽으로 비중이 과도하게 쏠릴 수 있습니다."
        }),
    },
    PortfolioModelSpec {
        id: "sector_rotation",
        name: "섹터·종목 모멘텀 로테이션",
        theory: "메브 파버 · 요즘 가장 잘나가는 업종(종목) TOP 3로 매달 자동 갈아타기",
        summary: "여러 섹터·우량주 중 '출렁임 대비 가장 잘 오른 상위 N개'를 고르고, 200일선 아래로 꺾인 몫은 안전 채권으로 대피합니다",
        description: "위험조정 모멘텀 상위 N개 업종·종목에 분산 투자하고, 추세선(200일선) 이탈 몫은 방어 채권으로 자동 대피합니다.",
        uses_roles: true,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: SECTOR_PARAMS,
        warmup_fn: |p| {
            let lb = p.get("lookback").copied().unwrap_or(126.0) as usize;
            let ma = p.get("ma_days").copied().unwrap_or(200.0) as usize;
            lb.max(ma) + 1
        },
        weight_fn: sector_rotation_weights,
        presets: || json!({
            "US": [
                {"symbol": "XLK", "name": "기술 (IT)", "role": "offensive", "weight": 0},
                {"symbol": "XLV", "name": "헬스케어", "role": "offensive", "weight": 0},
                {"symbol": "XLF", "name": "금융", "role": "offensive", "weight": 0},
                {"symbol": "XLE", "name": "에너지", "role": "offensive", "weight": 0},
                {"symbol": "XLI", "name": "산업재", "role": "offensive", "weight": 0},
                {"symbol": "XLY", "name": "경기소비재", "role": "offensive", "weight": 0},
                {"symbol": "XLP", "name": "필수소비재", "role": "offensive", "weight": 0},
                {"symbol": "XLU", "name": "유틸리티", "role": "offensive", "weight": 0},
                {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0}
            ],
            "KR": [
                {"symbol": "091160", "name": "반도체", "role": "offensive", "weight": 0},
                {"symbol": "091170", "name": "은행", "role": "offensive", "weight": 0},
                {"symbol": "091180", "name": "자동차", "role": "offensive", "weight": 0},
                {"symbol": "102970", "name": "증권", "role": "offensive", "weight": 0},
                {"symbol": "244580", "name": "바이오", "role": "offensive", "weight": 0},
                {"symbol": "305720", "name": "2차전지산업", "role": "offensive", "weight": 0},
                {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
                {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0}
            ]
        }),
        guide: || json!({
            "background": "메브 파버의 섹터 로테이션과 200일 이동평균 방어 필터를 결합한 주도주 순환 전략입니다.",
            "setup": ["공격 업종 ETF/우량주 5~10종 + 방어 단기채 1종 이상, 기간 126일/200일, 상위 3종 권장"],
            "execution": ["매월 초 위험조정 수익률 상위 3종을 선정하고 200일선 하회 몫은 단기채에 배분합니다."],
            "caution": "단기 혼조 장세에서는 주도주 교체가 잦을 수 있으므로 중장기 운용을 권장합니다."
        }),
    },
    PortfolioModelSpec {
        id: "cov_max_sharpe",
        name: "[151전략] 공분산 최대 샤프 최적화 (C⁻¹E)",
        theory: "《151 Trading Strategies》 3.18절 응용 · 공분산 역행렬로 변동성과 상관관계를 자동 조절해 샤프 극대화",
        summary: "종목 간 함께 흔들리는 정도(공분산)와 최근 상승 힘을 수학적으로 계산해 위험 대비 수익이 가장 높은 비중으로 자동 배분합니다",
        description: "헤지펀드 통계적 차익거래의 핵심 수식(w ∝ C⁻¹E)을 국내·미국 현물 주식(공매도 없음)에 맞게 응용한 모델입니다. 변동성이 크거나 서로 똑같이 움직이는 종목 비중은 자동으로 줄이고, 모든 자산이 하락세일 때는 최소분산(GMV) 안전 비중으로 자동 전환합니다.",
        uses_roles: false,
        allowed_roles: ROLES_OD,
        uses_weights: false,
        default_rebalance: "monthly",
        params: COV_SHARPE_PARAMS,
        warmup_fn: |p| {
            let cw = p.get("cov_window").copied().unwrap_or(120.0) as usize;
            let md = p.get("mom_days").copied().unwrap_or(60.0) as usize;
            cw.max(md) + 1
        },
        weight_fn: cov_max_sharpe_weights,
        presets: || json!({
            "US": [
                {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 20},
                {"symbol": "TLT", "name": "미국 장기국채", "role": "defensive", "weight": 20},
                {"symbol": "IEF", "name": "미국 중기국채", "role": "defensive", "weight": 20},
                {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 20}
            ],
            "KR": [
                {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 20},
                {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                {"symbol": "091160", "name": "KODEX 반도체", "role": "offensive", "weight": 20},
                {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 20},
                {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 20}
            ]
        }),
        guide: || json!({
            "background": "《151 Trading Strategies》 3.18절의 공분산 역행렬 최대 샤프 공식(w ∝ C⁻¹E)에 리지 안정화와 현물 롱-온리 제약, 하락장 최소분산(GMV) 자동 전환을 결합했습니다.",
            "setup": [
                "공분산 측정 기간(기본 120일): 최근 반년간 종목들이 얼마나 같이 흔들렸는지 계산합니다.",
                "기대수익 기간(기본 60일): 최근 분기 상승 힘을 기대수익 추정치로 사용합니다.",
                "단일 종목 최대 비중(기본 45%): 한 종목에 지나치게 쏠리지 않도록 상한을 강제합니다."
            ],
            "execution": [
                "매월 초 리지 정규화된 공분산 역행렬과 기대수익으로 최적 비중을 산출합니다.",
                "모든 종목이 하락세면 자동으로 최소분산(변동성이 가장 낮은 채권·금 위주) 비중으로 방어합니다."
            ],
            "caution": "패닉 급락장에서는 자산 간 상관관계가 일시적으로 1에 수렴할 수 있으므로 주식·채권·금처럼 성격이 다른 자산을 함께 담으세요."
        }),
    },
];

pub fn get_model(id: &str) -> Option<&'static PortfolioModelSpec> {
    PORTFOLIO_MODELS.iter().find(|m| m.id == id)
}

pub fn validate_assets(
    market_str: &str,
    raw_assets: Option<&Value>,
    model: &PortfolioModelSpec,
) -> ApiResult<Vec<AssetItem>> {
    let market = Market::parse(market_str)?;
    let arr = raw_assets
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_assets",
                format!("자산은 {MIN_ASSETS}~{MAX_ASSETS}개여야 합니다"),
            )
        })?;
    if !(MIN_ASSETS..=MAX_ASSETS).contains(&arr.len()) {
        return Err(ApiError::bad_request(
            "invalid_assets",
            format!("자산은 {MIN_ASSETS}~{MAX_ASSETS}개여야 합니다"),
        ));
    }
    let mut seen = HashSet::new();
    let mut clean = Vec::with_capacity(arr.len());
    for item in arr {
        let obj = item
            .as_object()
            .ok_or_else(|| ApiError::bad_request("invalid_assets", "각 자산은 객체여야 합니다"))?;
        let sym = obj
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_uppercase();
        if !market.validate_symbol(&sym) {
            let disp = if sym.is_empty() { "(빈 값)" } else { &sym };
            return Err(ApiError::bad_request(
                "invalid_symbol",
                format!("{disp}은(는) {market_str} 종목 코드가 아닙니다"),
            ));
        }
        if !seen.insert(sym.clone()) {
            return Err(ApiError::bad_request(
                "duplicate_symbol",
                format!("중복된 종목입니다: {sym}"),
            ));
        }
        let role = obj
            .get("role")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("offensive")
            .to_lowercase();
        if !VALID_ROLES.contains(&role.as_str()) {
            return Err(ApiError::bad_request(
                "invalid_role",
                format!("자산 역할은 {} 중 하나여야 합니다", VALID_ROLES.join(", ")),
            ));
        }
        if model.uses_roles && !model.allowed_roles.contains(&role.as_str()) {
            return Err(ApiError::bad_request(
                "invalid_role",
                format!(
                    "이 모델에서 허용하는 역할은 {}입니다",
                    model.allowed_roles.join(", ")
                ),
            ));
        }
        let weight = match obj.get("weight") {
            None | Some(Value::Null) => 0.0,
            Some(v) => v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| {
                    ApiError::bad_request("invalid_weight", format!("{sym}: 비중은 숫자여야 합니다"))
                })?,
        };
        if weight < 0.0 || !weight.is_finite() {
            return Err(ApiError::bad_request(
                "invalid_weight",
                format!("{sym}: 비중은 0 이상이어야 합니다"),
            ));
        }
        let name: String = obj
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&sym)
            .chars()
            .take(40)
            .collect();
        clean.push(AssetItem {
            symbol: sym,
            name,
            role,
            weight,
        });
    }

    if model.uses_weights && !model.uses_roles && clean.iter().map(|a| a.weight).sum::<f64>() <= 0.0 {
        return Err(ApiError::bad_request(
            "invalid_weight",
            "목표 비중 합계는 0보다 커야 합니다",
        ));
    }
    if model.uses_roles {
        if !clean.iter().any(|a| a.role == "offensive") {
            return Err(ApiError::bad_request(
                "invalid_assets",
                "공격(offensive) 자산이 최소 1개 필요합니다",
            ));
        }
        if !clean.iter().any(|a| a.role == "defensive") {
            return Err(ApiError::bad_request(
                "invalid_assets",
                "방어(defensive) 자산이 최소 1개 필요합니다",
            ));
        }
        if model.allowed_roles.contains(&"canary") && !clean.iter().any(|a| a.role == "canary") {
            return Err(ApiError::bad_request(
                "invalid_assets",
                "감시병(canary) 자산이 최소 1개 필요합니다",
            ));
        }
        if model.allowed_roles.contains(&"core") {
            let core_sum: f64 = clean
                .iter()
                .filter(|a| a.role == "core")
                .map(|a| a.weight)
                .sum();
            if !clean.iter().any(|a| a.role == "core") {
                return Err(ApiError::bad_request(
                    "invalid_assets",
                    "고정(core) 자산이 최소 1개 필요합니다",
                ));
            }
            if core_sum <= 0.0 || core_sum > 100.0 {
                return Err(ApiError::bad_request(
                    "invalid_weight",
                    "고정(core) 자산 비중 합계는 0보다 크고 100 이하여야 합니다",
                ));
            }
        }
    }
    Ok(clean)
}

pub fn align_bars(
    bars_by_sym: &HashMap<String, Bars>,
) -> (Vec<i64>, HashMap<String, Vec<f64>>, HashMap<String, Vec<f64>>) {
    let mut maps: HashMap<String, HashMap<String, (f64, f64)>> = HashMap::new();
    let mut day_ts: HashMap<String, i64> = HashMap::new();

    for (sym, b) in bars_by_sym {
        let mut m = HashMap::new();
        for idx in 0..b.len() {
            let d = day_str(b.time[idx]);
            m.insert(d.clone(), (b.open[idx], b.close[idx]));
            day_ts.entry(d.clone()).or_insert_with(|| {
                NaiveDate::parse_from_str(&d, "%Y-%m-%d")
                    .ok()
                    .and_then(|nd| nd.and_hms_opt(0, 0, 0))
                    .map(|ndt| Utc.from_utc_datetime(&ndt).timestamp())
                    .unwrap_or(b.time[idx])
            });
        }
        maps.insert(sym.clone(), m);
    }

    let mut common: Option<BTreeSet<String>> = None;
    for m in maps.values() {
        let keys: BTreeSet<String> = m.keys().cloned().collect();
        common = Some(match common {
            None => keys,
            Some(prev) => prev.intersection(&keys).cloned().collect(),
        });
    }
    let common_days: Vec<String> = common.unwrap_or_default().into_iter().collect();
    let times: Vec<i64> = common_days.iter().map(|d| day_ts[d]).collect();
    let mut opens = HashMap::new();
    let mut closes = HashMap::new();
    for sym in bars_by_sym.keys() {
        let m = &maps[sym];
        opens.insert(sym.clone(), common_days.iter().map(|d| m[d].0).collect());
        closes.insert(sym.clone(), common_days.iter().map(|d| m[d].1).collect());
    }
    (times, opens, closes)
}

#[allow(clippy::too_many_arguments)]
pub fn run_portfolio_backtest(
    bars_by_sym: &HashMap<String, Bars>,
    model_id: &str,
    assets: &[AssetItem],
    params: &ResolvedParams,
    rebalance: &str,
    drift_pct: f64,
    capital: f64,
    costs: Costs,
    start_ts: i64,
    buy_only: bool,
) -> ApiResult<Value> {
    let model = get_model(model_id)
        .ok_or_else(|| ApiError::bad_request("unknown_model", "알 수 없는 포트폴리오 이론입니다"))?;
    let (times, opens, closes) = align_bars(bars_by_sym);
    let warm = (model.warmup_fn)(params);
    let first = times
        .iter()
        .position(|&t| t >= start_ts)
        .unwrap_or(times.len());
    let start = first.max(warm);
    if times.len() < 3 || start >= times.len() - 2 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "포트폴리오 백테스트에 필요한 공통 거래일이 부족합니다",
        ));
    }

    let syms: Vec<String> = assets.iter().map(|a| a.symbol.clone()).collect();
    let comm = costs.commission_pct / 100.0;
    let tax = costs.tax_pct / 100.0;
    let slip = costs.slippage_pct / 100.0;

    let mut cash = capital;
    let mut holdings: HashMap<String, i64> = syms.iter().map(|s| (s.clone(), 0)).collect();
    let mut last_rebal: Option<String> = None;
    let mut pending_target: Option<(HashMap<String, f64>, String)> = None;
    let mut equity: Vec<f64> = Vec::with_capacity(times.len() - start);
    let mut rebalances: Vec<Value> = Vec::new();
    let mut total_trades = 0usize;

    for j in start..times.len() {
        let day = day_str(times[j]);
        if let Some((target_w, reason)) = pending_target.take() {
            let val_open = cash
                + syms
                    .iter()
                    .map(|s| holdings[s] as f64 * opens[s][j])
                    .sum::<f64>();
            let mut desired: HashMap<String, i64> = HashMap::new();
            if buy_only {
                let mut deficits: HashMap<String, f64> = HashMap::new();
                for s in &syms {
                    let tw = target_w.get(s).copied().unwrap_or(0.0);
                    let cur_val = holdings[s] as f64 * opens[s][j];
                    deficits.insert(s.clone(), (val_open * tw - cur_val).max(0.0));
                }
                let sum_def: f64 = deficits.values().sum();
                for s in &syms {
                    let px_buy = opens[s][j] * (1.0 + slip) * (1.0 + comm);
                    let alloc = if sum_def > 0.0 {
                        cash * deficits[s] / sum_def
                    } else {
                        0.0
                    };
                    let add = if px_buy > 0.0 && alloc > 0.0 {
                        (alloc / px_buy).floor() as i64
                    } else {
                        0
                    };
                    desired.insert(s.clone(), holdings[s] + add);
                }
            } else {
                for s in &syms {
                    let w = target_w.get(s).copied().unwrap_or(0.0);
                    let px_buy = opens[s][j] * (1.0 + slip) * (1.0 + comm);
                    let qty = if w > 0.0 && px_buy > 0.0 {
                        (val_open * w / px_buy).floor() as i64
                    } else {
                        0
                    };
                    desired.insert(s.clone(), qty);
                }
            }

            let mut traded_val = 0.0;
            let mut n_orders = 0usize;
            for s in &syms {
                let diff = desired[s] - holdings[s];
                if diff < 0 {
                    let qty = -diff;
                    let px = opens[s][j] * (1.0 - slip);
                    let gross = qty as f64 * px;
                    cash += gross * (1.0 - comm - tax);
                    *holdings.get_mut(s).unwrap() -= qty;
                    traded_val += gross;
                    n_orders += 1;
                }
            }
            for s in &syms {
                let diff = desired[s] - holdings[s];
                if diff > 0 {
                    let px = opens[s][j] * (1.0 + slip);
                    let unit_cost = px * (1.0 + comm);
                    let max_buy = if unit_cost > 0.0 {
                        (cash / unit_cost).floor() as i64
                    } else {
                        0
                    };
                    let qty = diff.min(max_buy);
                    if qty > 0 {
                        cash -= qty as f64 * unit_cost;
                        *holdings.get_mut(s).unwrap() += qty;
                        traded_val += qty as f64 * px;
                        n_orders += 1;
                    }
                }
            }

            last_rebal = Some(day.clone());
            total_trades += n_orders;
            let mut w_map = Map::new();
            for s in &syms {
                let w = target_w.get(s).copied().unwrap_or(0.0);
                if w > 0.0 {
                    w_map.insert(s.clone(), json!((w * 10000.0).round() / 100.0));
                }
            }
            let turnover = if val_open > 0.0 {
                (traded_val / val_open * 10000.0).round() / 100.0
            } else {
                0.0
            };
            rebalances.push(json!({
                "time": times[j],
                "date": day,
                "reason": reason,
                "trades": n_orders,
                "turnover_pct": turnover,
                "weights": Value::Object(w_map),
            }));
        }

        let eq = cash
            + syms
                .iter()
                .map(|s| holdings[s] as f64 * closes[s][j])
                .sum::<f64>();
        equity.push(eq);

        let cur_w: HashMap<String, f64> = syms
            .iter()
            .map(|s| {
                (
                    s.clone(),
                    if eq > 0.0 {
                        holdings[s] as f64 * closes[s][j] / eq
                    } else {
                        0.0
                    },
                )
            })
            .collect();
        let tgt_w = (model.weight_fn)(&closes, j, assets, params);
        let next_day = if j + 1 < times.len() {
            day_str(times[j + 1])
        } else {
            day
        };
        if let Some(reason) = should_rebalance(
            &next_day,
            last_rebal.as_deref(),
            rebalance,
            &cur_w,
            &tgt_w,
            drift_pct,
        ) {
            if j + 1 < times.len() {
                pending_target = Some((tgt_w, reason));
            }
        }
    }

    let mut bench_syms: Vec<String> = assets
        .iter()
        .filter(|a| a.role != "canary")
        .map(|a| a.symbol.clone())
        .collect();
    if bench_syms.is_empty() {
        bench_syms = syms.clone();
    }
    let eq_w = 1.0 / bench_syms.len() as f64;
    let mut bh_qty: HashMap<String, i64> = HashMap::new();
    let mut bh_cash = capital;
    for s in &bench_syms {
        let px = closes[s][start] * (1.0 + slip) * (1.0 + comm);
        let q = if px > 0.0 {
            (capital * eq_w / px).floor() as i64
        } else {
            0
        };
        bh_qty.insert(s.clone(), q);
        bh_cash -= q as f64 * px;
    }
    let bench: Vec<f64> = (start..times.len())
        .map(|j| {
            bh_cash
                + bench_syms
                    .iter()
                    .map(|s| bh_qty[s] as f64 * closes[s][j])
                    .sum::<f64>()
        })
        .collect();

    let seg_times = &times[start..];
    let latest_target = (model.weight_fn)(&closes, times.len() - 1, assets, params);
    let mut cur_w_json = Map::new();
    for s in &syms {
        let w = latest_target.get(s).copied().unwrap_or(0.0);
        cur_w_json.insert(s.clone(), json!((w * 10000.0).round() / 100.0));
    }

    let mut metrics = performance(seg_times, &equity);
    if let Some(m) = metrics.as_object_mut() {
        m.insert("rebalances".to_string(), json!(rebalances.len()));
        m.insert("trades".to_string(), json!(total_trades));
    }
    let dd = drawdown_series(&equity);
    let r4 = |x: f64| (x * 10000.0).round() / 10000.0;
    let eq_pts: Vec<Value> = seg_times
        .iter()
        .enumerate()
        .map(|(idx, &t)| {
            json!({
                "time": t,
                "equity": r4(equity[idx]),
                "benchmark": r4(bench[idx]),
                "drawdown": r4(dd[idx] * 100.0),
            })
        })
        .collect();

    let warnings = if start > first && start_ts > 0 {
        vec!["start_adjusted"]
    } else {
        vec![]
    };

    Ok(json!({
        "equity": eq_pts,
        "metrics": metrics,
        "benchmark": performance(seg_times, &bench),
        "rebalances": rebalances,
        "current_weights": Value::Object(cur_w_json),
        "period": { "start": seg_times[0], "end": seg_times[seg_times.len() - 1] },
        "warnings": warnings,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cov_max_sharpe_respects_long_only_cap_and_gmv_bear_fallback() {
        let assets = vec![
            AssetItem { symbol: "STK".into(), name: "Stock".into(), role: "offensive".into(), weight: 33.0 },
            AssetItem { symbol: "TECH".into(), name: "Tech".into(), role: "offensive".into(), weight: 33.0 },
            AssetItem { symbol: "BOND".into(), name: "Bond".into(), role: "defensive".into(), weight: 34.0 },
        ];
        let n = 160usize;
        // Bull case: STK & TECH rally with higher vol, BOND steady low vol
        let mut bull = HashMap::new();
        bull.insert("STK".into(), (0..n).map(|i| 100.0 + 0.45 * i as f64 + 3.0 * (i as f64 * 0.4).sin()).collect());
        bull.insert("TECH".into(), (0..n).map(|i| 100.0 + 0.65 * i as f64 + 6.0 * (i as f64 * 0.45).sin()).collect());
        bull.insert("BOND".into(), (0..n).map(|i| 100.0 + 0.05 * i as f64 + 0.2 * (i as f64 * 0.2).cos()).collect());
        let p = resolve_params(COV_SHARPE_PARAMS, &json!({"cov_window": 120, "mom_days": 60, "max_weight_pct": 45})).unwrap();
        let w_bull = cov_max_sharpe_weights(&bull, n - 1, &assets, &p);
        let sum_b: f64 = w_bull.values().sum();
        assert!((sum_b - 1.0).abs() < 1e-9, "bull weights sum != 1: {sum_b}");
        for (k, &v) in &w_bull {
            assert!(v >= 0.0 && v <= 0.45 + 1e-6, "bull weight {k}={v} out of [0, 0.45]");
        }

        // Bear case: ALL assets down -> falls back to GMV (lowest-vol BOND hits cap 0.45)
        let mut bear = HashMap::new();
        bear.insert("STK".into(), (0..n).map(|i| 200.0 - 0.5 * i as f64 + 5.0 * (i as f64 * 0.5).sin()).collect());
        bear.insert("TECH".into(), (0..n).map(|i| 200.0 - 0.8 * i as f64 + 8.0 * (i as f64 * 0.6).sin()).collect());
        bear.insert("BOND".into(), (0..n).map(|i| 100.0 - 0.02 * i as f64 + 0.1 * (i as f64 * 0.2).cos()).collect());
        let w_bear = cov_max_sharpe_weights(&bear, n - 1, &assets, &p);
        let sum_r: f64 = w_bear.values().sum();
        assert!((sum_r - 1.0).abs() < 1e-9, "bear weights sum != 1: {sum_r}");
        assert!(
            w_bear["BOND"] >= w_bear["TECH"],
            "GMV bear fallback should favor low-vol BOND over high-vol TECH: {w_bear:?}"
        );
    }
}
