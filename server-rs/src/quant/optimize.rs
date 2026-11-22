//! Parameter grid search (1D/2D) parallelized across CPU cores with `rayon`.

use rayon::prelude::*;
use serde_json::{json, Map, Value};

use crate::errors::{ApiError, ApiResult};
use crate::quant::engine::{run_backtest, Bars, Costs, RiskRules};
use crate::quant::strategies::StrategySpec;

pub const MAX_AXIS: usize = 25;

pub const CELL_METRICS: [&str; 9] = [
    "total_return_pct",
    "cagr_pct",
    "mdd_pct",
    "sharpe",
    "sortino",
    "calmar",
    "trades",
    "win_rate_pct",
    "profit_factor",
];

pub fn objectives_json() -> Value {
    json!({
        "sharpe": "샤프 지수",
        "cagr_pct": "연평균 수익률",
        "calmar": "칼마 지수",
        "sortino": "소르티노 지수",
        "total_return_pct": "총 수익률",
        "profit_factor": "손익비",
    })
}

pub fn is_valid_objective(obj: &str) -> bool {
    matches!(
        obj,
        "sharpe" | "cagr_pct" | "calmar" | "sortino" | "total_return_pct" | "profit_factor"
    )
}

#[derive(Debug, Clone)]
pub struct Axis {
    pub key: String,
    pub label: String,
    pub values: Vec<Value>,
}

impl Axis {
    pub fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "label": self.label,
            "values": self.values,
        })
    }
}

pub fn parse_axis(spec: &StrategySpec, raw: Option<&Value>) -> ApiResult<Axis> {
    let obj = raw
        .and_then(|v| v.as_object())
        .ok_or_else(|| ApiError::bad_request("invalid_range", "range must be an object"))?;
    let key = obj.get("key").and_then(|v| v.as_str()).unwrap_or("");
    let prm = spec
        .params
        .iter()
        .find(|p| p.key == key)
        .ok_or_else(|| ApiError::bad_request("invalid_range", format!("unknown parameter: {key}")))?;
    let num = |k: &str| -> ApiResult<f64> {
        obj.get(k)
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .ok_or_else(|| {
                ApiError::bad_request(
                    "invalid_range",
                    format!("{}: 시작/끝/간격을 숫자로 입력하세요", prm.label),
                )
            })
    };
    let start = num("start")?;
    let stop = num("stop")?;
    let step = num("step")?;
    if step <= 0.0 || stop < start {
        return Err(ApiError::bad_request(
            "invalid_range",
            format!("{}: 간격은 0보다 크고 끝 ≥ 시작이어야 합니다", prm.label),
        ));
    }
    if start < prm.min || stop > prm.max {
        return Err(ApiError::bad_request(
            "invalid_range",
            format!("{}: {} ~ {} 범위여야 합니다", prm.label, prm.min, prm.max),
        ));
    }
    let mut values: Vec<Value> = Vec::new();
    let mut i = 0usize;
    loop {
        let v = start + (i as f64) * step;
        if v > stop + step * 1e-9 {
            break;
        }
        let val = if prm.integer {
            json!(v.round() as i64)
        } else {
            json!((v * 1e6).round() / 1e6)
        };
        if !values.contains(&val) {
            values.push(val);
        }
        i += 1;
        if values.len() > MAX_AXIS {
            return Err(ApiError::bad_request(
                "range_too_large",
                format!("{}: 값은 최대 {MAX_AXIS}개까지 (간격을 늘리세요)", prm.label),
            ));
        }
    }
    Ok(Axis {
        key: prm.key.to_string(),
        label: prm.label.to_string(),
        values,
    })
}

#[derive(Debug, Clone)]
pub struct ComboItem {
    pub xi: usize,
    pub yi: usize,
    pub params: Option<Value>,
    pub warmup: usize,
    pub error: Option<String>,
}

pub fn build_combos(
    spec: &StrategySpec,
    base: &Value,
    x: &Axis,
    y: Option<&Axis>,
) -> Vec<ComboItem> {
    let mut out = Vec::new();
    let y_vals: Vec<Option<&Value>> = match y {
        Some(ya) => ya.values.iter().map(Some).collect(),
        None => vec![None],
    };
    for (yi, yv) in y_vals.iter().enumerate() {
        for (xi, xv) in x.values.iter().enumerate() {
            let mut params = base.clone();
            if let Some(obj) = params.as_object_mut() {
                obj.insert(x.key.clone(), xv.clone());
                if let (Some(ya), Some(yv)) = (y, yv) {
                    obj.insert(ya.key.clone(), (*yv).clone());
                }
            }
            match (spec.build)(&params) {
                Ok(strat) => out.push(ComboItem {
                    xi,
                    yi,
                    params: Some(strat.params_json()),
                    warmup: strat.warmup(),
                    error: None,
                }),
                Err(e) => out.push(ComboItem {
                    xi,
                    yi,
                    params: None,
                    warmup: 0,
                    error: Some(e),
                }),
            }
        }
    }
    out
}

pub fn max_warmup(items: &[ComboItem]) -> usize {
    items.iter().map(|it| it.warmup).max().unwrap_or(0)
}

pub fn run_grid(
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
) -> Value {
    let rows = y.map(|ya| ya.values.len()).unwrap_or(1);
    let cols = x.values.len();

    let evaluated: Vec<(usize, usize, Value)> = items
        .par_iter()
        .map(|it| {
            let Some(ref p) = it.params else {
                return (
                    it.xi,
                    it.yi,
                    json!({ "invalid": it.error.clone().unwrap_or_default() }),
                );
            };
            let Ok(mut strat) = (spec.build)(p) else {
                return (it.xi, it.yi, json!({ "invalid": "build_error" }));
            };
            let Ok(res) = run_backtest(
                bars,
                strat.as_mut(),
                capital,
                costs,
                start,
                true,
                RiskRules::default(),
                None,
                "off",
            ) else {
                return (it.xi, it.yi, json!({ "invalid": "backtest_error" }));
            };
            let mut m = Map::new();
            for k in CELL_METRICS {
                m.insert(
                    k.to_string(),
                    res["metrics"].get(k).cloned().unwrap_or(Value::Null),
                );
            }
            let score = m.get(objective).and_then(|v| v.as_f64());
            let trades = m.get("trades").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let eligible = score.is_some() && trades >= min_trades;
            (
                it.xi,
                it.yi,
                json!({
                    "params": strat.params_json(),
                    "metrics": Value::Object(m),
                    "score": score,
                    "eligible": eligible,
                }),
            )
        })
        .collect();

    let mut cells: Vec<Vec<Value>> = vec![vec![Value::Null; cols]; rows];
    for (xi, yi, val) in evaluated {
        cells[yi][xi] = val;
    }

    // Neighborhood averages (3x3 or 1x3)
    let mut neighbor_stats: Vec<(usize, usize, usize, Option<f64>)> = Vec::new();
    for yi in 0..rows {
        for xi in 0..cols {
            let c = &cells[yi][xi];
            if c.is_null() || c.get("invalid").is_some() {
                continue;
            }
            let mut near = Vec::new();
            let y0 = yi.saturating_sub(1);
            let y1 = (yi + 2).min(rows);
            let x0 = xi.saturating_sub(1);
            let x1 = (xi + 2).min(cols);
            for ny in y0..y1 {
                for nx in x0..x1 {
                    let nc = &cells[ny][nx];
                    if nc.get("eligible").and_then(|v| v.as_bool()).unwrap_or(false) {
                        if let Some(sc) = nc.get("score").and_then(|v| v.as_f64()) {
                            near.push(sc);
                        }
                    }
                }
            }
            let avg = if near.is_empty() {
                None
            } else {
                Some(near.iter().sum::<f64>() / near.len() as f64)
            };
            neighbor_stats.push((yi, xi, near.len(), avg));
        }
    }
    for (yi, xi, cnt, avg) in neighbor_stats {
        if let Some(obj) = cells[yi][xi].as_object_mut() {
            obj.insert("neighbors".to_string(), json!(cnt));
            obj.insert("neighbor_avg".to_string(), json!(avg));
        }
    }

    let mut eligible: Vec<(usize, usize, &Value)> = Vec::new();
    for yi in 0..rows {
        for xi in 0..cols {
            let c = &cells[yi][xi];
            if c.get("eligible").and_then(|v| v.as_bool()).unwrap_or(false) {
                eligible.push((yi, xi, c));
            }
        }
    }

    let best = eligible.iter().copied().max_by(|a, b| {
        let sa = a.2["score"].as_f64().unwrap_or(f64::NEG_INFINITY);
        let sb = b.2["score"].as_f64().unwrap_or(f64::NEG_INFINITY);
        sa.total_cmp(&sb)
    });
    let need = if y.is_some() { 3 } else { 2 };
    let stable: Vec<(usize, usize, &Value)> = eligible
        .iter()
        .copied()
        .filter(|t| t.2["neighbors"].as_u64().unwrap_or(0) as usize >= need)
        .collect();
    let pool = if stable.is_empty() { &eligible } else { &stable };
    let robust = pool.iter().copied().max_by(|a, b| {
        let sa = a.2["neighbor_avg"].as_f64().unwrap_or(f64::NEG_INFINITY);
        let sb = b.2["neighbor_avg"].as_f64().unwrap_or(f64::NEG_INFINITY);
        sa.total_cmp(&sb)
    });

    let pick = |t: Option<(usize, usize, &Value)>| -> Value {
        match t {
            None => Value::Null,
            Some((yi, xi, c)) => json!({
                "x": xi,
                "y": yi,
                "params": c["params"],
                "metrics": c["metrics"],
                "score": c["score"],
                "neighbor_avg": c["neighbor_avg"],
            }),
        }
    };

    json!({
        "cells": cells,
        "best": pick(best),
        "robust": pick(robust),
    })
}
