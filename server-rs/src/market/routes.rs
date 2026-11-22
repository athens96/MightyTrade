use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::extras;
use super::models::{Interval, Market};
use crate::auth::extractors::AuthUser;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/candles", get(candles))
        .route("/quote", get(quote))
        .route("/quotes", get(quotes))
        .route("/search", get(search))
        .route("/overview", get(overview))
        .route("/warnings", get(warnings))
        .route("/investor-flow", get(investor_flow))
        .route("/orderbook", get(orderbook))
        .route("/regimes", get(regimes))
}

#[derive(Deserialize)]
struct MarketSymbolQuery {
    market: Option<String>,
    symbol: Option<String>,
    interval: Option<String>,
    limit: Option<i64>,
    count: Option<i64>,
}

#[derive(Deserialize)]
struct QuotesQuery {
    market: Option<String>,
    symbols: Option<String>,
}

#[derive(Deserialize)]
struct SearchQuery {
    market: Option<String>,
    q: Option<String>,
}

fn parse_symbol(raw: Option<&str>) -> ApiResult<String> {
    let s = raw.unwrap_or("").trim().to_uppercase();
    if s.is_empty() || s.len() > 20 {
        return Err(ApiError::code_only(400, "invalid_symbol"));
    }
    Ok(s)
}

async fn candles(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketSymbolQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let symbol = parse_symbol(q.symbol.as_deref())?;
    let interval = Interval::parse(q.interval.as_deref().unwrap_or("1d"))?;
    let limit = q.limit.unwrap_or(200).clamp(1, 1000) as usize;
    let uid = user["id"].as_str().unwrap_or("");
    let (provider, data) =
        super::get_candles(&state, uid, market, &symbol, interval, limit).await?;
    Ok(Json(json!({
        "market": market.as_str(),
        "symbol": symbol,
        "interval": interval.as_str(),
        "provider": provider,
        "candles": data,
    })))
}

async fn quote(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketSymbolQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let symbol = parse_symbol(q.symbol.as_deref())?;
    let uid = user["id"].as_str().unwrap_or("");
    let res = super::get_quote(&state, uid, market, &symbol).await?;
    Ok(Json(json!({ "quote": res })))
}

async fn quotes(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<QuotesQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let mut symbols: Vec<String> = Vec::new();
    for part in q.symbols.unwrap_or_default().split(',') {
        let s = part.trim().to_uppercase();
        if !s.is_empty() && !symbols.contains(&s) {
            symbols.push(s);
        }
    }
    if symbols.is_empty() || symbols.len() > 50 || symbols.iter().any(|s| s.len() > 20) {
        return Err(ApiError::bad_request(
            "invalid_symbols",
            "symbols: 1~50 comma-separated codes",
        ));
    }
    let uid = user["id"].as_str().unwrap_or("");
    let map = super::get_quotes(&state, uid, market, &symbols).await?;
    let mut items = Vec::with_capacity(symbols.len());
    for sym in symbols {
        match map.get(&sym) {
            None => items.push(json!({ "symbol": sym, "error": "not_found" })),
            Some(Err(e)) => items.push(json!({ "symbol": sym, "error": e.code })),
            Some(Ok(qt)) => items.push(serde_json::to_value(qt).unwrap_or(Value::Null)),
        }
    }
    Ok(Json(json!({ "items": items })))
}

async fn search(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<SearchQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let query = q.q.unwrap_or_default();
    if query.trim().is_empty() {
        return Ok(Json(json!({ "items": [] })));
    }
    let uid = user["id"].as_str().unwrap_or("");
    let items = super::search(&state, uid, market, &query).await?;
    Ok(Json(json!({ "items": items })))
}

async fn overview(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(extras::overview(&state, uid).await?))
}

async fn warnings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketSymbolQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let symbol = parse_symbol(q.symbol.as_deref())?;
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(json!({ "items": extras::warnings(&state, uid, market, &symbol).await? })))
}

async fn investor_flow(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketSymbolQuery>,
) -> ApiResult<Json<Value>> {
    let symbol = parse_symbol(q.symbol.as_deref())?;
    let count = q.count.unwrap_or(20).clamp(1, 100) as usize;
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(extras::investor_flow(&state, uid, &symbol, count).await?))
}

async fn orderbook(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<MarketSymbolQuery>,
) -> ApiResult<Json<Value>> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let symbol = parse_symbol(q.symbol.as_deref())?;
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(extras::orderbook(&state, uid, market, &symbol).await?))
}

async fn regimes(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let kr = crate::quant::market_timing::current_regime_snapshot(&state, uid, "KR", None, 20, 60, None).await;
    let us = crate::quant::market_timing::current_regime_snapshot(&state, uid, "US", None, 20, 60, None).await;
    Ok(Json(json!({ "KR": kr, "US": us })))
}
