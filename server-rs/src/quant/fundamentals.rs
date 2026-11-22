//! Public fundamental metrics (PER, PBR, ROE, dividend yield, market cap) with 6h caching.

use dashmap::DashMap;
use futures_util::stream::{self, StreamExt};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

const FUNDAMENTALS_TTL: f64 = 6.0 * 3600.0;
const CRUMB_TTL: f64 = 1800.0;
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

static NUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"-?\d+(?:\.\d+)?").unwrap());
static JO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*조").unwrap());
static EOK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+)?)\s*억").unwrap());

static CACHE: Lazy<DashMap<(String, String), (f64, Value)>> = Lazy::new(DashMap::new);
static YAHOO_STATE: Lazy<AsyncMutex<(Option<Client>, Option<String>, f64)>> =
    Lazy::new(|| AsyncMutex::new((None, None, 0.0)));
static NAVER_HTTP: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("naver http client")
});

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn parse_float(raw: Option<&Value>) -> Option<f64> {
    let v = raw?;
    if let Some(f) = v.as_f64() {
        return Some(f);
    }
    let s = v.as_str()?.replace(',', "");
    let t = s.trim();
    if t.is_empty() || matches!(t, "-" | "N/A" | "적자" | "없음") {
        return None;
    }
    NUM_RE.find(t)?.as_str().parse().ok()
}

fn parse_kr_market_cap(raw: Option<&Value>) -> Option<f64> {
    let s = raw?.as_str()?.replace(',', "");
    let mut total = 0.0;
    let mut found = false;
    if let Some(c) = JO_RE.captures(&s) {
        if let Ok(v) = c[1].parse::<f64>() {
            total += v * 1e12;
            found = true;
        }
    }
    if let Some(c) = EOK_RE.captures(&s) {
        if let Ok(v) = c[1].parse::<f64>() {
            total += v * 1e8;
            found = true;
        }
    }
    if found { Some(total) } else { None }
}

fn empty(source: &str) -> Value {
    json!({
        "per": Value::Null,
        "pbr": Value::Null,
        "roe": Value::Null,
        "dividend_yield": Value::Null,
        "market_cap": Value::Null,
        "source": source,
    })
}

fn mock_fundamental(market: &str, symbol: &str) -> Value {
    let digest = Sha256::digest(format!("{market}:{symbol}:fund").as_bytes());
    let mut h = 0u64;
    for &b in &digest[..6] {
        h = (h << 8) | (b as u64);
    }
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let per = r2(6.0 + (h % 320) as f64 / 10.0);
    let pbr = r2(0.5 + ((h >> 10) % 75) as f64 / 10.0);
    let roe = r2(4.0 + ((h >> 18) % 360) as f64 / 10.0);
    let div = r2(((h >> 26) % 62) as f64 / 10.0);
    let mult = if market == "KR" { 1e12 } else { 1e9 };
    let mcap = (5 + ((h >> 34) % 450)) as f64 * mult;
    json!({
        "per": per,
        "pbr": pbr,
        "roe": roe,
        "dividend_yield": div,
        "market_cap": mcap,
        "source": "mock",
    })
}

async fn yahoo_session(force: bool) -> Result<(Client, String), String> {
    let mut g = YAHOO_STATE.lock().await;
    let now = now_secs();
    if !force {
        if let (Some(ref c), Some(ref cr)) = (&g.0, &g.1) {
            if g.2 > now {
                return Ok((c.clone(), cr.clone()));
            }
        }
    }
    let client = Client::builder()
        .user_agent(USER_AGENT)
        .cookie_store(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let _ = client.get("https://fc.yahoo.com").send().await;
    let resp = client
        .get("https://query1.finance.yahoo.com/v1/test/getcrumb")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err("crumb failed".to_string());
    }
    let crumb = resp.text().await.map_err(|e| e.to_string())?.trim().to_string();
    if crumb.is_empty() {
        return Err("empty crumb".to_string());
    }
    *g = (Some(client.clone()), Some(crumb.clone()), now + CRUMB_TTL);
    Ok((client, crumb))
}

async fn fetch_us_yahoo(symbols: &[String]) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    let Ok((mut client, mut crumb)) = yahoo_session(false).await else {
        for s in symbols {
            out.insert(s.clone(), empty("unavailable"));
        }
        return out;
    };
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    for chunk in symbols.chunks(50) {
        let mut y_to_orig = HashMap::new();
        let mut ysyms = Vec::new();
        for s in chunk {
            let y = s.replace('.', "-");
            y_to_orig.insert(y.clone(), s.clone());
            ysyms.push(y);
        }
        let joined = ysyms.join(",");
        let mut resp = match client
            .get("https://query1.finance.yahoo.com/v7/finance/quote")
            .query(&[("symbols", joined.as_str()), ("crumb", crumb.as_str())])
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if matches!(resp.status().as_u16(), 401 | 403) {
            if let Ok((c2, cr2)) = yahoo_session(true).await {
                client = c2;
                crumb = cr2;
                if let Ok(r2_resp) = client
                    .get("https://query1.finance.yahoo.com/v7/finance/quote")
                    .query(&[("symbols", joined.as_str()), ("crumb", crumb.as_str())])
                    .send()
                    .await
                {
                    resp = r2_resp;
                }
            }
        }
        if !resp.status().is_success() {
            continue;
        }
        let Ok(val) = resp.json::<Value>().await else {
            continue;
        };
        let rows = val
            .get("quoteResponse")
            .and_then(|q| q.get("result"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        for q in rows {
            let raw_sym = q.get("symbol").and_then(|v| v.as_str()).unwrap_or("").to_uppercase();
            let sym = y_to_orig.get(&raw_sym).cloned().unwrap_or(raw_sym);
            let per = parse_float(q.get("trailingPE")).filter(|&v| v > 0.0).map(r2);
            let pbr = parse_float(q.get("priceToBook")).filter(|&v| v > 0.0).map(r2);
            let eps = parse_float(q.get("epsTrailingTwelveMonths"));
            let bv = parse_float(q.get("bookValue"));
            let roe = match (eps, bv) {
                (Some(e), Some(b)) if b > 0.0 => Some(r2(e / b * 100.0)),
                _ => None,
            };
            let div = if let Some(rd) = parse_float(q.get("trailingAnnualDividendYield")) {
                Some(r2(rd * 100.0))
            } else {
                parse_float(q.get("dividendYield")).map(|a| r2(if a > 0.5 { a } else { a * 100.0 }))
            }
            .filter(|&v| v >= 0.0);
            let mcap = parse_float(q.get("marketCap"));
            out.insert(
                sym,
                json!({
                    "per": per,
                    "pbr": pbr,
                    "roe": roe,
                    "dividend_yield": div,
                    "market_cap": mcap,
                    "source": "yahoo",
                }),
            );
        }
    }
    for s in symbols {
        out.entry(s.clone()).or_insert_with(|| empty("unavailable"));
    }
    out
}

async fn fetch_kr_one(symbol: String) -> (String, Value) {
    let url = format!("https://m.stock.naver.com/api/stock/{symbol}/integration");
    let Ok(resp) = NAVER_HTTP.get(&url).send().await else {
        return (symbol, empty("unavailable"));
    };
    if !resp.status().is_success() {
        return (symbol, empty("unavailable"));
    }
    let Ok(data) = resp.json::<Value>().await else {
        return (symbol, empty("unavailable"));
    };
    let mut infos: HashMap<String, Value> = HashMap::new();
    if let Some(arr) = data.get("totalInfos").and_then(|v| v.as_array()) {
        for it in arr {
            if let Some(code) = it.get("code").and_then(|v| v.as_str()) {
                infos.insert(code.to_string(), it.get("value").cloned().unwrap_or(Value::Null));
            }
        }
    }
    let r2 = |x: f64| (x * 100.0).round() / 100.0;
    let per = parse_float(infos.get("per")).filter(|&v| v > 0.0).map(r2);
    let pbr = parse_float(infos.get("pbr")).filter(|&v| v > 0.0).map(r2);
    let eps = parse_float(infos.get("eps"));
    let bps = parse_float(infos.get("bps"));
    let div = parse_float(infos.get("dividendYieldRatio")).filter(|&v| v >= 0.0).map(r2);
    let mcap = parse_kr_market_cap(infos.get("marketValue"));
    let roe = match (eps, bps) {
        (Some(e), Some(b)) if b > 0.0 => Some(r2(e / b * 100.0)),
        _ => None,
    };
    (
        symbol,
        json!({
            "per": per,
            "pbr": pbr,
            "roe": roe,
            "dividend_yield": div,
            "market_cap": mcap,
            "source": "naver",
        }),
    )
}

async fn fetch_kr_naver(symbols: &[String]) -> HashMap<String, Value> {
    let rows: Vec<(String, Value)> = stream::iter(symbols.iter().cloned())
        .map(fetch_kr_one)
        .buffer_unordered(12)
        .collect()
        .await;
    rows.into_iter().collect()
}

pub async fn get_fundamentals(
    market: &str,
    symbols: &[String],
    use_mock: bool,
) -> HashMap<String, Value> {
    let clean: Vec<String> = symbols
        .iter()
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty())
        .collect();
    if use_mock {
        return clean
            .into_iter()
            .map(|s| {
                let v = mock_fundamental(market, &s);
                (s, v)
            })
            .collect();
    }
    let now = now_secs();
    let mut out = HashMap::new();
    let mut missing = Vec::new();
    for s in clean {
        let key = (market.to_string(), s.clone());
        if let Some(hit) = CACHE.get(&key) {
            if hit.0 > now {
                out.insert(s, hit.1.clone());
                continue;
            }
        }
        missing.push(s);
    }
    if !missing.is_empty() {
        let fetched = if market == "KR" {
            fetch_kr_naver(&missing).await
        } else {
            fetch_us_yahoo(&missing).await
        };
        for (s, row) in fetched {
            let ttl = if row.get("source").and_then(|v| v.as_str()) != Some("unavailable") {
                FUNDAMENTALS_TTL
            } else {
                300.0
            };
            CACHE.insert((market.to_string(), s.clone()), (now + ttl, row.clone()));
            out.insert(s, row);
        }
    }
    out
}
