pub mod daytrade;
pub mod quant151;
pub mod swing;

use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::quant::engine::{Bars, Context, Order};

#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub integer: bool,
}

impl Param {
    pub const fn int(key: &'static str, label: &'static str, default: f64, min: f64, max: f64) -> Self {
        Self {
            key,
            label,
            default,
            min,
            max,
            step: 1.0,
            integer: true,
        }
    }

    pub const fn float(
        key: &'static str,
        label: &'static str,
        default: f64,
        min: f64,
        max: f64,
        step: f64,
    ) -> Self {
        Self {
            key,
            label,
            default,
            min,
            max,
            step,
            integer: false,
        }
    }
}

pub type ResolvedParams = HashMap<String, f64>;

pub fn resolve_params(defs: &[Param], raw: &Value) -> Result<ResolvedParams, String> {
    let mut out = HashMap::new();
    let obj = raw.as_object();
    for prm in defs {
        let v = match obj.and_then(|m| m.get(prm.key)) {
            None | Some(Value::Null) => prm.default,
            Some(val) => val
                .as_f64()
                .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| format!("{}: 숫자를 입력하세요", prm.label))?,
        };
        if !v.is_finite() || v < prm.min || v > prm.max {
            return Err(format!("{}: {} ~ {} 범위여야 합니다", prm.label, prm.min, prm.max));
        }
        out.insert(
            prm.key.to_string(),
            if prm.integer { v.trunc() } else { v },
        );
    }
    Ok(out)
}

pub fn params_to_json(defs: &[Param], resolved: &ResolvedParams) -> Value {
    let mut m = Map::new();
    for prm in defs {
        let v = resolved.get(prm.key).copied().unwrap_or(prm.default);
        if prm.integer {
            m.insert(prm.key.to_string(), json!(v as i64));
        } else {
            m.insert(prm.key.to_string(), json!(v));
        }
    }
    Value::Object(m)
}

pub struct StrategySpec {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub timeframe: &'static str,
    pub theory: &'static str,
    pub summary: &'static str,
    pub description: &'static str,
    pub guide: fn() -> Value,
    pub params: &'static [Param],
    pub build: fn(&Value) -> Result<Box<dyn Strategy>, String>,
}

impl StrategySpec {
    pub fn meta(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "category": self.category,
            "timeframe": self.timeframe,
            "theory": self.theory,
            "summary": self.summary,
            "description": self.description,
            "guide": (self.guide)(),
            "params": self.params,
        })
    }
}

pub trait Strategy: Send {
    fn id(&self) -> &'static str;
    fn params_json(&self) -> Value;
    fn warmup(&self) -> usize;
    fn prepare(&mut self, bars: &Bars);
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order>;
    fn chart_indicators(&self) -> Vec<Value> {
        vec![]
    }
    fn last_atr(&self) -> Option<f64> {
        None
    }
}

pub fn all_specs() -> Vec<&'static StrategySpec> {
    vec![
        &swing::MA_CROSS_SPEC,
        &swing::TURTLE_SPEC,
        &swing::MACD_TREND_SPEC,
        &swing::BOLLINGER_SPEC,
        &swing::RSI2_SPEC,
        &swing::IBS_SPEC,
        &swing::TSMOM_SPEC,
        &quant151::HP_FILTER_SPEC,
        &quant151::KNN_SPEC,
        &quant151::ALPHA_COMBO_SPEC,
        &daytrade::VWAP_PULLBACK_SPEC,
        &daytrade::HOD_BREAKOUT_SPEC,
        &daytrade::RED_TO_GREEN_SPEC,
        &daytrade::CLOSING_BET_SPEC,
        &daytrade::VOL_BREAKOUT_SPEC,
        &daytrade::GAP_REVERSION_SPEC,
        &daytrade::ORB_SPEC,
        &daytrade::VWAP_SPEC,
    ]
}

pub fn get_spec(id: &str) -> Option<&'static StrategySpec> {
    all_specs().into_iter().find(|s| s.id == id)
}
