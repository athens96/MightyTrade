//! Market Timing ("시장 온도 신호등") regime filter for Quant Lab & AutoTrade bots (MT-5).

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{ApiError, ApiResult};
use crate::market;
use crate::market::models::{Interval, Market};
use crate::quant::engine::Bars;
use crate::quant::indicators as ind;
use crate::quant::screener_data;
use crate::state::AppState;

const CACHE_TTL_SEC: f64 = 300.0;
static REGIME_CACHE: Lazy<DashMap<String, (f64, Value)>> = Lazy::new(DashMap::new);

pub fn benchmarks_for(market: &str) -> &'static [(&'static str, &'static str)] {
    if market == "US" {
        &[("SPY", "SPY (미국 S&P500)"), ("QQQ", "QQQ (미국 나스닥100)")]
    } else {
        &[("069500", "KODEX 200 (코스피)"), ("229200", "KODEX 코스닥150")]
    }
}

pub fn default_benchmark(market: &str) -> &'static str {
    if market == "US" { "SPY" } else { "069500" }
}

pub fn regime_label(regime: &str) -> &'static str {
    match regime {
        "bull" => "🟢 상승장",
        "caution" => "🟡 혼조·주의",
        _ => "🔴 하락·방어",
    }
}

pub fn default_market_timing(market: &str) -> Value {
    json!({
        "mode": "off",
        "benchmark": default_benchmark(market),
        "ma_short": 20,
        "ma_long": 60,
    })
}

pub fn validate_market_timing(raw: Option<&Value>, market: &str) -> ApiResult<Value> {
    let def = default_market_timing(market);
    let Some(obj) = raw.and_then(|v| v.as_object()) else {
        return Ok(def);
    };
    let mode = obj
        .get("mode")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("off");
    if !matches!(mode, "off" | "block_buy" | "half_size" | "exit_all") {
        return Err(ApiError::bad_request(
            "invalid_market_timing",
            "올바르지 않은 마켓 타이밍 모드입니다",
        ));
    }
    let bench_raw = obj
        .get("benchmark")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_benchmark(market).to_string());
    let bench = if benchmarks_for(market).iter().any(|(s, _)| *s == bench_raw) {
        bench_raw
    } else {
        default_benchmark(market).to_string()
    };
    let parse_int = |k: &str, d: i64| -> ApiResult<i64> {
        match obj.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .map(|f| f as i64)
                .ok_or_else(|| {
                    ApiError::bad_request(
                        "invalid_market_timing",
                        "이동평균 기간은 정수여야 합니다",
                    )
                }),
        }
    };
    let ma_s = parse_int("ma_short", 20)?;
    let ma_l = parse_int("ma_long", 60)?;
    if !(5..=120).contains(&ma_s) || !(10..=300).contains(&ma_l) || ma_s >= ma_l {
        return Err(ApiError::bad_request(
            "invalid_market_timing",
            "단기 이평(5~120)은 장기 이평(10~300)보다 작아야 합니다",
        ));
    }
    Ok(json!({
        "mode": mode,
        "benchmark": bench,
        "ma_short": ma_s,
        "ma_long": ma_l,
    }))
}

pub fn compute_regime_series(bars: &Bars, ma_short: usize, ma_long: usize) -> Vec<String> {
    let n = bars.len();
    if n == 0 {
        return vec![];
    }
    let s_line = ind::sma(&bars.close, ma_short);
    let l_line = ind::sma(&bars.close, ma_long);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let c = bars.close[i];
        let reg = match (s_line[i], l_line[i]) {
            (Some(sv), Some(lv)) if c >= sv && c >= lv => "bull",
            (Some(sv), Some(lv)) if c < sv && c < lv => "bear",
            (Some(_), Some(_)) => "caution",
            _ => "bull",
        };
        out.push(reg.to_string());
    }
    out
}

pub fn align_regimes_to_bars(
    target_bars: &Bars,
    bench_bars: &Bars,
    ma_short: usize,
    ma_long: usize,
) -> Vec<String> {
    if bench_bars.is_empty() {
        return vec!["bull".to_string(); target_bars.len()];
    }
    let b_regimes = compute_regime_series(bench_bars, ma_short, ma_long);
    let b_days: Vec<i64> = bench_bars.time.iter().map(|t| t.div_euclid(86400)).collect();
    target_bars
        .time
        .iter()
        .map(|t| {
            let t_day = t.div_euclid(86400);
            let idx = b_days.partition_point(|&d| d < t_day);
            if idx > 0 {
                b_regimes[idx - 1].clone()
            } else {
                "bull".to_string()
            }
        })
        .collect()
}

pub async fn load_benchmark_bars(
    state: &AppState,
    user_id: &str,
    market_str: &str,
    benchmark: &str,
    start_ts: i64,
) -> Bars {
    let m = Market::parse(market_str).unwrap_or(Market::KR);
    let is_mock = state
        .config
        .market_providers
        .get(market_str)
        .map(|s| s.as_str())
        != Some("toss");
    let now = Utc::now().timestamp();
    if !is_mock {
        let period = if start_ts > 0 && (now - start_ts) > 5 * 365 * 86400 {
            "10y"
        } else {
            "5y"
        };
        let pub_map =
            screener_data::fetch_public_history(market_str, &[benchmark.to_string()], period).await;
        if let Some(b) = pub_map.get(benchmark) {
            if b.len() >= 30 {
                return b.clone();
            }
        }
    }
    let anchor = if start_ts > 0 { start_ts } else { now };
    let fetch_from = (anchor - 400 * 86400).max(0);
    let candles = market::get_history(state, user_id, m, benchmark, Interval::D1, fetch_from)
        .await
        .unwrap_or_default();
    Bars::from_candles(&candles)
}

pub async fn current_regime_snapshot(
    state: &AppState,
    user_id: &str,
    market_str: &str,
    benchmark: Option<&str>,
    ma_short: usize,
    ma_long: usize,
    before_date: Option<&str>,
) -> Value {
    let bench = benchmark.unwrap_or_else(|| default_benchmark(market_str));
    let cache_key = format!(
        "{market_str}:{bench}:{ma_short}:{ma_long}:{}",
        before_date.unwrap_or("latest")
    );
    let now_f = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let is_mock = state
        .config
        .market_providers
        .get(market_str)
        .map(|s| s.as_str())
        != Some("toss");
    if !is_mock && before_date.is_none() {
        if let Some(hit) = REGIME_CACHE.get(&cache_key) {
            if now_f - hit.0 < CACHE_TTL_SEC {
                return hit.1.clone();
            }
        }
    }

    let mut bars = load_benchmark_bars(
        state,
        user_id,
        market_str,
        bench,
        Utc::now().timestamp() - 260 * 86400,
    )
    .await;

    if let Some(bd) = before_date {
        let mut last_keep: Option<usize> = None;
        for (i, &t) in bars.time.iter().enumerate() {
            if let Some(dt) = DateTime::from_timestamp(t, 0) {
                if dt.date_naive().to_string().as_str() < bd {
                    last_keep = Some(i);
                }
            }
        }
        if let Some(end_i) = last_keep {
            bars = bars.slice(end_i + 1);
        }
    }

    let bench_name = benchmarks_for(market_str)
        .iter()
        .find(|(s, _)| *s == bench)
        .map(|(_, nm)| *nm)
        .unwrap_or(bench);

    if bars.is_empty() {
        return json!({
            "market": market_str,
            "benchmark": bench,
            "benchmark_name": bench_name,
            "regime": "bull",
            "label": regime_label("bull"),
            "close": Value::Null,
            "ma_short": ma_short,
            "ma_long": ma_long,
            "ma_short_val": Value::Null,
            "ma_long_val": Value::Null,
        });
    }

    let regimes = compute_regime_series(&bars, ma_short, ma_long);
    let s_val = ind::sma(&bars.close, ma_short).last().copied().flatten();
    let l_val = ind::sma(&bars.close, ma_long).last().copied().flatten();
    let reg = regimes.last().map(|s| s.as_str()).unwrap_or("bull");
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let snap = json!({
        "market": market_str,
        "benchmark": bench,
        "benchmark_name": bench_name,
        "regime": reg,
        "label": regime_label(reg),
        "close": r2(*bars.close.last().unwrap()),
        "ma_short": ma_short,
        "ma_long": ma_long,
        "ma_short_val": s_val.map(r2),
        "ma_long_val": l_val.map(r2),
    });

    if !is_mock && before_date.is_none() {
        REGIME_CACHE.insert(cache_key, (now_f, snap.clone()));
    }
    snap
}

pub fn buy_scale_for(mode: &str, regime: &str) -> f64 {
    match mode {
        "off" => 1.0,
        "block_buy" | "exit_all" => {
            if regime == "bear" { 0.0 } else { 1.0 }
        }
        "half_size" => match regime {
            "bear" => 0.5,
            "caution" => 0.75,
            _ => 1.0,
        },
        _ => 1.0,
    }
}
