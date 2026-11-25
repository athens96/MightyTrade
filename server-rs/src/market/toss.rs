use chrono::{DateTime, TimeZone, Utc};
use dashmap::DashMap;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

use super::aggregate::{resample_intraday, resample_weekly};
use super::models::{Candle, Instrument, Interval, Market, Quote};
use crate::errors::{ApiError, ApiResult};
use crate::integrations::broker::BrokerClient;
use crate::state::AppState;

const PAGE_SIZE: usize = 200;
const MAX_PAGES: usize = 10;
const HISTORY_MAX_PAGES: usize = 40;
const HISTORY_TTL: f64 = 300.0;
const STOCK_LIST_TTL: f64 = 12.0 * 3600.0;
const SEARCH_LIMIT: usize = 30;
const STOCK_DETAIL_BATCH: usize = 200;
const STOCK_DETAIL_SYNC_LIMIT: usize = 400;
const STOCK_DETAIL_PACE_MS: u64 = 250;
const MAX_CANDLE_CACHE: usize = 512;
const OHLC_CACHE_TTL: f64 = 300.0;
const OHLC_NEG_TTL: f64 = 60.0;
const QUOTE_CACHE_TTL: f64 = 3.0;

#[derive(Clone, Copy)]
struct DailyOhlc {
    prev_close: Option<f64>,
    open: Option<f64>,
    high: Option<f64>,
    low: Option<f64>,
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn to_candle(raw: &Value, base: &str) -> Option<Candle> {
    let ts_str = raw.get("timestamp")?.as_str()?;
    let dt = DateTime::parse_from_rfc3339(ts_str).ok()?;
    let t = if base == "1d" {
        let d = dt.date_naive();
        Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
            .timestamp()
    } else {
        dt.timestamp()
    };
    let f = |k: &str| -> Option<f64> {
        let v = raw.get(k)?;
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    };
    Some(Candle {
        time: t,
        open: f("openPrice")?,
        high: f("highPrice")?,
        low: f("lowPrice")?,
        close: f("closePrice")?,
        volume: f("volume").unwrap_or(0.0),
    })
}

pub struct TossMarketProvider {
    candle_cache: DashMap<(String, String, String, usize), (f64, Vec<Candle>)>,
    ohlc_cache: DashMap<(String, String), (f64, Option<DailyOhlc>)>,
    quote_cache: DashMap<(String, String), (f64, Quote)>,
    stocks: DashMap<(String, Market), (f64, Vec<Instrument>)>,
    stock_locks: DashMap<(String, Market), Arc<AsyncMutex<()>>>,
}

impl Default for TossMarketProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TossMarketProvider {
    pub fn new() -> Self {
        Self {
            candle_cache: DashMap::new(),
            ohlc_cache: DashMap::new(),
            quote_cache: DashMap::new(),
            stocks: DashMap::new(),
            stock_locks: DashMap::new(),
        }
    }

    fn check(market: Market, symbol: &str) -> ApiResult<()> {
        if !market.validate_symbol(symbol) {
            return Err(ApiError::bad_request(
                "symbol_market_mismatch",
                format!("{symbol} is not a {} symbol", market.as_str()),
            ));
        }
        Ok(())
    }

    async fn fetch_base(
        &self,
        state: &AppState,
        user_id: &str,
        symbol: &str,
        base: &str,
        needed: usize,
    ) -> ApiResult<Vec<Candle>> {
        let client = state.get_broker_client(user_id)?;
        let scoped_uid = format!("{user_id}:{}", client.kind().as_str());
        let key = (
            scoped_uid,
            symbol.to_string(),
            base.to_string(),
            needed,
        );
        let now = now_secs();
        if let Some(hit) = self.candle_cache.get(&key) {
            if hit.0 > now {
                return Ok(hit.1.clone());
            }
        }

        let mut by_time: BTreeMap<i64, Candle> = BTreeMap::new();
        let mut before: Option<String> = None;

        for _ in 0..MAX_PAGES {
            let remaining = needed.saturating_sub(by_time.len()).max(1);
            let mut params = HashMap::new();
            params.insert("symbol".to_string(), symbol.to_string());
            params.insert("interval".to_string(), base.to_string());
            params.insert("count".to_string(), PAGE_SIZE.min(remaining).to_string());
            if let Some(ref b) = before {
                params.insert("before".to_string(), b.clone());
            }
            let result = client
                .request("GET", "/api/v1/candles", Some(&params), None, None)
                .await?
                .unwrap_or(Value::Null);
            let rows = result
                .get("candles")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for r in &rows {
                if let Some(c) = to_candle(r, base) {
                    by_time.insert(c.time, c);
                }
            }
            before = result
                .get("nextBefore")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if before.is_none() || by_time.len() >= needed {
                break;
            }
        }

        let candles: Vec<Candle> = by_time.into_values().collect();
        let ttl = if base == "1m" { 10.0 } else { 60.0 };
        self.candle_cache.retain(|_, v| v.0 > now);
        if self.candle_cache.len() >= MAX_CANDLE_CACHE {
            if let Some(oldest_k) = self
                .candle_cache
                .iter()
                .min_by(|a, b| a.value().0.total_cmp(&b.value().0))
                .map(|e| e.key().clone())
            {
                self.candle_cache.remove(&oldest_k);
            }
        }
        self.candle_cache.insert(key, (now + ttl, candles.clone()));
        Ok(candles)
    }

    pub async fn get_candles(
        &self,
        state: &AppState,
        user_id: &str,
        market: Market,
        symbol: &str,
        interval: Interval,
        limit: usize,
    ) -> ApiResult<Vec<Candle>> {
        Self::check(market, symbol)?;
        let out = match interval {
            Interval::M1 | Interval::D1 => {
                self.fetch_base(state, user_id, symbol, interval.as_str(), limit)
                    .await?
            }
            Interval::W1 => {
                let needed = ((limit + 1) * 5).min(PAGE_SIZE * MAX_PAGES);
                let daily = self.fetch_base(state, user_id, symbol, "1d", needed).await?;
                resample_weekly(&daily)
            }
            _ => {
                let factor = (interval.seconds() / 60) as usize;
                let needed = ((limit + 1) * factor).min(PAGE_SIZE * MAX_PAGES);
                let minutes = self.fetch_base(state, user_id, symbol, "1m", needed).await?;
                resample_intraday(&minutes, interval.seconds())
            }
        };
        let skip = out.len().saturating_sub(limit);
        Ok(out[skip..].to_vec())
    }

    pub async fn get_history(
        &self,
        state: &AppState,
        user_id: &str,
        market: Market,
        symbol: &str,
        interval: Interval,
        start_ts: i64,
    ) -> ApiResult<Vec<Candle>> {
        Self::check(market, symbol)?;
        if interval != Interval::D1 {
            return Err(ApiError::bad_request(
                "unsupported_interval",
                "백테스트는 현재 일봉만 지원합니다",
            ));
        }
        let client = state.get_broker_client(user_id)?;
        let day_key = (start_ts.max(0) / 86400) as usize;
        let key = (
            format!("{user_id}:{}", client.kind().as_str()),
            symbol.to_string(),
            "1d-history".to_string(),
            day_key,
        );
        let now = now_secs();
        if let Some(hit) = self.candle_cache.get(&key) {
            if hit.0 > now {
                return Ok(hit.1.clone());
            }
        }

        let mut by_time: BTreeMap<i64, Candle> = BTreeMap::new();
        let mut before: Option<String> = None;

        for _ in 0..HISTORY_MAX_PAGES {
            let mut params = HashMap::new();
            params.insert("symbol".to_string(), symbol.to_string());
            params.insert("interval".to_string(), "1d".to_string());
            params.insert("count".to_string(), PAGE_SIZE.to_string());
            if let Some(ref b) = before {
                params.insert("before".to_string(), b.clone());
            }
            let result = client
                .request("GET", "/api/v1/candles", Some(&params), None, None)
                .await?
                .unwrap_or(Value::Null);
            let rows = result
                .get("candles")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let mut page_min: Option<i64> = None;
            for r in &rows {
                if let Some(c) = to_candle(r, "1d") {
                    page_min = Some(page_min.map_or(c.time, |m| m.min(c.time)));
                    by_time.insert(c.time, c);
                }
            }
            before = result
                .get("nextBefore")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            if before.is_none() || page_min.is_none() || page_min.unwrap() <= start_ts {
                break;
            }
        }

        let candles: Vec<Candle> = by_time.into_values().collect();
        self.candle_cache
            .insert(key, (now + HISTORY_TTL, candles.clone()));
        Ok(candles)
    }

    async fn build_quote(
        &self,
        state: &AppState,
        user_id: &str,
        broker_kind: &str,
        market: Market,
        symbol: &str,
        p: &Value,
    ) -> ApiResult<Quote> {
        let price = p
            .get("lastPrice")
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .ok_or_else(|| ApiError::not_found("stock_not_found", symbol))?;
        let pos_f = |k: &str| -> Option<f64> {
            p.get(k)
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .filter(|x| x.is_finite() && *x > 0.0)
        };
        let mut prev_opt = pos_f("prevClose");
        let mut open = pos_f("openPrice");
        let mut high = pos_f("highPrice");
        let mut low = pos_f("lowPrice");

        let cache_key = (format!("{user_id}:{broker_kind}"), symbol.to_string());
        let now = now_secs();
        let all_from_price =
            prev_opt.is_some() && open.is_some() && high.is_some() && low.is_some();

        if !all_from_price {
            let cached = self
                .ohlc_cache
                .get(&cache_key)
                .and_then(|e| if e.0 > now { Some(e.1) } else { None });
            if let Some(hit_opt) = cached {
                if let Some(ohlc) = hit_opt {
                    if prev_opt.is_none() {
                        prev_opt = ohlc.prev_close;
                    }
                    if open.is_none() {
                        open = ohlc.open;
                    }
                    if high.is_none() {
                        high = ohlc.high;
                    }
                    if low.is_none() {
                        low = ohlc.low;
                    }
                }
            } else {
                // Only hit 1d candle endpoint once per 5 minutes (60s negative cache on failure)
                // so frequent watchlist quote polling never triggers broker 429 storms.
                match self.fetch_base(state, user_id, symbol, "1d", 2).await {
                    Ok(daily) => {
                        let d_prev = if daily.len() >= 2 {
                            Some(daily[daily.len() - 2].close).filter(|x| *x > 0.0)
                        } else {
                            None
                        };
                        let last_bar = daily.last();
                        let d_open = last_bar.map(|b| b.open).filter(|x| *x > 0.0);
                        let d_high = last_bar.map(|b| b.high).filter(|x| *x > 0.0);
                        let d_low = last_bar.map(|b| b.low).filter(|x| *x > 0.0);
                        if prev_opt.is_none() {
                            prev_opt = d_prev;
                        }
                        if open.is_none() {
                            open = d_open;
                        }
                        if high.is_none() {
                            high = d_high;
                        }
                        if low.is_none() {
                            low = d_low;
                        }
                        self.ohlc_cache.insert(
                            cache_key.clone(),
                            (
                                now + OHLC_CACHE_TTL,
                                Some(DailyOhlc {
                                    prev_close: prev_opt,
                                    open,
                                    high: high.map(|h| h.max(price)),
                                    low: low.map(|l| l.min(price)),
                                }),
                            ),
                        );
                    }
                    Err(_) => {
                        self.ohlc_cache
                            .insert(cache_key.clone(), (now + OHLC_NEG_TTL, None));
                    }
                }
            }
        }

        let prev = prev_opt.unwrap_or(price);
        // Keep intraday high/low consistent with the latest real-time price & refresh cache in-memory
        let eff_open = open.unwrap_or(price);
        let high = high.map(|h| h.max(price).max(eff_open));
        let low = low.map(|l| l.min(price).min(eff_open));
        if open.is_some() || high.is_some() || low.is_some() {
            if let Some(mut entry) = self.ohlc_cache.get_mut(&cache_key) {
                if let Some(ref mut ohlc) = entry.1 {
                    if let Some(h) = high {
                        ohlc.high = Some(ohlc.high.map_or(h, |old| old.max(h)));
                    }
                    if let Some(l) = low {
                        ohlc.low = Some(ohlc.low.map_or(l, |old| old.min(l)));
                    }
                }
            } else if all_from_price {
                self.ohlc_cache.insert(
                    cache_key.clone(),
                    (
                        now + OHLC_CACHE_TTL,
                        Some(DailyOhlc {
                            prev_close: Some(prev),
                            open,
                            high,
                            low,
                        }),
                    ),
                );
            }
        }

        let change = price - prev;
        let ts = p
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_ts)
            .map(|d| d.timestamp())
            .unwrap_or_else(|| Utc::now().timestamp());
        let currency = p
            .get("currency")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(if market == Market::KR { "KRW" } else { "USD" })
            .to_string();

        let quote = Quote {
            market: market.as_str().to_string(),
            symbol: symbol.to_string(),
            price,
            change: (change * 10000.0).round() / 10000.0,
            change_pct: if prev != 0.0 {
                (change / prev * 100.0 * 100.0).round() / 100.0
            } else {
                0.0
            },
            currency,
            time: ts,
            open,
            high,
            low,
            prev_close: if prev > 0.0 { Some(prev) } else { None },
        };
        self.quote_cache
            .insert(cache_key, (now + QUOTE_CACHE_TTL, quote.clone()));
        Ok(quote)
    }

    pub async fn get_quote(
        &self,
        state: &AppState,
        user_id: &str,
        market: Market,
        symbol: &str,
    ) -> ApiResult<Quote> {
        Self::check(market, symbol)?;
        let client = state.get_broker_client(user_id)?;
        let bkind = client.kind().as_str();
        let mut params = HashMap::new();
        params.insert("symbols".to_string(), symbol.to_string());
        let rows = client
            .request("GET", "/api/v1/prices", Some(&params), None, None)
            .await?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let first = rows
            .first()
            .ok_or_else(|| ApiError::not_found("stock_not_found", symbol))?;
        self.build_quote(state, user_id, bkind, market, symbol, first)
            .await
    }

    pub async fn get_quotes(
        &self,
        state: &AppState,
        user_id: &str,
        market: Market,
        symbols: &[String],
    ) -> ApiResult<HashMap<String, Result<Quote, ApiError>>> {
        let mut out: HashMap<String, Result<Quote, ApiError>> = HashMap::new();
        let client = state.get_broker_client(user_id)?;
        let bkind = client.kind().as_str();
        let scoped_uid = format!("{user_id}:{bkind}");
        let now = now_secs();

        let mut to_fetch = Vec::new();
        for sym in symbols {
            match Self::check(market, sym) {
                Ok(()) => {
                    if let Some(hit) = self.quote_cache.get(&(scoped_uid.clone(), sym.clone())) {
                        if hit.0 > now {
                            out.insert(sym.clone(), Ok(hit.1.clone()));
                            continue;
                        }
                    }
                    to_fetch.push(sym.clone());
                }
                Err(e) => {
                    out.insert(sym.clone(), Err(e));
                }
            }
        }
        if to_fetch.is_empty() {
            return Ok(out);
        }
        let mut params = HashMap::new();
        params.insert("symbols".to_string(), to_fetch.join(","));
        let rows = client
            .request("GET", "/api/v1/prices", Some(&params), None, None)
            .await?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut by_sym: HashMap<String, Value> = HashMap::new();
        for r in rows {
            if let Some(s) = r.get("symbol").and_then(|v| v.as_str()) {
                by_sym.insert(s.to_string(), r);
            }
        }
        for sym in to_fetch {
            match by_sym.get(&sym) {
                Some(row) if !row.get("lastPrice").unwrap_or(&Value::Null).is_null() => {
                    out.insert(
                        sym.clone(),
                        self.build_quote(state, user_id, bkind, market, &sym, row)
                            .await,
                    );
                }
                _ => {
                    out.insert(sym.clone(), Err(ApiError::not_found("stock_not_found", &sym)));
                }
            }
        }
        Ok(out)
    }

    async fn enrich_chunk(client: &BrokerClient, syms: &[String]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if syms.is_empty() || client.is_kiwoom() {
            return out;
        }
        let mut params = HashMap::new();
        params.insert("symbols".to_string(), syms.join(","));
        if let Ok(Some(Value::Array(rows))) = client
            .request("GET", "/api/v1/stocks", Some(&params), None, None)
            .await
        {
            for r in rows {
                let sym = r.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
                let eng = r
                    .get("englishName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if !sym.is_empty() && !eng.is_empty() {
                    out.insert(sym.to_string(), eng.to_string());
                }
            }
        }
        out
    }

    async fn stock_list(
        self: &Arc<Self>,
        state: &AppState,
        user_id: &str,
        market: Market,
    ) -> ApiResult<Vec<Instrument>> {
        let client = state.get_broker_client(user_id)?;
        let key = (format!("{user_id}:{}", client.kind().as_str()), market);
        let now = now_secs();
        if let Some(hit) = self.stocks.get(&key) {
            if hit.0 > now {
                return Ok(hit.1.clone());
            }
        }

        let lock = self
            .stock_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        let now = now_secs();
        if let Some(hit) = self.stocks.get(&key) {
            if hit.0 > now {
                return Ok(hit.1.clone());
            }
        }

        let markets: &[&str] = match market {
            Market::KR => &["KOSPI", "KOSDAQ", "KR_ETC"],
            Market::US => &["NASDAQ", "NYSE", "AMEX", "US_ETC"],
        };
        let mut items: Vec<Instrument> = Vec::new();
        for (i, m) in markets.iter().enumerate() {
            if i > 0 && !client.is_kiwoom() {
                tokio::time::sleep(Duration::from_millis(1050)).await;
            }
            let mut params = HashMap::new();
            params.insert("market".to_string(), (*m).to_string());
            params.insert("status".to_string(), "ACTIVE".to_string());
            let rows = client
                .request("GET", "/api/v1/stocks/all", Some(&params), None, None)
                .await?
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            for s in rows {
                let sym = s.get("symbol").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let eng = s
                    .get("englishName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .filter(|x| !x.is_empty())
                    .map(|x| x.to_string())
                    .unwrap_or_else(|| eng.clone());
                let exch = s
                    .get("market")
                    .and_then(|v| v.as_str())
                    .unwrap_or(m)
                    .to_string();
                items.push(Instrument {
                    market: market.as_str().to_string(),
                    symbol: sym,
                    name,
                    exchange: exch,
                    english_name: eng,
                });
            }
        }

        if market == Market::US {
            let missing: Vec<String> = items
                .iter()
                .filter(|it| it.english_name.is_empty())
                .map(|it| it.symbol.clone())
                .collect();
            let sync_end = missing.len().min(STOCK_DETAIL_SYNC_LIMIT);
            for chunk in missing[..sync_end].chunks(STOCK_DETAIL_BATCH) {
                let enriched = Self::enrich_chunk(&client, chunk).await;
                for it in &mut items {
                    if let Some(eng) = enriched.get(&it.symbol) {
                        it.english_name = eng.clone();
                    }
                }
            }
            if missing.len() > sync_end {
                let bg_syms = missing[sync_end..].to_vec();
                let me = Arc::clone(self);
                let client_bg = client.clone();
                let bg_key = key.clone();
                tokio::spawn(async move {
                    for chunk in bg_syms.chunks(STOCK_DETAIL_BATCH) {
                        tokio::time::sleep(Duration::from_millis(STOCK_DETAIL_PACE_MS)).await;
                        let enriched = Self::enrich_chunk(&client_bg, chunk).await;
                        if let Some(mut entry) = me.stocks.get_mut(&bg_key) {
                            for it in &mut entry.1 {
                                if let Some(eng) = enriched.get(&it.symbol) {
                                    it.english_name = eng.clone();
                                }
                            }
                        }
                    }
                });
            }
        }

        let done = now_secs();
        self.stocks.retain(|_, v| v.0 > done);
        self.stocks.insert(key, (done + STOCK_LIST_TTL, items.clone()));
        Ok(items)
    }

    pub async fn search(
        self: &Arc<Self>,
        state: &AppState,
        user_id: &str,
        market: Market,
        query: &str,
    ) -> ApiResult<Vec<Instrument>> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let list = self.stock_list(state, user_id, market).await?;
        let mut scored: Vec<(u8, usize, Instrument)> = Vec::new();
        for inst in list {
            let sym = inst.symbol.to_lowercase();
            let name = inst.name.to_lowercase();
            let eng = inst.english_name.to_lowercase();
            let rank = if sym == q {
                0
            } else if sym.starts_with(&q) {
                1
            } else if name.starts_with(&q) || eng.starts_with(&q) {
                2
            } else if name.contains(&q) || eng.contains(&q) {
                3
            } else {
                continue;
            };
            scored.push((rank, inst.name.chars().count(), inst));
        }
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        Ok(scored.into_iter().take(SEARCH_LIMIT).map(|x| x.2).collect())
    }
}
