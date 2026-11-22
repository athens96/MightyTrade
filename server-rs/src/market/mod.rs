pub mod aggregate;
pub mod extras;
pub mod mock;
pub mod models;
pub mod routes;
pub mod toss;

use std::collections::HashMap;

use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use models::{Candle, Instrument, Interval, Market, Quote};

pub fn provider_name(state: &AppState, market: Market) -> String {
    state
        .config
        .market_providers
        .get(market.as_str())
        .cloned()
        .unwrap_or_else(|| "mock".to_string())
}

pub async fn get_candles(
    state: &AppState,
    user_id: &str,
    market: Market,
    symbol: &str,
    interval: Interval,
    limit: usize,
) -> ApiResult<(String, Vec<Candle>)> {
    let name = provider_name(state, market);
    if name == "toss" {
        let data = state
            .market_toss
            .get_candles(state, user_id, market, symbol, interval, limit)
            .await?;
        Ok((name, data))
    } else {
        Ok(("mock".to_string(), mock::get_candles(market, symbol, interval, limit)))
    }
}

pub async fn get_history(
    state: &AppState,
    user_id: &str,
    market: Market,
    symbol: &str,
    interval: Interval,
    start_ts: i64,
) -> ApiResult<Vec<Candle>> {
    if provider_name(state, market) == "toss" {
        state
            .market_toss
            .get_history(state, user_id, market, symbol, interval, start_ts)
            .await
    } else {
        Ok(mock::get_history(market, symbol, interval, start_ts))
    }
}

pub async fn get_quote(
    state: &AppState,
    user_id: &str,
    market: Market,
    symbol: &str,
) -> ApiResult<Quote> {
    if provider_name(state, market) == "toss" {
        state
            .market_toss
            .get_quote(state, user_id, market, symbol)
            .await
    } else {
        Ok(mock::get_quote(market, symbol))
    }
}

pub async fn get_quotes(
    state: &AppState,
    user_id: &str,
    market: Market,
    symbols: &[String],
) -> ApiResult<HashMap<String, Result<Quote, ApiError>>> {
    if provider_name(state, market) == "toss" {
        state
            .market_toss
            .get_quotes(state, user_id, market, symbols)
            .await
    } else {
        let mut out = HashMap::new();
        for sym in symbols {
            out.insert(sym.clone(), Ok(mock::get_quote(market, sym)));
        }
        Ok(out)
    }
}

pub async fn search(
    state: &AppState,
    user_id: &str,
    market: Market,
    query: &str,
) -> ApiResult<Vec<Instrument>> {
    if provider_name(state, market) == "toss" {
        state
            .market_toss
            .search(state, user_id, market, query)
            .await
    } else {
        Ok(mock::search(market, query))
    }
}
