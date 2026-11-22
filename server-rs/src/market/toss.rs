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
use crate::integrations::toss::TossClient;
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
        let key = (
            user_id.to_string(),
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

        let client = state.get_toss_client(user_id)?;
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
        let day_key = (start_ts.max(0) / 86400) as usize;
        let key = (
            user_id.to_string(),
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

        let client = state.get_toss_client(user_id)?;
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
        market: Market,
        symbol: &str,
        p: &Value,
    ) -> ApiResult<Quote> {
        let price = p
            .get("lastPrice")
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .ok_or_else(|| ApiError::not_found("stock_not_found", symbol))?;
        let daily = self.fetch_base(state, user_id, symbol, "1d", 2).await?;
        let prev = if daily.len() >= 2 {
            daily[daily.len() - 2].close
        } else {
            price
        };
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

        Ok(Quote {
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
        })
    }

    pub async fn get_quote(
        &self,
        state: &AppState,
        user_id: &str,
        market: Market,
        symbol: &str,
    ) -> ApiResult<Quote> {
        Self::check(market, symbol)?;
        let client = state.get_toss_client(user_id)?;
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
        self.build_quote(state, user_id, market, symbol, first)
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
        let mut valid = Vec::new();
        for sym in symbols {
            match Self::check(market, sym) {
                Ok(()) => valid.push(sym.clone()),
                Err(e) => {
                    out.insert(sym.clone(), Err(e));
                }
            }
        }
        if valid.is_empty() {
            return Ok(out);
        }
        let client = state.get_toss_client(user_id)?;
        let mut params = HashMap::new();
        params.insert("symbols".to_string(), valid.join(","));
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
        for sym in valid {
            match by_sym.get(&sym) {
                Some(row) if !row.get("lastPrice").unwrap_or(&Value::Null).is_null() => {
                    out.insert(
                        sym.clone(),
                        self.build_quote(state, user_id, market, &sym, row).await,
                    );
                }
                _ => {
                    out.insert(sym.clone(), Err(ApiError::not_found("stock_not_found", &sym)));
                }
            }
        }
        Ok(out)
    }

    async fn enrich_chunk(client: &TossClient, syms: &[String]) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if syms.is_empty() {
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
        let key = (user_id.to_string(), market);
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

        let client = state.get_toss_client(user_id)?;
        let markets: &[&str] = match market {
            Market::KR => &["KOSPI", "KOSDAQ", "KR_ETC"],
            Market::US => &["NASDAQ", "NYSE", "AMEX", "US_ETC"],
        };
        let mut items: Vec<Instrument> = Vec::new();
        for (i, m) in markets.iter().enumerate() {
            if i > 0 {
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
