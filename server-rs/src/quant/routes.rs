use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;

use crate::auth::extractors::AuthUser;
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, models::{Candle, Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::quant::engine::{buy_and_hold, run_backtest, Bars, Costs, RiskRules};
use crate::quant::market_timing;
use crate::quant::metrics::performance;
use crate::quant::montecarlo;
use crate::quant::optimize::{self, Axis, ComboItem};
use crate::quant::portfolio;
use crate::quant::screener;
use crate::quant::screener_data;
use crate::quant::strategies::{all_specs, get_spec, resolve_params, params_to_json, StrategySpec};
use crate::quant::walkforward;
use crate::state::AppState;

static QUANT_SYMBOL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[0-9A-Z][0-9A-Z.\-]{0,19}$").unwrap());

const MAX_GRID: usize = 400;
const MAX_WF_RUNS: usize = 3000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/strategies", get(strategies))
        .route("/defaults", get(defaults))
        .route("/backtest", post(backtest))
        .route("/optimize", post(optimize_route))
        .route("/walkforward", post(walkforward_route))
        .route("/montecarlo", post(montecarlo_route))
        .route("/portfolio/models", get(portfolio_models))
        .route("/portfolio/backtest", post(portfolio_backtest))
        .route("/screener/presets", get(screener_presets))
        .route("/screener/run", post(screener_run))
        .route("/screener/rotation-backtest", post(screener_rotation_backtest))
}

#[derive(Deserialize)]
struct MarketQuery {
    market: Option<String>,
}

fn period_days(period: &str) -> Option<Option<i64>> {
    match period {
        "1y" => Some(Some(365)),
        "3y" => Some(Some(365 * 3)),
        "5y" => Some(Some(365 * 5)),
        "10y" => Some(Some(3652)),
        "max" => Some(None),
        _ => None,
    }
}

fn warning_msg(code: &str) -> String {
    match code {
        "insufficient_capital" => {
            "초기 자본이 1주 가격보다 작거나 위험 비중이 작아 매수하지 못한 신호가 있습니다.".to_string()
        }
        "start_adjusted" => {
            "상장 기간이 짧거나 지표 계산에 필요한 데이터가 부족해 시작일이 뒤로 조정되었습니다.".to_string()
        }
        other => other.to_string(),
    }
}

fn parse_num(body: &Value, key: &str, default: f64, lo: f64, hi: f64) -> ApiResult<f64> {
    let v = match body.get(key) {
        None | Some(Value::Null) => default,
        Some(x) => x
            .as_f64()
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| ApiError::bad_request("invalid_number", format!("{key} must be a number")))?,
    };
    if !v.is_finite() || v < lo || v > hi {
        return Err(ApiError::bad_request(
            "out_of_range",
            format!("{key} must be between {lo} and {hi}"),
        ));
    }
    Ok(v)
}

struct Setup {
    market: Market,
    symbol: String,
    spec: &'static StrategySpec,
    params: Value,
    period: String,
    capital: f64,
    costs: Costs,
    risk: RiskRules,
    market_timing: Value,
}

async fn parse_setup(state: &AppState, uid: &str, body: &Value) -> ApiResult<Setup> {
    let market = Market::parse(body.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let symbol = body
        .get("symbol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_uppercase();
    if !QUANT_SYMBOL_RE.is_match(&symbol) {
        return Err(ApiError::code_only(400, "invalid_symbol"));
    }
    let sid = body.get("strategy").and_then(|v| v.as_str()).unwrap_or("");
    let spec = get_spec(sid).ok_or_else(|| ApiError::code_only(400, "unknown_strategy"))?;
    let period = body
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("5y")
        .to_string();
    if period_days(&period).is_none() {
        return Err(ApiError::code_only(400, "invalid_period"));
    }
    let risk = RiskRules::from_value(body.get("risk"))
        .map_err(|e| ApiError::bad_request("invalid_risk", e))?;
    let mt_cfg = market_timing::validate_market_timing(body.get("market_timing"), market.as_str())?;

    let def = if body.get("commission_pct").is_none() {
        cost_defaults(state, uid, market).await
    } else {
        json!({})
    };
    let capital = parse_num(
        body,
        "capital",
        def.get("capital").and_then(|v| v.as_f64()).unwrap_or(0.0),
        1.0,
        1e13,
    )?;
    let costs = Costs {
        commission_pct: parse_num(
            body,
            "commission_pct",
            def.get("commission_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
        tax_pct: parse_num(
            body,
            "tax_pct",
            def.get("tax_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
        slippage_pct: parse_num(
            body,
            "slippage_pct",
            def.get("slippage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
    };

    Ok(Setup {
        market,
        symbol,
        spec,
        params: body.get("params").cloned().unwrap_or_else(|| json!({})),
        period,
        capital,
        costs,
        risk,
        market_timing: mt_cfg,
    })
}

async fn load_bars(
    state: &AppState,
    uid: &str,
    s: &Setup,
    warmup: usize,
) -> ApiResult<(Vec<Candle>, Bars, usize, Vec<String>)> {
    let tf = s.spec.timeframe;
    let is_mock = market::provider_name(state, s.market) != "toss";
    if tf != "1d" {
        let interval = Interval::parse(tf)?;
        let mut bars_opt: Option<Bars> = None;
        let mut candles: Vec<Candle> = Vec::new();
        if !is_mock {
            if let Some(b) =
                screener_data::fetch_public_intraday(s.market.as_str(), &s.symbol, tf).await
            {
                candles = b.to_candles();
                bars_opt = Some(b);
            }
        }
        let bars = match bars_opt {
            Some(b) => b,
            None => {
                let (_, cs) =
                    market::get_candles(state, uid, s.market, &s.symbol, interval, 400).await?;
                let b = Bars::from_candles(&cs);
                candles = cs;
                b
            }
        };
        if bars.len() < 2 || warmup >= bars.len() - 1 {
            return Err(ApiError::bad_request(
                "not_enough_data",
                "분봉 백테스트에 필요한 데이터가 부족합니다",
            ));
        }
        return Ok((candles, bars, warmup, vec![]));
    }

    let days_opt = period_days(&s.period).unwrap();
    let now = Utc::now().timestamp();
    let start_ts = days_opt.map(|d| now - d * 86400).unwrap_or(0);
    let fetch_from = days_opt
        .map(|_| start_ts - ((warmup as f64 * 1.6 + 15.0) * 86400.0) as i64)
        .unwrap_or(0);

    let mut bars_opt: Option<Bars> = None;
    let mut candles: Vec<Candle> = Vec::new();
    if !is_mock {
        let pub_map = screener_data::fetch_public_history(
            s.market.as_str(),
            &[s.symbol.clone()],
            &s.period,
        )
        .await;
        if let Some(b) = pub_map.get(&s.symbol) {
            candles = b.to_candles();
            bars_opt = Some(b.clone());
        }
    }
    let bars = match bars_opt {
        Some(b) => b,
        None => {
            let cs =
                market::get_history(state, uid, s.market, &s.symbol, Interval::D1, fetch_from)
                    .await?;
            let b = Bars::from_candles(&cs);
            candles = cs;
            b
        }
    };
    let first = bars
        .time
        .iter()
        .position(|&t| t >= start_ts)
        .unwrap_or(bars.len());
    let start = first.max(warmup);
    if bars.len() < 2 || start >= bars.len() - 1 {
        return Err(ApiError::bad_request(
            "not_enough_data",
            "백테스트에 필요한 데이터가 부족합니다",
        ));
    }
    let warn = if start > first && days_opt.is_some() {
        vec!["start_adjusted".to_string()]
    } else {
        vec![]
    };
    Ok((candles, bars, start, warn))
}

fn context_fields(s: &Setup, bars: &Bars, start: usize) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("market".to_string(), json!(s.market.as_str()));
    m.insert("symbol".to_string(), json!(s.symbol));
    m.insert("strategy".to_string(), json!(s.spec.id));
    m.insert("interval".to_string(), json!(s.spec.timeframe));
    m.insert("costs".to_string(), json!(s.costs));
    m.insert("capital".to_string(), json!(s.capital));
    m.insert(
        "currency".to_string(),
        json!(if s.market == Market::KR { "KRW" } else { "USD" }),
    );
    m.insert(
        "period".to_string(),
        json!({ "start": bars.time[start], "end": bars.time[bars.len() - 1] }),
    );
    m
}

fn grid_inputs(
    s: &Setup,
    body: &Value,
) -> ApiResult<(Axis, Option<Axis>, String, usize, Value, Vec<ComboItem>)> {
    let x = optimize::parse_axis(s.spec, body.get("x"))?;
    let y = if body.get("y").is_some_and(|v| !v.is_null()) {
        Some(optimize::parse_axis(s.spec, body.get("y"))?)
    } else {
        None
    };
    if y.as_ref().is_some_and(|ya| ya.key == x.key) {
        return Err(ApiError::bad_request(
            "invalid_range",
            "X와 Y는 서로 다른 파라미터여야 합니다",
        ));
    }
    let objective = body
        .get("objective")
        .and_then(|v| v.as_str())
        .unwrap_or("sharpe")
        .to_string();
    if !optimize::is_valid_objective(&objective) {
        return Err(ApiError::code_only(400, "invalid_objective"));
    }
    let min_trades = parse_num(body, "min_trades", 5.0, 0.0, 1000.0)? as usize;
    let base_resolved = resolve_params(s.spec.params, &s.params)
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let base = params_to_json(s.spec.params, &base_resolved);
    let items = optimize::build_combos(s.spec, &base, &x, y.as_ref());
    if !items.iter().any(|it| it.params.is_some()) {
        let err = items
            .iter()
            .find_map(|it| it.error.clone())
            .unwrap_or_else(|| "invalid params".to_string());
        return Err(ApiError::bad_request("invalid_params", err));
    }
    Ok((x, y, objective, min_trades, base, items))
}

async fn strategies(AuthUser(_): AuthUser) -> Json<Value> {
    let items: Vec<Value> = all_specs().into_iter().map(|s| s.meta()).collect();
    Json(json!({
        "items": items,
        "objectives": optimize::objectives_json(),
    }))
}

async fn defaults(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(cost_defaults(&state, uid, market).await))
}

async fn backtest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let s = parse_setup(&state, uid, &body).await?;
    let mut strat = (s.spec.build)(&s.params)
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let (candles, bars, start, warn) = load_bars(&state, uid, &s, strat.warmup()).await?;

    let mt_mode = s.market_timing["mode"].as_str().unwrap_or("off");
    let regimes = if mt_mode != "off" {
        let bench = s.market_timing["benchmark"].as_str().unwrap_or("069500");
        let bench_bars =
            market_timing::load_benchmark_bars(&state, uid, s.market.as_str(), bench, bars.time[0])
                .await;
        Some(market_timing::align_regimes_to_bars(
            &bars,
            &bench_bars,
            s.market_timing["ma_short"].as_u64().unwrap_or(20) as usize,
            s.market_timing["ma_long"].as_u64().unwrap_or(60) as usize,
        ))
    } else {
        None
    };

    let mut result = run_backtest(
        &bars,
        strat.as_mut(),
        s.capital,
        s.costs,
        start,
        true,
        s.risk,
        regimes.as_deref(),
        mt_mode,
    )
    .map_err(|e| ApiError::bad_request("backtest_failed", e))?;

    let mut codes: Vec<String> = result
        .as_object_mut()
        .and_then(|o| o.remove("warnings"))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    codes.extend(warn);
    let warnings: Vec<String> = codes.iter().map(|c| warning_msg(c)).collect();

    let mut out = context_fields(&s, &bars, start);
    out.insert("params".to_string(), strat.params_json());
    out.insert("risk".to_string(), s.risk.to_value());
    out.insert("market_timing".to_string(), s.market_timing.clone());
    out.insert("start_index".to_string(), json!(start));
    out.insert("candles".to_string(), json!(candles));
    out.insert("chart_indicators".to_string(), json!(strat.chart_indicators()));
    out.insert("warnings".to_string(), json!(warnings));
    if let Some(res_obj) = result.as_object() {
        for (k, v) in res_obj {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(Value::Object(out)))
}

async fn optimize_route(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let s = parse_setup(&state, uid, &body).await?;
    let (x, y, objective, min_trades, _base, items) = grid_inputs(&s, &body)?;
    if items.len() > MAX_GRID {
        return Err(ApiError::bad_request(
            "range_too_large",
            format!("조합이 {}개입니다. 최대 {MAX_GRID}개까지 가능합니다", items.len()),
        ));
    }
    let t0 = Instant::now();
    let (_, bars, start, warn) = load_bars(&state, uid, &s, optimize::max_warmup(&items)).await?;
    let bars_c = bars.clone();
    let x_c = x.clone();
    let y_c = y.clone();
    let obj_c = objective.clone();
    let spec = s.spec;
    let capital = s.capital;
    let costs = s.costs;

    let grid = tokio::task::spawn_blocking(move || {
        optimize::run_grid(
            &bars_c,
            spec,
            &items,
            &x_c,
            y_c.as_ref(),
            capital,
            costs,
            start,
            &obj_c,
            min_trades,
        )
    })
    .await
    .map_err(|e| ApiError::internal("worker_error", e.to_string()))?;

    let bh = buy_and_hold(&bars, start, bars.len(), s.capital, s.costs, true);
    let bench = performance(&bars.time[start..], &bh);

    let mut out = context_fields(&s, &bars, start);
    out.insert("x".to_string(), x.to_json());
    out.insert("y".to_string(), y.map(|ya| ya.to_json()).unwrap_or(Value::Null));
    out.insert("objective".to_string(), json!(objective));
    out.insert("min_trades".to_string(), json!(min_trades));
    out.insert("benchmark".to_string(), bench);
    out.insert(
        "elapsed_sec".to_string(),
        json!((t0.elapsed().as_secs_f64() * 100.0).round() / 100.0),
    );
    out.insert(
        "warnings".to_string(),
        json!(warn.iter().map(|c| warning_msg(c)).collect::<Vec<_>>()),
    );
    if let Some(go) = grid.as_object() {
        for (k, v) in go {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(Value::Object(out)))
}

async fn walkforward_route(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let s = parse_setup(&state, uid, &body).await?;
    if s.spec.timeframe != "1d" {
        return Err(ApiError::bad_request(
            "daily_strategy_only",
            "워크포워드는 수년 단위 검증이므로 일봉(스윙·일봉 단타) 전략에서만 사용할 수 있습니다. 분봉 단타 전략은 [백테스트 · 최적화 · 몬테카를로]를 이용해 주세요.",
        ));
    }
    let (x, y, objective, min_trades, _base, items) = grid_inputs(&s, &body)?;
    let is_years = parse_num(&body, "is_years", 3.0, 0.5, 20.0)?;
    let oos_months = parse_num(&body, "oos_months", 12.0, 1.0, 60.0)?;
    let mode = body
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("rolling")
        .to_string();
    if mode != "rolling" && mode != "anchored" {
        return Err(ApiError::code_only(400, "invalid_mode"));
    }

    let t0 = Instant::now();
    let (_, bars, start, warn) = load_bars(&state, uid, &s, optimize::max_warmup(&items)).await?;
    let span_years = (bars.time[bars.len() - 1] - bars.time[start]) as f64 / 86400.0 / 365.25;
    let est_windows = (((span_years - is_years) * 12.0 / oos_months) as i64 + 1).max(1) as usize;
    if est_windows * items.len() > MAX_WF_RUNS {
        return Err(ApiError::bad_request(
            "range_too_large",
            format!(
                "약 {est_windows}개 구간 × {}개 조합 = 계산량이 너무 큽니다. 조합을 줄이거나 검증 기간을 늘리세요",
                items.len()
            ),
        ));
    }

    let bars_c = bars.clone();
    let x_c = x.clone();
    let y_c = y.clone();
    let obj_c = objective.clone();
    let mode_c = mode.clone();
    let spec = s.spec;
    let capital = s.capital;
    let costs = s.costs;

    let wf = tokio::task::spawn_blocking(move || {
        walkforward::walk_forward(
            &bars_c,
            spec,
            &items,
            &x_c,
            y_c.as_ref(),
            capital,
            costs,
            start,
            &obj_c,
            min_trades,
            is_years,
            oos_months,
            &mode_c,
        )
    })
    .await
    .map_err(|e| ApiError::internal("worker_error", e.to_string()))??;

    let mut out = context_fields(&s, &bars, start);
    out.insert("x".to_string(), x.to_json());
    out.insert("y".to_string(), y.map(|ya| ya.to_json()).unwrap_or(Value::Null));
    out.insert("objective".to_string(), json!(objective));
    out.insert("min_trades".to_string(), json!(min_trades));
    out.insert("is_years".to_string(), json!(is_years));
    out.insert("oos_months".to_string(), json!(oos_months));
    out.insert("mode".to_string(), json!(mode));
    out.insert(
        "elapsed_sec".to_string(),
        json!((t0.elapsed().as_secs_f64() * 100.0).round() / 100.0),
    );
    out.insert(
        "warnings".to_string(),
        json!(warn.iter().map(|c| warning_msg(c)).collect::<Vec<_>>()),
    );
    if let Some(wo) = wf.as_object() {
        for (k, v) in wo {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(Value::Object(out)))
}

async fn montecarlo_route(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let s = parse_setup(&state, uid, &body).await?;
    let runs = parse_num(&body, "runs", 1000.0, 100.0, 5000.0)? as usize;
    let method = body
        .get("method")
        .and_then(|v| v.as_str())
        .unwrap_or("bootstrap")
        .to_string();
    if method != "bootstrap" && method != "shuffle" {
        return Err(ApiError::code_only(400, "invalid_method"));
    }
    let mut strat = (s.spec.build)(&s.params)
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let params_json = strat.params_json();
    let (_, bars, start, warn) = load_bars(&state, uid, &s, strat.warmup()).await?;
    let bars_c = bars.clone();
    let capital = s.capital;
    let costs = s.costs;

    let result = tokio::task::spawn_blocking(move || -> ApiResult<Value> {
        let bt = run_backtest(
            &bars_c,
            strat.as_mut(),
            capital,
            costs,
            start,
            true,
            RiskRules::default(),
            None,
            "off",
        )
        .map_err(|e| ApiError::bad_request("backtest_failed", e))?;
        let eq = bt["equity"].as_array().cloned().unwrap_or_default();
        let tr = bt["trades"].as_array().cloned().unwrap_or_default();
        let impacts = montecarlo::trade_impacts(&tr, &eq);
        let years = (bars_c.time[bars_c.len() - 1] - bars_c.time[start]) as f64 / 86400.0 / 365.25;
        montecarlo::simulate(&impacts, runs, &method, 42, if years > 0.05 { Some(years) } else { None })
    })
    .await
    .map_err(|e| ApiError::internal("worker_error", e.to_string()))??;

    let mut out = context_fields(&s, &bars, start);
    out.insert("params".to_string(), params_json);
    out.insert(
        "warnings".to_string(),
        json!(warn.iter().map(|c| warning_msg(c)).collect::<Vec<_>>()),
    );
    if let Some(ro) = result.as_object() {
        for (k, v) in ro {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(Value::Object(out)))
}

async fn portfolio_models(AuthUser(_): AuthUser) -> Json<Value> {
    let items: Vec<Value> = portfolio::PORTFOLIO_MODELS.iter().map(|m| m.meta()).collect();
    Json(json!({
        "items": items,
        "rebalance_modes": portfolio::rebalance_modes_json(),
    }))
}

async fn portfolio_backtest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let market = Market::parse(body.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let model_id = body.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let model = portfolio::get_model(model_id)
        .ok_or_else(|| ApiError::bad_request("unknown_model", "알 수 없는 포트폴리오 이론입니다"))?;
    let resolved = model
        .resolve(body.get("params"))
        .map_err(|e| ApiError::bad_request("invalid_params", e))?;
    let presets_val = (model.presets)();
    let default_assets = presets_val.get(market.as_str());
    let raw_assets = body
        .get("assets")
        .filter(|v| !v.is_null() && v.as_array().is_some_and(|a| !a.is_empty()))
        .or(default_assets);
    let assets = portfolio::validate_assets(market.as_str(), raw_assets, model)?;

    let rebalance = body
        .get("rebalance")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(model.default_rebalance)
        .to_string();
    if !portfolio::is_valid_rebalance_mode(&rebalance) {
        return Err(ApiError::code_only(400, "invalid_rebalance"));
    }
    let drift_pct = parse_num(&body, "drift_pct", 5.0, 0.5, 50.0)?;
    let period = body
        .get("period")
        .and_then(|v| v.as_str())
        .unwrap_or("5y")
        .to_string();
    let days_opt = period_days(&period).ok_or_else(|| ApiError::code_only(400, "invalid_period"))?;

    let def = if body.get("commission_pct").is_none() {
        cost_defaults(&state, uid, market).await
    } else {
        json!({})
    };
    let capital = parse_num(
        &body,
        "capital",
        def.get("capital").and_then(|v| v.as_f64()).unwrap_or(0.0),
        1.0,
        1e13,
    )?;
    let costs = Costs {
        commission_pct: parse_num(
            &body,
            "commission_pct",
            def.get("commission_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
        tax_pct: parse_num(
            &body,
            "tax_pct",
            def.get("tax_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
        slippage_pct: parse_num(
            &body,
            "slippage_pct",
            def.get("slippage_pct").and_then(|v| v.as_f64()).unwrap_or(0.0),
            0.0,
            5.0,
        )?,
    };

    let now = Utc::now().timestamp();
    let start_ts = days_opt.map(|d| now - d * 86400).unwrap_or(0);
    let warm = (model.warmup_fn)(&resolved);
    let fetch_from = days_opt
        .map(|_| start_ts - ((warm as f64 * 1.6 + 20.0) * 86400.0) as i64)
        .unwrap_or(0);
    let is_mock = market::provider_name(&state, market) != "toss";
    let syms: Vec<String> = assets.iter().map(|a| a.symbol.clone()).collect();
    let mut bars_by_sym: HashMap<String, Bars> = if !is_mock {
        screener_data::fetch_public_history(market.as_str(), &syms, &period).await
    } else {
        HashMap::new()
    };
    for sym in &syms {
        if !bars_by_sym.contains_key(sym) {
            let cs = market::get_history(&state, uid, market, sym, Interval::D1, fetch_from).await?;
            bars_by_sym.insert(sym.clone(), Bars::from_candles(&cs));
        }
    }

    let buy_only = body
        .get("buy_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mid = model.id;
    let assets_c = assets.clone();
    let resolved_c = resolved.clone();
    let rebal_c = rebalance.clone();
    let mut res = tokio::task::spawn_blocking(move || {
        portfolio::run_portfolio_backtest(
            &bars_by_sym,
            mid,
            &assets_c,
            &resolved_c,
            &rebal_c,
            drift_pct,
            capital,
            costs,
            start_ts,
            buy_only,
        )
    })
    .await
    .map_err(|e| ApiError::internal("worker_error", e.to_string()))??;

    let warn_codes: Vec<String> = res
        .as_object_mut()
        .and_then(|o| o.remove("warnings"))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|v| v.as_str().map(|s| warning_msg(s)))
        .collect();

    let mut out = serde_json::Map::new();
    out.insert("market".to_string(), json!(market.as_str()));
    out.insert("model".to_string(), json!(model.id));
    out.insert("assets".to_string(), json!(assets));
    out.insert("params".to_string(), model.params_json(&resolved));
    out.insert("rebalance".to_string(), json!(rebalance));
    out.insert("drift_pct".to_string(), json!(drift_pct));
    out.insert("buy_only".to_string(), json!(buy_only));
    out.insert("capital".to_string(), json!(capital));
    out.insert(
        "currency".to_string(),
        json!(if market == Market::KR { "KRW" } else { "USD" }),
    );
    out.insert("costs".to_string(), json!(costs));
    out.insert("warnings".to_string(), json!(warn_codes));
    if let Some(ro) = res.as_object() {
        for (k, v) in ro {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Json(Value::Object(out)))
}

async fn screener_presets(AuthUser(_): AuthUser) -> Json<Value> {
    Json(screener::presets_meta())
}

async fn screener_run(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let market = Market::parse(body.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let preset = body
        .get("preset")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("momentum_quality");
    let default_uni = if market == Market::KR { "kr_all_150" } else { "us_sp100" };
    let universe = body
        .get("universe")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_uni);
    let custom: Option<Vec<String>> = match body.get("symbols") {
        Some(Value::String(s)) => Some(
            s.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
        ),
        Some(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
        ),
        _ => None,
    };
    let top_n = parse_num(&body, "top_n", 20.0, 1.0, 40.0)? as usize;
    let exclude_warnings = body
        .get("exclude_warnings")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let strict_only = body
        .get("strict_only")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let strategy_id = body.get("strategy").and_then(|v| v.as_str());

    let res = screener::run_screen(
        &state,
        uid,
        market,
        preset,
        universe,
        custom.as_deref(),
        strategy_id,
        exclude_warnings,
        strict_only,
        top_n,
    )
    .await?;
    Ok(Json(res))
}

async fn screener_rotation_backtest(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("");
    let market = Market::parse(body.get("market").and_then(|v| v.as_str()).unwrap_or(""))?;
    let preset = body
        .get("preset")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("momentum_quality");
    let default_uni = if market == Market::KR { "kr_all_150" } else { "us_sp100" };
    let universe = body
        .get("universe")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_uni);
    let custom: Option<Vec<String>> = match body.get("symbols") {
        Some(Value::String(s)) => Some(
            s.split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
        ),
        Some(Value::Array(arr)) => Some(
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect(),
        ),
        _ => None,
    };
    let top_n = parse_num(&body, "top_n", 5.0, 2.0, 15.0)? as usize;
    let default_hb = (top_n * 2).max(10) as f64;
    let hold_buffer = parse_num(&body, "hold_buffer", default_hb, top_n as f64, 40.0)? as usize;
    let cadence = body
        .get("cadence")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("monthly");
    let period = body
        .get("period")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("3y");
    let def = cost_defaults(&state, uid, market).await;
    let capital = parse_num(
        &body,
        "capital",
        def.get("capital").and_then(|v| v.as_f64()).unwrap_or(10_000_000.0),
        1.0,
        1e13,
    )?;

    let res = crate::autotrade::rotation::run_rotation_backtest(
        &state,
        uid,
        market,
        preset,
        universe,
        custom.as_deref(),
        top_n,
        hold_buffer,
        cadence,
        period,
        capital,
        None,
    )
    .await?;
    Ok(Json(res))
}
