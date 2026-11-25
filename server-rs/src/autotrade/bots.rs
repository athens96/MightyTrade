//! Bot records and their event log (store collections `bots`, `bot_events`).

use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::autotrade::{dca, notifier, protections, rotation, scalp, semi, webhook};
use crate::errors::{ApiError, ApiResult};
use crate::market::models::Market;
use crate::quant::engine::RiskRules;
use crate::quant::{market_timing, portfolio, screener};
use crate::quant::strategies::get_spec;
use crate::state::AppState;
use crate::storage::find_where;
use crate::users::repository::now_iso;

pub const BOTS: &str = "bots";
pub const EVENTS: &str = "bot_events";
pub const MAX_BOTS: usize = 20;
pub const MAX_EVENTS_PER_BOT: usize = 300;

static SPLIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\s,;]+").unwrap());

pub fn bot_kind(bot: &Value) -> &str {
    bot.get("kind")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("single")
}

pub fn bot_category(bot: &Value) -> &str {
    let k = bot_kind(bot);
    if matches!(k, "rebalance" | "rotation" | "dca" | "webhook" | "scalp") {
        return k;
    }
    bot.get("strategy")
        .and_then(|v| v.as_str())
        .and_then(get_spec)
        .map(|s| s.category)
        .unwrap_or("swing")
}

pub fn bot_timeframe(bot: &Value) -> &str {
    let k = bot_kind(bot);
    if k == "scalp" {
        return "1m";
    }
    if matches!(k, "rebalance" | "rotation" | "dca" | "webhook") {
        return "1d";
    }
    bot.get("strategy")
        .and_then(|v| v.as_str())
        .and_then(get_spec)
        .map(|s| s.timeframe)
        .unwrap_or("1d")
}

pub fn empty_state(capital: f64) -> Value {
    json!({
        "position": { "qty": 0, "entry_price": 0.0, "entry_date": Value::Null },
        "cash": capital,
        "pending": [],
        "trades": [],
        "equity": [],
        "last_run_date": Value::Null,
        "last_price": Value::Null,
        "last_reconcile": Value::Null,
    })
}

pub fn empty_rebalance_state(capital: f64) -> Value {
    json!({
        "positions": {},
        "cash": capital,
        "realized": 0.0,
        "pending": [],
        "rebalances": [],
        "trades": [],
        "equity": [],
        "last_run_date": Value::Null,
        "last_rebalance_date": Value::Null,
        "target_weights": {},
        "last_prices": {},
        "last_reconcile": Value::Null,
    })
}

pub fn has_open_position(bot: &Value, mode: Option<&str>) -> bool {
    let modes: Vec<&str> = match mode {
        Some(m) => vec![m],
        None => vec!["paper", "live"],
    };
    let kind = bot_kind(bot);
    if matches!(kind, "rebalance" | "rotation" | "scalp") {
        return modes.iter().any(|m| {
            bot["state"][*m]
                .get("positions")
                .and_then(|v| v.as_object())
                .is_some_and(|mp| {
                    mp.values()
                        .any(|p| p.get("qty").and_then(|q| q.as_f64()).unwrap_or(0.0) > 0.0)
                })
        });
    }
    modes.iter().any(|m| {
        bot["state"][*m]["position"]["qty"]
            .as_f64()
            .unwrap_or(0.0)
            > 0.0
    })
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
        .filter(|x| x.is_finite())
}

fn take_chars(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

pub fn validate(state: &AppState, body: &Value, existing: Option<&Value>) -> ApiResult<Value> {
    let mut merged = existing
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    if let Some(bo) = body.as_object() {
        for (k, v) in bo {
            merged.insert(k.clone(), v.clone());
        }
    }
    let src = Value::Object(merged);

    let kind = src
        .get("kind")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("single");
    if !matches!(kind, "single" | "rebalance" | "rotation" | "dca" | "webhook" | "scalp") {
        return Err(ApiError::code_only(400, "invalid_kind"));
    }
    let market = Market::parse(src.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let m = market.as_str();
    let capital = as_f64(src.get("capital")).ok_or_else(|| {
        ApiError::bad_request("invalid_capital", "운용 자금을 숫자로 입력하세요")
    })?;
    if capital <= 0.0 {
        return Err(ApiError::bad_request(
            "invalid_capital",
            "운용 자금은 0보다 커야 합니다",
        ));
    }

    if kind == "rebalance" {
        let model_id = src.get("model").and_then(|v| v.as_str()).unwrap_or("");
        let model = portfolio::get_model(model_id)
            .ok_or_else(|| ApiError::bad_request("unknown_model", "알 수 없는 리밸런싱 이론입니다"))?;
        let resolved = model
            .resolve(src.get("params"))
            .map_err(|e| ApiError::bad_request("invalid_params", e))?;
        let presets_val = (model.presets)();
        let raw_assets = src
            .get("assets")
            .filter(|v| !v.is_null() && v.as_array().is_some_and(|a| !a.is_empty()))
            .or_else(|| presets_val.get(m));
        let assets = portfolio::validate_assets(m, raw_assets, model)?;
        let rebalance = src
            .get("rebalance")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(model.default_rebalance);
        if !portfolio::is_valid_rebalance_mode(rebalance) {
            return Err(ApiError::code_only(400, "invalid_rebalance"));
        }
        let drift_pct = match src.get("drift_pct") {
            None | Some(Value::Null) => 5.0,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "허용 편차를 숫자로 입력하세요")
            })?,
        };
        if !(0.5..=50.0).contains(&drift_pct) {
            return Err(ApiError::bad_request(
                "out_of_range",
                "허용 편차는 0.5 ~ 50% 범위여야 합니다",
            ));
        }
        let default_name = format!("{} ({m})", model.name);
        let name = take_chars(
            src.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_name),
            60,
        );
        let guard = protections::validate_guard(src.get("guard"))?;
        let sym_csv = assets
            .iter()
            .map(|a| a.symbol.as_str())
            .collect::<Vec<_>>()
            .join(",");
        return Ok(json!({
            "kind": "rebalance",
            "category": "rebalance",
            "timeframe": "1d",
            "name": name,
            "market": m,
            "symbol": sym_csv,
            "model": model.id,
            "assets": assets,
            "rebalance": rebalance,
            "drift_pct": drift_pct,
            "buy_only": src.get("buy_only").and_then(|v| v.as_bool()).unwrap_or(false),
            "params": model.params_json(&resolved),
            "guard": guard,
            "capital": capital,
        }));
    }

    if kind == "dca" {
        let symbol = src
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_uppercase();
        if !market.validate_symbol(&symbol) {
            return Err(ApiError::code_only(400, "invalid_symbol"));
        }
        let cadence = src
            .get("rebalance")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("weekly");
        if !dca::is_valid_cadence(cadence) {
            return Err(ApiError::bad_request(
                "invalid_rebalance",
                "적립 주기는 daily / weekly / biweekly / monthly 중 하나여야 합니다",
            ));
        }
        let weekday = match src.get("weekday") {
            None | Some(Value::Null) => 0i64,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "적립 금액·배수 설정값은 숫자여야 합니다")
            })? as i64,
        };
        let default_base = if m == "KR" { 100_000.0 } else { 100.0 };
        let base_amount = match src.get("base_amount") {
            None | Some(Value::Null) => default_base,
            x => {
                let v = as_f64(x).ok_or_else(|| {
                    ApiError::bad_request("invalid_number", "적립 금액·배수 설정값은 숫자여야 합니다")
                })?;
                if v == 0.0 { default_base } else { v }
            }
        };
        let dip_mult = match src.get("dip_mult") {
            None | Some(Value::Null) => 1.5,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "적립 금액·배수 설정값은 숫자여야 합니다")
            })?,
        };
        let peak_mult = match src.get("peak_mult") {
            None | Some(Value::Null) => 0.5,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "적립 금액·배수 설정값은 숫자여야 합니다")
            })?,
        };
        if !(0..=4).contains(&weekday) {
            return Err(ApiError::bad_request(
                "out_of_range",
                "적립 요일은 0(월) ~ 4(금) 범위여야 합니다",
            ));
        }
        if base_amount <= 0.0 || base_amount > capital {
            return Err(ApiError::bad_request(
                "out_of_range",
                "1회 적립 금액은 0보다 크고 총 운용 자금 이하여야 합니다",
            ));
        }
        if !((1.0..=3.0).contains(&dip_mult) && (0.0..=1.0).contains(&peak_mult)) {
            return Err(ApiError::bad_request(
                "out_of_range",
                "세일 배수(1~3x) 및 과열 배수(0~1x) 범위를 확인하세요",
            ));
        }
        let guard = protections::validate_guard(src.get("guard"))?;
        let mt_cfg = market_timing::validate_market_timing(src.get("market_timing"), m)?;
        let default_name = format!("스마트 적립 · {symbol}");
        let name = take_chars(
            src.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_name),
            60,
        );
        return Ok(json!({
            "kind": "dca",
            "category": "dca",
            "timeframe": "1d",
            "name": name,
            "market": m,
            "symbol": symbol,
            "rebalance": cadence,
            "weekday": weekday,
            "base_amount": (base_amount * 100.0).round() / 100.0,
            "smart_scale": src.get("smart_scale").and_then(|v| v.as_bool()).unwrap_or(true),
            "dip_mult": (dip_mult * 100.0).round() / 100.0,
            "peak_mult": (peak_mult * 100.0).round() / 100.0,
            "params": {},
            "guard": guard,
            "market_timing": mt_cfg,
            "capital": capital,
        }));
    }

    if kind == "rotation" {
        let preset_id = src
            .get("preset")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("momentum_quality");
        let preset = screener::get_preset(preset_id).ok_or_else(|| {
            ApiError::bad_request("unknown_preset", "알 수 없는 종목 발굴 방식입니다")
        })?;
        if preset.kr_only && m != "KR" {
            return Err(ApiError::bad_request(
                "kr_only_preset",
                "이 발굴 방식은 국내(KR) 시장 전용입니다",
            ));
        }
        let default_uni = if m == "KR" { "kr_all_150" } else { "us_sp100" };
        let universe = src
            .get("universe")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(default_uni);
        if !screener::is_valid_universe(m, universe) {
            return Err(ApiError::bad_request(
                "unknown_universe",
                "알 수 없는 후보 종목 묶음입니다",
            ));
        }
        let custom_syms: Vec<String> = match src.get("custom_symbols") {
            Some(Value::String(s)) => SPLIT_RE
                .split(s)
                .map(|x| x.trim().to_uppercase())
                .filter(|x| !x.is_empty())
                .collect(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|x| x.trim().to_uppercase())
                .filter(|x| !x.is_empty())
                .collect(),
            _ => vec![],
        };
        if universe == "custom" && custom_syms.len() < 2 {
            return Err(ApiError::bad_request(
                "too_few_symbols",
                "직접 입력 종목은 최소 2개 이상이어야 합니다",
            ));
        }
        let top_n = match src.get("top_n") {
            None | Some(Value::Null) => 5i64,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "상위 종목 수·유지 순위는 정수여야 합니다")
            })? as i64,
        };
        let default_hb = (top_n * 2).max(10);
        let hold_buffer = match src.get("hold_buffer") {
            None | Some(Value::Null) => default_hb,
            x => as_f64(x).ok_or_else(|| {
                ApiError::bad_request("invalid_number", "상위 종목 수·유지 순위는 정수여야 합니다")
            })? as i64,
        };
        if !(2..=15).contains(&top_n) {
            return Err(ApiError::bad_request(
                "out_of_range",
                "상위 편입 종목 수는 2 ~ 15개 범위여야 합니다",
            ));
        }
        if !(top_n..=40).contains(&hold_buffer) {
            return Err(ApiError::bad_request(
                "out_of_range",
                format!("유지 순위 버퍼는 {top_n} ~ 40위 범위여야 합니다"),
            ));
        }
        let cadence = src
            .get("rebalance")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("weekly");
        if !rotation::is_valid_cadence(cadence) {
            return Err(ApiError::bad_request(
                "invalid_rebalance",
                "순환 주기는 weekly / biweekly / monthly 중 하나여야 합니다",
            ));
        }
        let strat_id = src
            .get("strategy")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(preset.recommended_strategy);
        let guard = protections::validate_guard(src.get("guard"))?;
        let mt_cfg = market_timing::validate_market_timing(src.get("market_timing"), m)?;
        let default_name = format!("순환매 · {} ({m})", preset.name);
        let name = take_chars(
            src.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_name),
            60,
        );
        return Ok(json!({
            "kind": "rotation",
            "category": "rotation",
            "timeframe": "1d",
            "name": name,
            "market": m,
            "symbol": format!("{universe}:TOP{top_n}"),
            "preset": preset_id,
            "universe": universe,
            "custom_symbols": custom_syms,
            "strategy": strat_id,
            "top_n": top_n,
            "hold_buffer": hold_buffer,
            "rebalance": cadence,
            "exclude_warnings": src.get("exclude_warnings").and_then(|v| v.as_bool()).unwrap_or(true),
            "params": {},
            "guard": guard,
            "market_timing": mt_cfg,
            "capital": capital,
        }));
    }

    if kind == "scalp" {
        let source = src
            .get("source")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(if m == "KR" { "kiwoom_condition" } else { "watchlist" });
        if !matches!(source, "kiwoom_condition" | "watchlist") {
            return Err(ApiError::bad_request(
                "invalid_scalp_source",
                "스캘핑 후보 소스는 영웅문 조건검색식(kiwoom_condition) 또는 직접 지정 목록(watchlist)이어야 합니다",
            ));
        }
        if source == "kiwoom_condition" && m != "KR" {
            return Err(ApiError::bad_request(
                "kr_only_condition",
                "영웅문 조건검색식 연동은 국내(KR) 시장 전용입니다 (미국 주식은 직접 지정 목록을 사용하세요)",
            ));
        }
        let condition_seq = src
            .get("condition_seq")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let condition_name = take_chars(
            src.get("condition_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim(),
            40,
        );
        let custom_syms: Vec<String> = match src.get("custom_symbols") {
            Some(Value::String(s)) => SPLIT_RE
                .split(s)
                .map(|x| x.trim().to_uppercase())
                .filter(|x| !x.is_empty())
                .collect(),
            Some(Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .map(|x| x.trim().to_uppercase())
                .filter(|x| !x.is_empty())
                .collect(),
            _ => vec![],
        };
        if source == "kiwoom_condition" && condition_seq.is_empty() {
            return Err(ApiError::bad_request(
                "condition_required",
                "연동할 영웅문 조건검색식을 선택하세요",
            ));
        }
        if source == "watchlist" {
            if custom_syms.is_empty() || custom_syms.len() > 15 {
                return Err(ApiError::bad_request(
                    "invalid_symbols",
                    "직접 지정 종목은 1 ~ 15개 범위로 입력하세요",
                ));
            }
            for sym in &custom_syms {
                if !market.validate_symbol(sym) {
                    return Err(ApiError::bad_request(
                        "invalid_symbol",
                        format!("{sym}: 선택한 시장({m})의 올바른 종목 코드가 아닙니다"),
                    ));
                }
            }
        }
        let min_strength = as_f64(src.get("min_strength")).unwrap_or(120.0);
        let min_gain_pct = as_f64(src.get("min_gain_pct")).unwrap_or(1.0);
        let max_gain_pct = as_f64(src.get("max_gain_pct")).unwrap_or(22.0);
        let take_profit_pct = as_f64(src.get("take_profit_pct")).unwrap_or(2.0);
        let stop_loss_pct = as_f64(src.get("stop_loss_pct")).unwrap_or(1.0);
        let trailing_stop_pct = as_f64(src.get("trailing_stop_pct")).unwrap_or(0.8);
        let max_positions = as_f64(src.get("max_positions")).unwrap_or(1.0) as i64;
        let max_trades_per_day = as_f64(src.get("max_trades_per_day")).unwrap_or(5.0) as i64;
        let entry_window_mins = as_f64(src.get("entry_window_mins")).unwrap_or(90.0) as i64;

        if !(80.0..=500.0).contains(&min_strength) {
            return Err(ApiError::bad_request("out_of_range", "최소 체결강도는 80 ~ 500% 범위여야 합니다"));
        }
        if !(-10.0..=25.0).contains(&min_gain_pct)
            || !(1.0..=29.5).contains(&max_gain_pct)
            || min_gain_pct >= max_gain_pct
        {
            return Err(ApiError::bad_request("out_of_range", "당일 등락률 범위가 올바르지 않습니다"));
        }
        if !(0.3..=20.0).contains(&take_profit_pct) || !(0.2..=10.0).contains(&stop_loss_pct) {
            return Err(ApiError::bad_request("out_of_range", "익절(0.3~20%) 및 칼손절(0.2~10%) 범위를 확인하세요"));
        }
        if !(0.0..=10.0).contains(&trailing_stop_pct) {
            return Err(ApiError::bad_request("out_of_range", "트레일링 익절은 0 ~ 10% 범위여야 합니다"));
        }
        if !(1..=5).contains(&max_positions) || !(1..=30).contains(&max_trades_per_day) {
            return Err(ApiError::bad_request("out_of_range", "동시 종목 수(1~5) 및 하루 최대 매매(1~30회) 범위를 확인하세요"));
        }
        if !(0..=360).contains(&entry_window_mins) {
            return Err(ApiError::bad_request("out_of_range", "집중 매매 시간은 0(장중 전체) ~ 360분 범위여야 합니다"));
        }

        let guard = protections::validate_guard(src.get("guard"))?;
        let mt_cfg = market_timing::validate_market_timing(src.get("market_timing"), m)?;
        let sym_label = if source == "kiwoom_condition" {
            format!("COND:{condition_seq}")
        } else {
            custom_syms.join(",")
        };
        let default_name = if source == "kiwoom_condition" {
            if !condition_name.is_empty() {
                format!("스캘핑 · {condition_name}")
            } else {
                format!("스캘핑 · 조건식 #{condition_seq}")
            }
        } else {
            format!("스캘핑 · {} ({m})", custom_syms.first().cloned().unwrap_or_default())
        };
        let name = take_chars(
            src.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_name),
            60,
        );
        return Ok(json!({
            "kind": "scalp",
            "category": "scalp",
            "timeframe": "1m",
            "name": name,
            "market": m,
            "symbol": sym_label,
            "source": source,
            "condition_seq": condition_seq,
            "condition_name": condition_name,
            "custom_symbols": custom_syms,
            "min_strength": min_strength,
            "min_gain_pct": min_gain_pct,
            "max_gain_pct": max_gain_pct,
            "above_vwap": src.get("above_vwap").and_then(|v| v.as_bool()).unwrap_or(true),
            "take_profit_pct": take_profit_pct,
            "stop_loss_pct": stop_loss_pct,
            "trailing_stop_pct": trailing_stop_pct,
            "max_positions": max_positions,
            "max_trades_per_day": max_trades_per_day,
            "entry_window_mins": entry_window_mins,
            "params": {},
            "guard": guard,
            "market_timing": mt_cfg,
            "capital": capital,
        }));
    }

    if kind == "webhook" {
        let symbol = src
            .get("symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_uppercase();
        if !market.validate_symbol(&symbol) {
            return Err(ApiError::code_only(400, "invalid_symbol"));
        }
        let risk = RiskRules::from_value(src.get("risk"))
            .map_err(|e| ApiError::bad_request("invalid_risk", e))?
            .to_value();
        let guard = protections::validate_guard(src.get("guard"))?;
        let default_name = format!("웹훅 · {symbol}");
        let name = take_chars(
            src.get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&default_name),
            60,
        );
        let (secret_enc, secret_hint) = if let Some(enc) = existing
            .and_then(|e| e.get("webhook_secret_enc"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            (
                enc.to_string(),
                existing
                    .and_then(|e| e.get("webhook_secret_hint"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        } else {
            let (_, enc, hint) = webhook::generate_secret_fields(state);
            (enc, hint)
        };
        return Ok(json!({
            "kind": "webhook",
            "category": "webhook",
            "timeframe": "1d",
            "name": name,
            "market": m,
            "symbol": symbol,
            "params": {},
            "risk": risk,
            "guard": guard,
            "capital": capital,
            "webhook_secret_enc": secret_enc,
            "webhook_secret_hint": secret_hint,
        }));
    }

    // single strategy bot (swing or daytrade)
    let symbol = src
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if !market.validate_symbol(&symbol) {
        return Err(ApiError::code_only(400, "invalid_symbol"));
    }
    let sid = src.get("strategy").and_then(|v| v.as_str()).unwrap_or("");
    let spec = get_spec(sid).ok_or_else(|| ApiError::code_only(400, "unknown_strategy"))?;
    let empty_obj = json!({});
    let strat = (spec.build)(src.get("params").filter(|v| v.is_object()).unwrap_or(&empty_obj))
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let params = strat.params_json();
    let risk = RiskRules::from_value(src.get("risk"))
        .map_err(|e| ApiError::bad_request("invalid_risk", e))?
        .to_value();
    let guard = protections::validate_guard(src.get("guard"))?;
    let mt_cfg = market_timing::validate_market_timing(src.get("market_timing"), m)?;
    let default_name = format!("{} · {symbol}", spec.name);
    let name = take_chars(
        src.get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&default_name),
        60,
    );
    Ok(json!({
        "kind": "single",
        "category": spec.category,
        "timeframe": spec.timeframe,
        "name": name,
        "market": m,
        "symbol": symbol,
        "strategy": spec.id,
        "params": params,
        "risk": risk,
        "guard": guard,
        "market_timing": mt_cfg,
        "capital": capital,
    }))
}

pub fn create(state: &AppState, uid: &str, body: &Value) -> ApiResult<Value> {
    if list_for(state, uid).len() >= MAX_BOTS {
        return Err(ApiError::bad_request(
            "too_many_bots",
            format!("봇은 최대 {MAX_BOTS}개까지 만들 수 있습니다"),
        ));
    }
    let mut fields = validate(state, body, None)?;
    let bot_id = Uuid::new_v4().simple().to_string()[..12].to_string();
    let cap = fields["capital"].as_f64().unwrap_or(0.0);
    let kind = fields["kind"].as_str().unwrap_or("single").to_string();
    let init_st = if kind == "scalp" {
        (scalp::empty_state(cap), scalp::empty_state(cap))
    } else if kind == "rebalance" || kind == "rotation" {
        (empty_rebalance_state(cap), empty_rebalance_state(cap))
    } else {
        (empty_state(cap), empty_state(cap))
    };
    let now = now_iso();
    if let Some(obj) = fields.as_object_mut() {
        obj.insert("user_id".to_string(), json!(uid));
        obj.insert("mode".to_string(), json!("paper"));
        obj.insert("enabled".to_string(), json!(false));
        obj.insert("created_at".to_string(), json!(now));
        obj.insert("updated_at".to_string(), json!(now));
        obj.insert("state".to_string(), json!({ "paper": init_st.0, "live": init_st.1 }));
    }
    Ok(state.store.put(BOTS, &bot_id, fields))
}

pub fn get(state: &AppState, uid: &str, bot_id: &str) -> ApiResult<Value> {
    let rec = state
        .store
        .get(BOTS, bot_id)
        .filter(|r| r.get("user_id").and_then(|v| v.as_str()) == Some(uid))
        .ok_or_else(|| ApiError::code_only(404, "bot_not_found"))?;
    Ok(rec)
}

pub fn save(state: &AppState, bot: &mut Value) -> Value {
    bot["updated_at"] = json!(now_iso());
    let id = bot["id"].as_str().unwrap_or("").to_string();
    state.store.put(BOTS, &id, bot.clone())
}

pub fn delete(state: &AppState, bot_id: &str) {
    state.store.delete(BOTS, bot_id);
    for e in find_where(state.store.as_ref(), EVENTS, |r| {
        r.get("bot_id").and_then(|v| v.as_str()) == Some(bot_id)
    }) {
        if let Some(eid) = e.get("id").and_then(|v| v.as_str()) {
            state.store.delete(EVENTS, eid);
        }
    }
}

pub fn list_for(state: &AppState, uid: &str) -> Vec<Value> {
    let mut rows = find_where(state.store.as_ref(), BOTS, |b| {
        b.get("user_id").and_then(|v| v.as_str()) == Some(uid)
    });
    rows.sort_by(|a, b| {
        let ca = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let cb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        ca.cmp(cb)
    });
    rows
}

pub fn all_enabled(state: &AppState) -> Vec<Value> {
    find_where(state.store.as_ref(), BOTS, |b| {
        b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
    })
}

pub fn state_key(mode: &str) -> &'static str {
    if mode == "paper" {
        "paper"
    } else {
        "live"
    }
}

pub fn state(bot: &Value) -> &Value {
    let m = bot.get("mode").and_then(|v| v.as_str()).unwrap_or("paper");
    &bot["state"][state_key(m)]
}

pub fn record_live_slippage(
    st: &mut Value,
    symbol: &str,
    side: &str,
    qty: f64,
    signal_price: Option<f64>,
    fill_price: f64,
    date: &str,
) -> Option<f64> {
    let sig_px = signal_price?;
    if sig_px <= 0.0 || fill_price <= 0.0 || qty <= 0.0 {
        return None;
    }
    let s_up = side.to_uppercase();
    let bps = if s_up == "BUY" {
        ((fill_price / sig_px) - 1.0) * 10_000.0
    } else {
        (1.0 - (fill_price / sig_px)) * 10_000.0
    };
    let rounded_bps = (bps * 100.0).round() / 100.0;
    let obj = st.as_object_mut()?;
    let fills = obj
        .entry("fills".to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()?;
    fills.push(json!({
        "date": date,
        "symbol": symbol,
        "side": s_up,
        "qty": (qty * 10000.0).round() / 10000.0,
        "signal_price": (sig_px * 10000.0).round() / 10000.0,
        "fill_price": (fill_price * 10000.0).round() / 10000.0,
        "slippage_bps": rounded_bps,
    }));
    if fills.len() > 200 {
        let drop_n = fills.len() - 200;
        fills.drain(0..drop_n);
    }
    Some(rounded_bps)
}

pub async fn log(
    state: &AppState,
    bot: &Value,
    kind: &str,
    message: &str,
    level: &str,
    data: Option<Value>,
) {
    let bid = bot["id"].as_str().unwrap_or("");
    let uid = bot["user_id"].as_str().unwrap_or("");
    let mode = bot["mode"].as_str().unwrap_or("paper");
    let eid = Uuid::new_v4().simple().to_string();
    state.store.put(
        EVENTS,
        &eid,
        json!({
            "bot_id": bid,
            "user_id": uid,
            "mode": mode,
            "at": Utc::now().to_rfc3339(),
            "kind": kind,
            "level": level,
            "message": message,
            "data": data.unwrap_or_else(|| json!({})),
        }),
    );
    let mut evs = find_where(state.store.as_ref(), EVENTS, |e| {
        e.get("bot_id").and_then(|v| v.as_str()) == Some(bid)
    });
    if evs.len() > MAX_EVENTS_PER_BOT {
        evs.sort_by(|a, b| {
            let aa = a.get("at").and_then(|v| v.as_str()).unwrap_or("");
            let ba = b.get("at").and_then(|v| v.as_str()).unwrap_or("");
            aa.cmp(ba)
        });
        for e in evs.iter().take(evs.len() - MAX_EVENTS_PER_BOT) {
            if let Some(id) = e.get("id").and_then(|v| v.as_str()) {
                state.store.delete(EVENTS, id);
            }
        }
    }
    if matches!(
        kind,
        "order" | "approval" | "fill" | "error" | "skip" | "halt" | "protection"
    ) {
        notifier::notify_bot_log(state, bot, kind, message, level).await;
    }
}

pub fn events(state: &AppState, bot: &Value, limit: usize) -> Vec<Value> {
    let bid = bot["id"].as_str().unwrap_or("");
    let mut rows = find_where(state.store.as_ref(), EVENTS, |e| {
        e.get("bot_id").and_then(|v| v.as_str()) == Some(bid)
    });
    rows.sort_by(|a, b| {
        let aa = a.get("at").and_then(|v| v.as_str()).unwrap_or("");
        let ba = b.get("at").and_then(|v| v.as_str()).unwrap_or("");
        ba.cmp(aa)
    });
    rows.truncate(limit);
    rows
}

fn tail_slice(arr: Option<&Value>, n: usize) -> Value {
    let Some(a) = arr.and_then(|v| v.as_array()) else {
        return json!([]);
    };
    let start = a.len().saturating_sub(n);
    Value::Array(a[start..].to_vec())
}

pub fn public(bot: &Value) -> Value {
    let mut out = Map::new();
    if let Some(obj) = bot.as_object() {
        for (k, v) in obj {
            if k != "state" && k != "webhook_secret_enc" {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    let kind = bot_kind(bot);
    out.insert("kind".to_string(), json!(kind));
    out.insert("category".to_string(), json!(bot_category(bot)));
    out.insert("timeframe".to_string(), json!(bot_timeframe(bot)));

    let st = state(bot);
    let pending_app = semi::active_proposal(st, None);
    let is_paper = bot.get("mode").and_then(|v| v.as_str()) == Some("paper");

    if matches!(kind, "rebalance" | "rotation" | "scalp") {
        let positions = st.get("positions").cloned().unwrap_or_else(|| json!({}));
        let last_prices = st.get("last_prices").cloned().unwrap_or_else(|| json!({}));
        let mut total_qty = 0i64;
        let mut pos_val = 0.0;
        if let Some(pm) = positions.as_object() {
            for (sym, p) in pm {
                let q = p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
                let px = last_prices
                    .get(sym)
                    .and_then(|v| v.as_f64())
                    .or_else(|| p.get("entry_price").and_then(|v| v.as_f64()))
                    .unwrap_or(0.0);
                total_qty += q;
                pos_val += (q as f64) * px;
            }
        }
        let mut st_out = json!({
            "position": { "qty": total_qty, "entry_price": 0.0, "entry_date": Value::Null },
            "positions": positions,
            "pending": st.get("pending").cloned().unwrap_or_else(|| json!([])),
            "pending_approval": pending_app,
            "last_run_date": st.get("last_run_date").cloned().unwrap_or(Value::Null),
            "last_rebalance_date": st.get("last_rebalance_date").cloned().unwrap_or(Value::Null),
            "target_weights": st.get("target_weights").cloned().unwrap_or_else(|| json!({})),
            "selected_picks": st.get("selected_picks").cloned().unwrap_or_else(|| json!([])),
            "last_candidates": st.get("last_candidates").cloned().unwrap_or_else(|| json!([])),
            "today_trades_count": st.get("today_trades_count").cloned().unwrap_or_else(|| json!(0)),
            "last_prices": last_prices,
            "last_price": pos_val,
            "rebalances": tail_slice(st.get("rebalances"), 30),
            "trades": if kind == "scalp" { tail_slice(st.get("trades"), 50) } else { json!([]) },
            "equity": tail_slice(st.get("equity"), 400),
            "tripped": st.get("tripped").cloned().unwrap_or(Value::Null),
            "cooldown_until": st.get("cooldown_until").cloned().unwrap_or(Value::Null),
        });
        if is_paper {
            let cash = st.get("cash").and_then(|v| v.as_f64()).unwrap_or(0.0);
            st_out["cash"] = json!(cash);
            st_out["equity_now"] = json!(cash + pos_val);
        }
        out.insert("state".to_string(), st_out);
        return Value::Object(out);
    }

    let pos = st.get("position").cloned().unwrap_or_else(|| json!({ "qty": 0, "entry_price": 0.0, "entry_date": Value::Null }));
    let price = st
        .get("last_price")
        .and_then(|v| v.as_f64())
        .filter(|&x| x > 0.0)
        .or_else(|| pos.get("entry_price").and_then(|v| v.as_f64()))
        .unwrap_or(0.0);
    let mut st_out = json!({
        "position": pos,
        "pending": st.get("pending").cloned().unwrap_or_else(|| json!([])),
        "pending_approval": pending_app,
        "last_run_date": st.get("last_run_date").cloned().unwrap_or(Value::Null),
        "last_price": st.get("last_price").cloned().unwrap_or(Value::Null),
        "trades": tail_slice(st.get("trades"), 50),
        "equity": tail_slice(st.get("equity"), 400),
        "tripped": st.get("tripped").cloned().unwrap_or(Value::Null),
        "cooldown_until": st.get("cooldown_until").cloned().unwrap_or(Value::Null),
    });
    if kind == "dca" {
        st_out["dca_stats"] = dca::public_dca_stats(st);
    }
    if is_paper {
        let cash = st.get("cash").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let qty = pos.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0);
        st_out["cash"] = json!(cash);
        st_out["equity_now"] = json!(cash + qty * price);
    }
    out.insert("state".to_string(), st_out);
    Value::Object(out)
}
