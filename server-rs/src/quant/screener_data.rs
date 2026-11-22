//! Fast async public market-data pipeline (Naver Finance KR, Yahoo Finance US) with TTL caches.

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use dashmap::DashMap;
use encoding_rs::EUC_KR;
use futures_util::stream::{self, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::quant::engine::Bars;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
const BARS_TTL: f64 = 600.0;
const FLOW_TTL: f64 = 600.0;
const LIST_TTL: f64 = 6.0 * 3600.0;
const MAX_BARS_CACHE: usize = 800;

static PREF_SUFFIX_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(우[A-Z0-9]?|우B|스팩\d*호?|리츠|인프라)$").unwrap());
static INT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[+-]?\d+").unwrap());
static FLOAT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[+-]?\d+(?:\.\d+)?").unwrap());
static FCHART_ROW_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"data="(\d{8})\|([^"]+)""#).unwrap());

static BARS_CACHE: Lazy<DashMap<(String, String, String), (f64, Bars)>> = Lazy::new(DashMap::new);
static FLOW_CACHE: Lazy<DashMap<String, (f64, Value)>> = Lazy::new(DashMap::new);
static LIST_CACHE: Lazy<DashMap<String, (f64, Vec<(String, String)>)>> = Lazy::new(DashMap::new);

static PUBLIC_HTTP: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(6))
        .build()
        .expect("public http client")
});

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn clean_int(raw: Option<&Value>) -> Option<i64> {
    let v = raw?;
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    if let Some(f) = v.as_f64() {
        return Some(f as i64);
    }
    let s = v.as_str()?.replace(',', "");
    INT_RE.find(s.trim())?.as_str().parse().ok()
}

fn clean_float(raw: Option<&Value>) -> Option<f64> {
    let v = raw?;
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    let s = v.as_str()?.replace(',', "");
    FLOAT_RE.find(s.trim())?.as_str().parse().ok()
}

fn is_ordinary_kr_stock(item: &Value) -> bool {
    let code = item.get("itemCode").and_then(|v| v.as_str()).unwrap_or("").trim();
    let name = item.get("stockName").and_then(|v| v.as_str()).unwrap_or("").trim();
    let end_type = item
        .get("stockEndType")
        .and_then(|v| v.as_str())
        .unwrap_or("stock")
        .to_lowercase();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if end_type != "stock" {
        return false;
    }
    if !code.ends_with('0') {
        return false;
    }
    if PREF_SUFFIX_RE.is_match(name) || name.contains("스팩") {
        return false;
    }
    true
}

pub async fn fetch_kr_market_leaders(board: &str, limit: usize) -> Vec<(String, String)> {
    let key = format!("{board}:{limit}");
    let now = now_secs();
    if let Some(hit) = LIST_CACHE.get(&key) {
        if hit.0 > now {
            return hit.1.clone();
        }
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=3 {
        if out.len() >= limit {
            break;
        }
        let url = format!("https://m.stock.naver.com/api/stocks/marketValue/{board}");
        let Ok(resp) = PUBLIC_HTTP
            .get(&url)
            .query(&[("page", page.to_string()), ("pageSize", "100".to_string())])
            .send()
            .await
        else {
            break;
        };
        if !resp.status().is_success() {
            break;
        }
        let Ok(val) = resp.json::<Value>().await else {
            break;
        };
        let Some(stocks) = val.get("stocks").and_then(|v| v.as_array()) else {
            break;
        };
        if stocks.is_empty() {
            break;
        }
        for st in stocks {
            if !is_ordinary_kr_stock(st) {
                continue;
            }
            let code = st["itemCode"].as_str().unwrap_or("").trim().to_string();
            let name = st["stockName"].as_str().unwrap_or("").trim().to_string();
            if seen.insert(code.clone()) {
                out.push((code, name));
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    if !out.is_empty() {
        LIST_CACHE.insert(key, (now + LIST_TTL, out.clone()));
    }
    out
}

async fn fetch_kr_bars_one(symbol: &str, count: usize) -> Option<Bars> {
    let resp = PUBLIC_HTTP
        .get("https://fchart.stock.naver.com/sise.nhn")
        .query(&[
            ("symbol", symbol),
            ("timeframe", "day"),
            ("count", &count.to_string()),
            ("requestType", "0"),
        ])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let (decoded, _, _) = EUC_KR.decode(&bytes);
    let mut times = Vec::new();
    let mut opens = Vec::new();
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut closes = Vec::new();
    let mut vols = Vec::new();

    for cap in FCHART_ROW_RE.captures_iter(&decoded) {
        let ymd = &cap[1];
        let parts: Vec<&str> = cap[2].split('|').collect();
        if parts.len() < 5 {
            continue;
        }
        let Ok(d) = NaiveDate::parse_from_str(ymd, "%Y%m%d") else {
            continue;
        };
        let (Ok(mut o), Ok(mut h), Ok(mut lo), Ok(c), Ok(v)) = (
            parts[0].parse::<f64>(),
            parts[1].parse::<f64>(),
            parts[2].parse::<f64>(),
            parts[3].parse::<f64>(),
            parts[4].parse::<f64>(),
        ) else {
            continue;
        };
        if c <= 0.0 {
            continue;
        }
        if o <= 0.0 {
            o = c;
        }
        if h <= 0.0 {
            h = o.max(c);
        }
        if lo <= 0.0 {
            lo = o.min(c);
        }
        let ts = Utc.from_utc_datetime(&d.and_hms_opt(0, 0, 0)?).timestamp();
        times.push(ts);
        opens.push(o);
        highs.push(h);
        lows.push(lo);
        closes.push(c);
        vols.push(v);
    }
    if times.len() < 20 {
        return None;
    }
    Some(Bars {
        time: times,
        open: opens,
        high: highs,
        low: lows,
        close: closes,
        volume: vols,
    })
}

async fn fetch_us_bars_one(symbol: &str, range_str: &str) -> Option<Bars> {
    let ysym = symbol.replace('.', "-");
    let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/{ysym}");
    let resp = PUBLIC_HTTP
        .get(&url)
        .query(&[("range", range_str), ("interval", "1d")])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let val: Value = resp.json().await.ok()?;
    let res = val.get("chart")?.get("result")?.as_array()?.first()?;
    let raw_t = res.get("timestamp")?.as_array()?;
    let quote = res.get("indicators")?.get("quote")?.as_array()?.first()?;
    let raw_o = quote.get("open").and_then(|v| v.as_array());
    let raw_h = quote.get("high").and_then(|v| v.as_array());
    let raw_l = quote.get("low").and_then(|v| v.as_array());
    let raw_c = quote.get("close").and_then(|v| v.as_array())?;
    let raw_v = quote.get("volume").and_then(|v| v.as_array());
    let raw_adj = res
        .get("indicators")
        .and_then(|x| x.get("adjclose"))
        .and_then(|x| x.as_array())
        .and_then(|a| a.first())
        .and_then(|x| x.get("adjclose"))
        .and_then(|v| v.as_array());

    let mut times: Vec<i64> = Vec::new();
    let mut opens = Vec::new();
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut closes = Vec::new();
    let mut vols = Vec::new();

    for (idx, tv) in raw_t.iter().enumerate() {
        let Some(t) = tv.as_i64() else { continue };
        let Some(mut c) = raw_c.get(idx).and_then(|v| v.as_f64()).filter(|&x| x > 0.0) else {
            continue;
        };
        let mut o = raw_o
            .and_then(|a| a.get(idx))
            .and_then(|v| v.as_f64())
            .filter(|&x| x > 0.0)
            .unwrap_or(c);
        let mut h = raw_h
            .and_then(|a| a.get(idx))
            .and_then(|v| v.as_f64())
            .filter(|&x| x > 0.0)
            .unwrap_or_else(|| o.max(c));
        let mut lo = raw_l
            .and_then(|a| a.get(idx))
            .and_then(|v| v.as_f64())
            .filter(|&x| x > 0.0)
            .unwrap_or_else(|| o.min(c));
        let v = raw_v
            .and_then(|a| a.get(idx))
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0);
        if let Some(adj) = raw_adj
            .and_then(|a| a.get(idx))
            .and_then(|x| x.as_f64())
            .filter(|&x| x > 0.0)
        {
            let ratio = adj / c;
            o *= ratio;
            h *= ratio;
            lo *= ratio;
            c = adj;
        }
        let d_utc = DateTime::from_timestamp(t, 0)?.date_naive();
        let day_ts = Utc.from_utc_datetime(&d_utc.and_hms_opt(0, 0, 0)?).timestamp();
        if times.last().copied() == Some(day_ts) {
            *opens.last_mut().unwrap() = o;
            *highs.last_mut().unwrap() = h;
            *lows.last_mut().unwrap() = lo;
            *closes.last_mut().unwrap() = c;
            *vols.last_mut().unwrap() = v;
            continue;
        }
        times.push(day_ts);
        opens.push(o);
        highs.push(h);
        lows.push(lo);
        closes.push(c);
        vols.push(v);
    }
    if times.len() < 20 {
        return None;
    }
    Some(Bars {
        time: times,
        open: opens,
        high: highs,
        low: lows,
        close: closes,
        volume: vols,
    })
}

async fn fetch_us_intraday_one(ysym: &str, interval: &str, range_str: &str) -> Option<Bars> {
    let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/{ysym}");
    let resp = PUBLIC_HTTP
        .get(&url)
        .query(&[("range", range_str), ("interval", interval)])
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let val: Value = resp.json().await.ok()?;
    let res = val.get("chart")?.get("result")?.as_array()?.first()?;
    let raw_t = res.get("timestamp")?.as_array()?;
    let quote = res.get("indicators")?.get("quote")?.as_array()?.first()?;
    let raw_o = quote.get("open").and_then(|v| v.as_array());
    let raw_h = quote.get("high").and_then(|v| v.as_array());
    let raw_l = quote.get("low").and_then(|v| v.as_array());
    let raw_c = quote.get("close").and_then(|v| v.as_array())?;
    let raw_v = quote.get("volume").and_then(|v| v.as_array());

    let mut times: Vec<i64> = Vec::new();
    let mut opens = Vec::new();
    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut closes = Vec::new();
    let mut vols = Vec::new();

    for (idx, tv) in raw_t.iter().enumerate() {
        let Some(ts) = tv.as_i64() else { continue };
        let Some(c) = raw_c.get(idx).and_then(|v| v.as_f64()).filter(|&x| x > 0.0) else {
            continue;
        };
        if times.last().is_some_and(|&prev| prev >= ts) {
            continue;
        }
        let o = raw_o.and_then(|a| a.get(idx)).and_then(|v| v.as_f64()).filter(|&x| x > 0.0).unwrap_or(c);
        let h = raw_h.and_then(|a| a.get(idx)).and_then(|v| v.as_f64()).filter(|&x| x > 0.0).unwrap_or_else(|| o.max(c));
        let lo = raw_l.and_then(|a| a.get(idx)).and_then(|v| v.as_f64()).filter(|&x| x > 0.0).unwrap_or_else(|| o.min(c));
        let v = raw_v.and_then(|a| a.get(idx)).and_then(|x| x.as_f64()).unwrap_or(0.0);
        times.push(ts);
        opens.push(o);
        highs.push(h);
        lows.push(lo);
        closes.push(c);
        vols.push(v);
    }
    if times.len() < 20 {
        return None;
    }
    Some(Bars {
        time: times,
        open: opens,
        high: highs,
        low: lows,
        close: closes,
        volume: vols,
    })
}

pub async fn fetch_public_history(market: &str, symbols: &[String], period: &str) -> HashMap<String, Bars> {
    let kr_count = match period {
        "1y" => 550,
        "3y" => 1100,
        "5y" => 1650,
        "10y" => 3000,
        "max" => 6000,
        _ => 1650,
    };
    let us_range = match period {
        "1y" => "2y",
        "3y" => "5y",
        "5y" => "10y",
        "10y" | "max" => "max",
        _ => "10y",
    };
    let tag = if market == "KR" {
        kr_count.to_string()
    } else {
        us_range.to_string()
    };
    let now = now_secs();
    let mut out = HashMap::new();
    let mut missing = Vec::new();

    for sym in symbols {
        let key = (market.to_string(), sym.clone(), tag.clone());
        if let Some(hit) = BARS_CACHE.get(&key) {
            if hit.0 > now {
                out.insert(sym.clone(), hit.1.clone());
                continue;
            }
        }
        missing.push(sym.clone());
    }

    if !missing.is_empty() {
        let m = market.to_string();
        let fetched: Vec<(String, Option<Bars>)> = stream::iter(missing)
            .map(|sym| {
                let m = m.clone();
                async move {
                    let b = if m == "KR" {
                        fetch_kr_bars_one(&sym, kr_count).await
                    } else {
                        fetch_us_bars_one(&sym, us_range).await
                    };
                    (sym, b)
                }
            })
            .buffer_unordered(12)
            .collect()
            .await;

        if BARS_CACHE.len() > MAX_BARS_CACHE {
            BARS_CACHE.clear();
        }
        for (sym, b) in fetched {
            if let Some(bars) = b {
                BARS_CACHE.insert(
                    (market.to_string(), sym.clone(), tag.clone()),
                    (now + BARS_TTL, bars.clone()),
                );
                out.insert(sym, bars);
            }
        }
    }
    out
}

pub async fn fetch_public_intraday(market: &str, symbol: &str, interval: &str) -> Option<Bars> {
    let now = now_secs();
    let key = (
        market.to_string(),
        format!("{symbol}:{interval}"),
        "60d".to_string(),
    );
    if let Some(hit) = BARS_CACHE.get(&key) {
        if hit.0 > now {
            return Some(hit.1.clone());
        }
    }
    let b = if market == "US" {
        fetch_us_intraday_one(&symbol.replace('.', "-"), interval, "60d").await
    } else {
        match fetch_us_intraday_one(&format!("{symbol}.KS"), interval, "60d").await {
            Some(x) => Some(x),
            None => fetch_us_intraday_one(&format!("{symbol}.KQ"), interval, "60d").await,
        }
    };
    if let Some(ref bars) = b {
        BARS_CACHE.insert(key, (now + 300.0, bars.clone()));
    }
    b
}

pub async fn fetch_public_bars(market: &str, symbols: &[String], count: usize) -> HashMap<String, Bars> {
    let bucket = if count > 350 { 520 } else { 320 };
    let range_str = if bucket > 350 { "2y" } else { "18mo" };
    let tag = format!("sc-{bucket}");
    let now = now_secs();
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    for sym in symbols {
        let key = (market.to_string(), sym.clone(), tag.clone());
        if let Some(hit) = BARS_CACHE.get(&key) {
            if hit.0 > now {
                out.insert(sym.clone(), hit.1.clone());
                continue;
            }
        }
        missing.push(sym.clone());
    }
    if !missing.is_empty() {
        let m = market.to_string();
        let fetched: Vec<(String, Option<Bars>)> = stream::iter(missing)
            .map(|sym| {
                let m = m.clone();
                async move {
                    let b = if m == "KR" {
                        fetch_kr_bars_one(&sym, bucket).await
                    } else {
                        fetch_us_bars_one(&sym, range_str).await
                    };
                    (sym, b)
                }
            })
            .buffer_unordered(12)
            .collect()
            .await;

        if BARS_CACHE.len() > MAX_BARS_CACHE {
            BARS_CACHE.clear();
        }
        for (sym, b) in fetched {
            if let Some(bars) = b {
                BARS_CACHE.insert(
                    (market.to_string(), sym.clone(), tag.clone()),
                    (now + BARS_TTL, bars.clone()),
                );
                out.insert(sym, bars);
            }
        }
    }
    out
}

async fn fetch_kr_flow_one(symbol: String) -> (String, Option<Value>) {
    let url = format!("https://m.stock.naver.com/api/stock/{symbol}/trend");
    let Ok(resp) = PUBLIC_HTTP
        .get(&url)
        .query(&[("pageSize", "20"), ("page", "1")])
        .send()
        .await
    else {
        return (symbol, None);
    };
    if !resp.status().is_success() {
        return (symbol, None);
    }
    let Ok(Value::Array(recs)) = resp.json::<Value>().await else {
        return (symbol, None);
    };
    if recs.is_empty() {
        return (symbol, None);
    }
    let f5: i64 = recs
        .iter()
        .take(5)
        .map(|x| clean_int(x.get("foreignerPureBuyQuant")).unwrap_or(0))
        .sum();
    let i5: i64 = recs
        .iter()
        .take(5)
        .map(|x| clean_int(x.get("organPureBuyQuant")).unwrap_or(0))
        .sum();
    let f20: i64 = recs
        .iter()
        .take(20)
        .map(|x| clean_int(x.get("foreignerPureBuyQuant")).unwrap_or(0))
        .sum();
    let i20: i64 = recs
        .iter()
        .take(20)
        .map(|x| clean_int(x.get("organPureBuyQuant")).unwrap_or(0))
        .sum();
    let h_now = clean_float(recs[0].get("foreignerHoldRatio"));
    let h_old = if recs.len() >= 2 {
        clean_float(recs[recs.len() - 1].get("foreignerHoldRatio"))
    } else {
        None
    };
    let f_delta = match (h_now, h_old) {
        (Some(a), Some(b)) => Some(((a - b) * 100.0).round() / 100.0),
        _ => None,
    };
    (
        symbol,
        Some(json!({
            "flow_5d_for": f5,
            "flow_5d_inst": i5,
            "flow_20d_for": f20,
            "flow_20d_inst": i20,
            "foreign_delta_pct": f_delta,
        })),
    )
}

pub async fn fetch_kr_flows(symbols: &[String]) -> HashMap<String, Value> {
    let now = now_secs();
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    for sym in symbols {
        if let Some(hit) = FLOW_CACHE.get(sym) {
            if hit.0 > now {
                out.insert(sym.clone(), hit.1.clone());
                continue;
            }
        }
        missing.push(sym.clone());
    }
    if !missing.is_empty() {
        let fetched: Vec<(String, Option<Value>)> = stream::iter(missing)
            .map(fetch_kr_flow_one)
            .buffer_unordered(12)
            .collect()
            .await;
        for (sym, row) in fetched {
            if let Some(r) = row {
                FLOW_CACHE.insert(sym.clone(), (now + FLOW_TTL, r.clone()));
                out.insert(sym, r);
            }
        }
    }
    out
}
