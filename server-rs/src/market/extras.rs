//! Toss-only extras (KOSPI/KOSDAQ overview, session calendar, warnings, investor flow, orderbook)
//! and Toss numeric JSON normalization shared with Account & Trading.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::models::Market;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;

const INDICES: [&str; 2] = ["KOSPI", "KOSDAQ"];

const STRING_KEYS: [&str; 22] = [
    "symbol",
    "name",
    "marketCountry",
    "currency",
    "market",
    "conditionalOrderId",
    "clientOrderId",
    "orderId",
    "triggeredOrderId",
    "status",
    "type",
    "orderSide",
    "orderType",
    "expireDate",
    "createdAt",
    "date",
    "timestamp",
    "accountNo",
    "accountType",
    "nextCursor",
    "id",
    "warningType",
];

pub fn normalize(v: Value, key: &str) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, x) in map {
                if k == "rate" || k == "rateAfterCost" {
                    let target = if k == "rate" { "ratePct" } else { "rateAfterCostPct" };
                    if let Some(n) = x
                        .as_f64()
                        .or_else(|| x.as_str().and_then(|s| s.parse::<f64>().ok()))
                    {
                        out.insert(target.to_string(), json!(n * 100.0));
                        continue;
                    }
                }
                let nk = k.clone();
                out.insert(k, normalize(x, &nk));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.into_iter().map(|x| normalize(x, key)).collect()),
        Value::String(s) => {
            if !STRING_KEYS.contains(&key) {
                if let Ok(f) = s.parse::<f64>() {
                    if f.is_finite() {
                        return json!(f);
                    }
                }
            }
            Value::String(s)
        }
        other => other,
    }
}

fn session_labels(market: &str) -> &'static [(&'static str, &'static str)] {
    if market == "KR" {
        &[
            ("preMarket", "장전 거래 (NXT)"),
            ("regularMarket", "정규장"),
            ("afterMarket", "장후 거래 (NXT)"),
        ]
    } else {
        &[
            ("dayMarket", "데이마켓"),
            ("preMarket", "프리마켓"),
            ("regularMarket", "정규장"),
            ("afterMarket", "애프터마켓"),
        ]
    }
}

fn warning_meta(wtype: &str) -> (&str, &'static str) {
    match wtype {
        "LIQUIDATION_TRADING" => ("정리매매", "critical"),
        "OVERHEATED" => ("단기과열", "warning"),
        "INVESTMENT_WARNING" => ("투자경고", "serious"),
        "INVESTMENT_RISK" => ("투자위험", "critical"),
        "VI_STATIC_AND_DYNAMIC" => ("VI 발동 (정적·동적)", "serious"),
        "VI_STATIC" => ("VI 발동 (정적)", "serious"),
        "VI_DYNAMIC" => ("VI 발동 (동적)", "serious"),
        "STOCK_WARRANTS" => ("신주인수권", "warning"),
        other => (other, "warning"),
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub struct ExtrasCache {
    map: DashMap<String, (f64, Value)>,
}

impl Default for ExtrasCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtrasCache {
    pub fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<Value> {
        let now = now_secs();
        self.map
            .get(key)
            .and_then(|e| if e.0 > now { Some(e.1.clone()) } else { None })
    }

    pub fn set(&self, key: String, ttl: f64, val: Value) {
        let now = now_secs();
        self.map.retain(|_, v| v.0 > now);
        self.map.insert(key, (now + ttl, val));
    }
}

fn require_toss(state: &AppState, market: Market) -> ApiResult<()> {
    let prov = state
        .config
        .market_providers
        .get(market.as_str())
        .map(|s| s.as_str())
        .unwrap_or("mock");
    if !matches!(prov, "toss" | "kiwoom" | "broker") {
        return Err(ApiError::not_implemented(
            "requires_toss",
            "증권사 실시간 시세 연동에서만 제공됩니다",
        ));
    }
    Ok(())
}

fn check_symbol(market: Market, symbol: &str) -> ApiResult<()> {
    if !market.validate_symbol(symbol) {
        return Err(ApiError::bad_request(
            "symbol_market_mismatch",
            format!("{symbol} is not a {} symbol", market.as_str()),
        ));
    }
    Ok(())
}

fn parse_dt(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub fn sessions_from_calendar(market: &str, cal: &Value) -> Vec<(String, DateTime<Utc>, DateTime<Utc>)> {
    let mut out = Vec::new();
    for day in ["previousBusinessDay", "today", "nextBusinessDay"] {
        let Some(d) = cal.get(day).and_then(|v| v.as_object()) else {
            continue;
        };
        let block = if market == "KR" {
            d.get("integrated").and_then(|v| v.as_object())
        } else {
            Some(d)
        };
        let Some(block) = block else {
            continue;
        };
        for (name, _) in session_labels(market) {
            if let Some(s) = block.get(*name).and_then(|v| v.as_object()) {
                let start = s.get("startTime").and_then(|v| v.as_str()).and_then(parse_dt);
                let end = s.get("endTime").and_then(|v| v.as_str()).and_then(parse_dt);
                if let (Some(st), Some(en)) = (start, end) {
                    out.push(((*name).to_string(), st, en));
                }
            }
        }
    }
    out.sort_by_key(|x| x.1);
    out
}

pub fn session_status(market: &str, cal: &Value, now: DateTime<Utc>) -> Value {
    let sessions = sessions_from_calendar(market, cal);
    let label_for = |name: &str| -> String {
        session_labels(market)
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| (*v).to_string())
            .unwrap_or_else(|| name.to_string())
    };
    for (name, start, end) in &sessions {
        if *start <= now && now < *end {
            return json!({
                "open": true,
                "session": name,
                "label": label_for(name),
                "until": end.to_rfc3339(),
            });
        }
    }
    let upcoming = sessions.iter().find(|(_, s, _)| *s > now);
    let today_open = cal
        .get("today")
        .and_then(|d| d.get(if market == "KR" { "integrated" } else { "regularMarket" }))
        .map(|v| v.is_object())
        .unwrap_or(false);
    json!({
        "open": false,
        "session": Value::Null,
        "label": if today_open { "장 마감" } else { "휴장" },
        "next": upcoming.map(|(n, s, _)| json!({
            "session": n,
            "label": label_for(n),
            "at": s.to_rfc3339(),
        })),
    })
}

pub async fn calendar(state: &AppState, user_id: &str, market: &str) -> ApiResult<Value> {
    let broker = crate::users::credentials::active_broker(state, user_id).as_str();
    let today = Utc::now().date_naive().to_string();
    let key = format!("{user_id}:{broker}:calendar:{market}:{today}");
    if let Some(hit) = state.market_extras.get(&key) {
        return Ok(hit);
    }
    let client = state.get_broker_client(user_id)?;
    let cal = client
        .request("GET", &format!("/api/v1/market-calendar/{market}"), None, None, None)
        .await?
        .unwrap_or_else(|| json!({}));
    state.market_extras.set(key, 600.0, cal.clone());
    Ok(cal)
}

pub async fn market_status(state: &AppState, user_id: &str, market: &str) -> ApiResult<Value> {
    let cal = calendar(state, user_id, market).await?;
    Ok(session_status(market, &cal, Utc::now()))
}

pub async fn regular_session(
    state: &AppState,
    user_id: &str,
    market: &str,
) -> ApiResult<Option<(String, DateTime<Utc>, DateTime<Utc>)>> {
    let cal = calendar(state, user_id, market).await?;
    let Some(today) = cal.get("today") else {
        return Ok(None);
    };
    let block = if market == "KR" {
        today.get("integrated")
    } else {
        Some(today)
    };
    let Some(reg) = block.and_then(|b| b.get("regularMarket")) else {
        return Ok(None);
    };
    let Some(date_str) = today.get("date").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let Some(st) = reg.get("startTime").and_then(|v| v.as_str()).and_then(parse_dt) else {
        return Ok(None);
    };
    let Some(en) = reg.get("endTime").and_then(|v| v.as_str()).and_then(parse_dt) else {
        return Ok(None);
    };
    Ok(Some((date_str.to_string(), st, en)))
}

pub async fn overview(state: &AppState, user_id: &str) -> ApiResult<Value> {
    require_toss(state, Market::KR)?;
    let broker = crate::users::credentials::active_broker(state, user_id).as_str();
    let idx_key = format!("{user_id}:{broker}:indices");
    let indices = if let Some(hit) = state.market_extras.get(&idx_key) {
        hit
    } else {
        let client = state.get_broker_client(user_id)?;
        let mut params = HashMap::new();
        params.insert("symbols".to_string(), INDICES.join(","));
        let rows = client
            .request("GET", "/api/v1/market-indicators/prices", Some(&params), None, None)
            .await?
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut by_sym: HashMap<String, Value> = HashMap::new();
        for r in rows {
            if let Some(s) = r.get("symbol").and_then(|v| v.as_str()) {
                by_sym.insert(s.to_string(), r);
            }
        }
        let mut out = Vec::new();
        for sym in INDICES {
            let Some(row) = by_sym.get(sym) else {
                continue;
            };
            let price = row
                .get("lastPrice")
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0.0);
            let mut cp = HashMap::new();
            cp.insert("interval".to_string(), "1d".to_string());
            cp.insert("count".to_string(), "2".to_string());
            let daily = client
                .request(
                    "GET",
                    &format!("/api/v1/market-indicators/{sym}/candles"),
                    Some(&cp),
                    None,
                    None,
                )
                .await?
                .and_then(|v| v.get("candles").and_then(|c| c.as_array()).cloned())
                .unwrap_or_default();
            let prev = daily
                .get(1)
                .and_then(|c| c.get("closePrice"))
                .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(price);
            out.push(json!({
                "symbol": sym,
                "price": price,
                "change": price - prev,
                "change_pct": if prev != 0.0 { (price / prev - 1.0) * 100.0 } else { 0.0 },
            }));
        }
        let val = Value::Array(out);
        state.market_extras.set(idx_key, 15.0, val.clone());
        val
    };

    let kr = market_status(state, user_id, "KR").await?;
    let us = market_status(state, user_id, "US").await?;
    Ok(json!({
        "indices": indices,
        "status": { "KR": kr, "US": us },
    }))
}

pub async fn warnings(state: &AppState, user_id: &str, market: Market, symbol: &str) -> ApiResult<Value> {
    require_toss(state, market)?;
    check_symbol(market, symbol)?;
    let broker = crate::users::credentials::active_broker(state, user_id).as_str();
    let key = format!("{user_id}:{broker}:warnings:{symbol}");
    if let Some(hit) = state.market_extras.get(&key) {
        return Ok(hit);
    }
    let client = state.get_broker_client(user_id)?;
    let rows = client
        .request("GET", &format!("/api/v1/stocks/{symbol}/warnings"), None, None, None)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let out: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            let wt = r.get("warningType").and_then(|v| v.as_str()).unwrap_or("");
            let (label, level) = warning_meta(wt);
            json!({
                "type": wt,
                "label": label,
                "level": level,
                "start": r.get("startDate").cloned().unwrap_or(Value::Null),
                "end": r.get("endDate").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let val = Value::Array(out);
    state.market_extras.set(key, 60.0, val.clone());
    Ok(val)
}

pub async fn investor_flow(state: &AppState, user_id: &str, symbol: &str, count: usize) -> ApiResult<Value> {
    require_toss(state, Market::KR)?;
    check_symbol(Market::KR, symbol)?;
    let broker = crate::users::credentials::active_broker(state, user_id).as_str();
    let key = format!("{user_id}:{broker}:flow:{symbol}:{count}");
    if let Some(hit) = state.market_extras.get(&key) {
        return Ok(json!({ "records": hit }));
    }
    let client = state.get_broker_client(user_id)?;
    let mut params = HashMap::new();
    params.insert("count".to_string(), count.to_string());
    let r = client
        .request(
            "GET",
            &format!("/api/v1/stocks/{symbol}/investor-trading"),
            Some(&params),
            None,
            None,
        )
        .await?
        .unwrap_or_else(|| json!({}));
    let recs = normalize(r.get("records").cloned().unwrap_or_else(|| json!([])), "records");
    let out: Vec<Value> = recs
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|rec| {
            let fh = rec
                .get("foreignerHolding")
                .and_then(|x| x.get("holdingRate"))
                .and_then(|v| v.as_f64())
                .map(|rate| json!(rate * 100.0))
                .unwrap_or(Value::Null);
            json!({
                "date": rec.get("date").cloned().unwrap_or(Value::Null),
                "individual": rec.get("individual").and_then(|x| x.get("netBuyVolume")).cloned().unwrap_or(Value::Null),
                "foreigner": rec.get("foreigner").and_then(|x| x.get("netBuyVolume")).cloned().unwrap_or(Value::Null),
                "institution": rec.get("institution").and_then(|x| x.get("netBuyVolume")).cloned().unwrap_or(Value::Null),
                "foreign_holding_pct": fh,
            })
        })
        .collect();
    let val = Value::Array(out);
    state.market_extras.set(key, 300.0, val.clone());
    Ok(json!({ "records": val }))
}

pub async fn orderbook(state: &AppState, user_id: &str, market: Market, symbol: &str) -> ApiResult<Value> {
    require_toss(state, market)?;
    check_symbol(market, symbol)?;
    let client = state.get_broker_client(user_id)?;
    let mut p1 = HashMap::new();
    p1.insert("symbol".to_string(), symbol.to_string());
    let book = normalize(
        client
            .request("GET", "/api/v1/orderbook", Some(&p1), None, None)
            .await?
            .unwrap_or_else(|| json!({})),
        "",
    );
    let mut p2 = HashMap::new();
    p2.insert("symbol".to_string(), symbol.to_string());
    p2.insert("count".to_string(), "30".to_string());
    let trades = normalize(
        client
            .request("GET", "/api/v1/trades", Some(&p2), None, None)
            .await?
            .unwrap_or_else(|| json!([])),
        "",
    );
    Ok(json!({ "orderbook": book, "trades": trades }))
}
