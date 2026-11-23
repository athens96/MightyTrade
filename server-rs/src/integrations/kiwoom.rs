//! Kiwoom Securities Official REST API client (`openapi.kiwoom.com`).
//!
//! Supports both live (`https://api.kiwoom.com`) and official mock (`https://mockapi.kiwoom.com`)
//! environments, OAuth2 (`au10001`) shared encrypted token caching, per-client rate limiting,
//! unified canonical broker request translation (KR + US stocks), and WebSocket condition search.

use async_trait::async_trait;
use chrono::{Datelike, Duration as ChronoDuration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;
use tokio_tungstenite::tungstenite::Message;

use crate::crypto::Crypto;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::Store;

pub const KIWOOM_LIVE_URL: &str = "https://api.kiwoom.com";
pub const KIWOOM_MOCK_URL: &str = "https://mockapi.kiwoom.com";
pub const KIWOOM_LIVE_WS_URL: &str = "wss://api.kiwoom.com:10000/api/dostk/websocket";
pub const KIWOOM_MOCK_WS_URL: &str = "wss://mockapi.kiwoom.com:10000/api/dostk/websocket";
pub const KIWOOM_LIVE_US_WS_URL: &str = "wss://api.kiwoom.com:10000/api/us/websocket";
pub const KIWOOM_MOCK_US_WS_URL: &str = "wss://mockapi.kiwoom.com:10000/api/us/websocket";
pub const KIWOOM_TOKENS: &str = "integration_tokens";
const TOKEN_MARGIN_SECS: f64 = 300.0;

#[async_trait]
pub trait KiwoomHttpOverride: Send + Sync {
    async fn tr_post(
        &self,
        is_mock: bool,
        path: &str,
        api_id: &str,
        body: &Value,
        cont_yn: &str,
        next_key: &str,
    ) -> ApiResult<Value>;

    async fn verify(&self, _is_mock: bool, _appkey: &str, _secretkey: &str) -> ApiResult<()> {
        Ok(())
    }

    async fn ws_conditions(&self, _is_mock: bool) -> ApiResult<Vec<(String, String)>> {
        Ok(vec![])
    }

    async fn ws_condition_matches(&self, _is_mock: bool, _seq: &str) -> ApiResult<Vec<Value>> {
        Ok(vec![])
    }
}

pub fn kiwoom_token_id(appkey: &str, is_mock: bool) -> String {
    let prefix = if is_mock { "mock" } else { "live" };
    let digest = Sha256::digest(format!("{prefix}:{appkey}").as_bytes());
    let mut hex = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    format!("kiwoom_{prefix}_{hex}")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub fn parse_kiwoom_signed(v: Option<&Value>) -> Option<f64> {
    let val = v?;
    if let Some(f) = val.as_f64() {
        return f.is_finite().then_some(f);
    }
    let s = val.as_str()?.trim().replace(',', "");
    if s.is_empty() {
        return None;
    }
    let clean = s.trim_start_matches('+');
    clean.parse::<f64>().ok().filter(|f| f.is_finite())
}

pub fn parse_kiwoom_abs(v: Option<&Value>) -> Option<f64> {
    parse_kiwoom_signed(v).map(|f| f.abs())
}

pub fn strip_kr_prefix(raw: &str) -> String {
    let s = raw.trim().to_uppercase();
    if s.len() == 7 && (s.starts_with('A') || s.starts_with('J') || s.starts_with('Q')) {
        let rest = &s[1..];
        if rest.chars().all(|c| c.is_ascii_digit()) {
            return rest.to_string();
        }
    }
    s
}

pub fn is_kr_symbol(sym: &str) -> bool {
    sym.len() == 6 && sym.chars().all(|c| c.is_ascii_digit())
}

pub fn guess_us_exch(symbol: &str) -> &'static str {
    const AMEX_ETFS: &[&str] = &[
        "SPY", "VOO", "VTI", "SCHD", "JEPI", "JEPQ", "GLD", "SLV", "DBC", "XLE", "XLF", "XLK",
        "XLV", "XLI", "XLP", "XLY", "XLU", "XLB", "XLRE", "XLC", "BIL", "SGOV", "USO", "VNQ",
        "EFA", "EEM", "VEA", "VWO", "AGG", "LQD", "HYG", "TIP", "SOXL", "SOXS", "DIA", "IWM",
        "ARKK",
    ];
    const NYSE_SYMS: &[&str] = &[
        "BRK.B", "JPM", "V", "MA", "KO", "PG", "XOM", "JNJ", "WMT", "BAC", "DIS", "TSM", "UNH",
        "LLY", "ORCL", "IBM", "GE", "CAT", "BA", "NKE", "HD", "CVX", "ABBV", "MRK", "PFE", "MCD",
        "CRM", "ACN", "TMO", "ABT", "PLTR", "UBER", "SPOT", "SNOW",
    ];
    if AMEX_ETFS.contains(&symbol) {
        "NA"
    } else if NYSE_SYMS.contains(&symbol) {
        "NY"
    } else {
        "ND"
    }
}

fn parse_expires_dt(raw: &str) -> Option<f64> {
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 14 {
        return None;
    }
    let dt = NaiveDateTime::parse_from_str(&digits[..14], "%Y%m%d%H%M%S").ok()?;
    let kst = FixedOffset::east_opt(9 * 3600)?;
    let aware = kst.from_local_datetime(&dt).single()?;
    Some(aware.timestamp() as f64)
}

fn first_str<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a str> {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn first_arr<'a>(obj: &'a Value, keys: &[&str]) -> &'a [Value] {
    for k in keys {
        if let Some(arr) = obj.get(*k).and_then(|v| v.as_array()) {
            return arr.as_slice();
        }
    }
    &[]
}

fn parse_yyyymmdd_utc(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() < 8 {
        return None;
    }
    let nd = NaiveDate::parse_from_str(&d[..8], "%Y%m%d").ok()?;
    Some(format!("{nd}T00:00:00Z"))
}

fn parse_kst_minute_rfc3339(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() < 12 {
        return None;
    }
    let padded = if d.len() >= 14 {
        d[..14].to_string()
    } else {
        format!("{}00", &d[..12])
    };
    let ndt = NaiveDateTime::parse_from_str(&padded, "%Y%m%d%H%M%S").ok()?;
    let kst = FixedOffset::east_opt(9 * 3600)?;
    let aware = kst.from_local_datetime(&ndt).single()?;
    Some(aware.with_timezone(&Utc).to_rfc3339())
}

fn parse_us_et_minute_rfc3339(raw: &str) -> Option<String> {
    let d: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if d.len() < 12 {
        return None;
    }
    let padded = if d.len() >= 14 {
        d[..14].to_string()
    } else {
        format!("{}00", &d[..12])
    };
    let ndt = NaiveDateTime::parse_from_str(&padded, "%Y%m%d%H%M%S").ok()?;
    let off_hrs = if is_us_dst(ndt.date()) { -4 } else { -5 };
    let et = FixedOffset::east_opt(off_hrs * 3600)?;
    let aware = et.from_local_datetime(&ndt).single()?;
    Some(aware.with_timezone(&Utc).to_rfc3339())
}

fn is_us_dst(date: NaiveDate) -> bool {
    let year = date.year();
    // 2nd Sunday of March
    let mar1 = NaiveDate::from_ymd_opt(year, 3, 1).unwrap();
    let mar_sun_offset = (7 - mar1.weekday().num_days_from_sunday()) % 7;
    let dst_start = mar1 + ChronoDuration::days((mar_sun_offset + 7) as i64);
    // 1st Sunday of November
    let nov1 = NaiveDate::from_ymd_opt(year, 11, 1).unwrap();
    let nov_sun_offset = (7 - nov1.weekday().num_days_from_sunday()) % 7;
    let dst_end = nov1 + ChronoDuration::days(nov_sun_offset as i64);
    date >= dst_start && date < dst_end
}

fn prev_weekday(mut d: NaiveDate) -> NaiveDate {
    loop {
        d -= ChronoDuration::days(1);
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            return d;
        }
    }
}

fn next_weekday(mut d: NaiveDate) -> NaiveDate {
    loop {
        d += ChronoDuration::days(1);
        if !matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
            return d;
        }
    }
}

fn build_day_sessions(market: &str, d: NaiveDate) -> Value {
    if matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
        return json!({ "date": d.to_string() });
    }
    if market == "KR" {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let mk = |h: u32, m: u32| {
            kst.from_local_datetime(&d.and_hms_opt(h, m, 0).unwrap())
                .single()
                .unwrap()
                .with_timezone(&Utc)
                .to_rfc3339()
        };
        json!({
            "date": d.to_string(),
            "integrated": {
                "preMarket": { "startTime": mk(8, 0), "endTime": mk(8, 50) },
                "regularMarket": { "startTime": mk(9, 0), "endTime": mk(15, 30) },
                "afterMarket": { "startTime": mk(15, 40), "endTime": mk(20, 0) },
            }
        })
    } else {
        let off_hrs = if is_us_dst(d) { -4 } else { -5 };
        let et = FixedOffset::east_opt(off_hrs * 3600).unwrap();
        let mk = |h: u32, m: u32| {
            et.from_local_datetime(&d.and_hms_opt(h, m, 0).unwrap())
                .single()
                .unwrap()
                .with_timezone(&Utc)
                .to_rfc3339()
        };
        json!({
            "date": d.to_string(),
            "preMarket": { "startTime": mk(4, 0), "endTime": mk(9, 30) },
            "regularMarket": { "startTime": mk(9, 30), "endTime": mk(16, 0) },
            "afterMarket": { "startTime": mk(16, 0), "endTime": mk(20, 0) },
        })
    }
}

pub fn synthesize_market_calendar(market: &str) -> Value {
    let now = Utc::now();
    let today = if market == "KR" {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        now.with_timezone(&kst).date_naive()
    } else {
        let off = if is_us_dst(now.date_naive()) { -4 } else { -5 };
        let et = FixedOffset::east_opt(off * 3600).unwrap();
        now.with_timezone(&et).date_naive()
    };
    json!({
        "previousBusinessDay": build_day_sessions(market, prev_weekday(today)),
        "today": build_day_sessions(market, today),
        "nextBusinessDay": build_day_sessions(market, next_weekday(today)),
    })
}

pub struct KiwoomClient {
    pub is_mock: bool,
    base_url: String,
    ws_url: String,
    us_ws_url: String,
    appkey: String,
    secretkey: String,
    pub account_no: String,
    store: Arc<dyn Store>,
    crypto: Arc<Crypto>,
    http: Client,
    tok_id: String,
    mem: AsyncMutex<(Option<String>, f64)>,
    pace: AsyncMutex<Instant>,
    us_exch_cache: DashMap<String, String>,
    override_backend: Option<Arc<dyn KiwoomHttpOverride>>,
}

impl KiwoomClient {
    pub fn new(
        appkey: String,
        secretkey: String,
        account_no: String,
        is_mock: bool,
        store: Arc<dyn Store>,
        crypto: Arc<Crypto>,
        http: Client,
        override_backend: Option<Arc<dyn KiwoomHttpOverride>>,
    ) -> ApiResult<Self> {
        if appkey.trim().is_empty() || secretkey.trim().is_empty() {
            return Err(ApiError::internal(
                "kiwoom_not_configured",
                "키움 App Key / Secret Key가 비어 있습니다",
            ));
        }
        let base_url = if is_mock {
            KIWOOM_MOCK_URL.to_string()
        } else {
            KIWOOM_LIVE_URL.to_string()
        };
        let ws_url = if is_mock {
            KIWOOM_MOCK_WS_URL.to_string()
        } else {
            KIWOOM_LIVE_WS_URL.to_string()
        };
        let us_ws_url = if is_mock {
            KIWOOM_MOCK_US_WS_URL.to_string()
        } else {
            KIWOOM_LIVE_US_WS_URL.to_string()
        };
        let tok_id = kiwoom_token_id(&appkey, is_mock);
        Ok(Self {
            is_mock,
            base_url,
            ws_url,
            us_ws_url,
            appkey,
            secretkey,
            account_no,
            store,
            crypto,
            http,
            tok_id,
            mem: AsyncMutex::new((None, 0.0)),
            pace: AsyncMutex::new(Instant::now() - Duration::from_secs(2)),
            us_exch_cache: DashMap::new(),
            override_backend,
        })
    }

    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }

    pub fn us_ws_url(&self) -> &str {
        &self.us_ws_url
    }

    pub async fn resolve_us_exch(&self, sym: &str) -> String {
        let key = sym.trim().to_uppercase();
        if let Some(hit) = self.us_exch_cache.get(&key) {
            return hit.value().clone();
        }
        if let Ok((v, _, _)) = self
            .tr_post("/api/us/stkinfo", "usa10098", &json!({ "stk_cd": key }), "N", "")
            .await
        {
            let rows = first_arr(&v, &["list", "result_list", "output"]);
            if let Some(exch) = rows
                .first()
                .and_then(|r| first_str(r, &["stex_tp"]))
                .filter(|s| matches!(*s, "ND" | "NY" | "NA"))
            {
                self.us_exch_cache.insert(key, exch.to_string());
                return exch.to_string();
            }
        }
        guess_us_exch(&key).to_string()
    }

    async fn pace_request(&self) {
        if self.override_backend.is_some() {
            return;
        }
        let min_gap = if self.is_mock {
            Duration::from_millis(550)
        } else {
            Duration::from_millis(210)
        };
        let mut last = self.pace.lock().await;
        let elapsed = last.elapsed();
        if elapsed < min_gap {
            tokio::time::sleep(min_gap - elapsed).await;
        }
        *last = Instant::now();
    }

    async fn issue_token(&self) -> ApiResult<(String, f64)> {
        if let Some(ref ov) = self.override_backend {
            ov.verify(self.is_mock, &self.appkey, &self.secretkey).await?;
            return Ok(("fake-kiwoom-token".to_string(), now_secs() + 3600.0));
        }
        let body = json!({
            "grant_type": "client_credentials",
            "appkey": self.appkey,
            "secretkey": self.secretkey,
        });
        let resp = self
            .http
            .post(format!("{}/oauth2/token", self.base_url))
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("api-id", "au10001")
            .json(&body)
            .send()
            .await
            .map_err(|e| ApiError::bad_gateway("kiwoom_unreachable", e.to_string()))?;
        let status = resp.status().as_u16();
        let parsed: Value = resp.json().await.unwrap_or_else(|_| json!({}));
        let ret_code = parsed
            .get("return_code")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0);
        if status >= 400 || ret_code != 0 {
            let msg = parsed
                .get("return_msg")
                .or_else(|| parsed.get("error_description"))
                .and_then(|v| v.as_str())
                .unwrap_or("키움 인증 토큰 발급에 실패했습니다");
            return Err(ApiError::bad_gateway(
                format!("kiwoom_auth_{ret_code}"),
                msg.to_string(),
            ));
        }
        let tok = parsed
            .get("token")
            .or_else(|| parsed.get("access_token"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::bad_gateway("kiwoom_auth_missing_token", "키움 응답에 token이 없습니다")
            })?
            .to_string();
        let exp = parsed
            .get("expires_dt")
            .and_then(|v| v.as_str())
            .and_then(parse_expires_dt)
            .unwrap_or_else(|| now_secs() + 12.0 * 3600.0);
        Ok((tok, exp))
    }

    fn load_shared(&self) -> (Option<String>, f64) {
        let Some(rec) = self.store.get(KIWOOM_TOKENS, &self.tok_id) else {
            return (None, 0.0);
        };
        let enc = rec.get("token_enc").and_then(|v| v.as_str());
        let exp = rec.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        match enc.and_then(|s| self.crypto.decrypt(s).ok()) {
            Some(tok) => (Some(tok), exp),
            None => (None, 0.0),
        }
    }

    async fn get_token(&self, force_refresh: bool, rejected: Option<&str>) -> ApiResult<String> {
        let mut guard = self.mem.lock().await;
        if !force_refresh {
            if let Some(ref tok) = guard.0 {
                if now_secs() < guard.1 - TOKEN_MARGIN_SECS {
                    return Ok(tok.clone());
                }
            }
        }
        let _flock = self.store.lock(&self.tok_id);
        let (shared_tok, shared_exp) = self.load_shared();
        let usable = shared_tok.is_some() && now_secs() < shared_exp - TOKEN_MARGIN_SECS;
        let same_rejected = force_refresh && shared_tok.as_deref() == rejected;
        let (tok, exp) = if !usable || same_rejected {
            let (new_tok, new_exp) = self.issue_token().await?;
            self.store.put(
                KIWOOM_TOKENS,
                &self.tok_id,
                json!({
                    "token_enc": self.crypto.encrypt(&new_tok),
                    "expires_at": new_exp,
                }),
            );
            (new_tok, new_exp)
        } else {
            (shared_tok.unwrap(), shared_exp)
        };
        *guard = (Some(tok.clone()), exp);
        Ok(tok)
    }

    pub async fn access_token(&self, rejected: Option<&str>) -> ApiResult<String> {
        if self.override_backend.is_some() {
            return Ok("fake-kiwoom-token".to_string());
        }
        self.get_token(rejected.is_some(), rejected).await
    }

    pub async fn tr_post(
        &self,
        path: &str,
        api_id: &str,
        body: &Value,
        cont_yn: &str,
        next_key: &str,
    ) -> ApiResult<(Value, String, String)> {
        if let Some(ref ov) = self.override_backend {
            let val = ov
                .tr_post(self.is_mock, path, api_id, body, cont_yn, next_key)
                .await?;
            let c_yn = val
                .get("cont_yn")
                .and_then(|v| v.as_str())
                .unwrap_or("N")
                .to_string();
            let n_key = val
                .get("next_key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Ok((val, c_yn, n_key));
        }

        let mut token = self.get_token(false, None).await?;
        for attempt in 0..3 {
            self.pace_request().await;
            let resp = self
                .http
                .post(format!("{}{}", self.base_url, path))
                .header("Content-Type", "application/json;charset=UTF-8")
                .header("authorization", format!("Bearer {token}"))
                .header("api-id", api_id)
                .header("cont-yn", cont_yn)
                .header("next-key", next_key)
                .json(body)
                .send()
                .await
                .map_err(|e| ApiError::bad_gateway("kiwoom_unreachable", e.to_string()))?;
            let status = resp.status().as_u16();
            let resp_cont = resp
                .headers()
                .get("cont-yn")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("N")
                .to_string();
            let resp_next = resp
                .headers()
                .get("next-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ApiError::bad_gateway("kiwoom_unreachable", e.to_string()))?;
            let parsed: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));

            if status == 401 && attempt == 0 {
                token = self.get_token(true, Some(&token)).await?;
                continue;
            }
            if status == 429 && attempt < 2 {
                tokio::time::sleep(Duration::from_millis(800)).await;
                continue;
            }
            if status >= 400 {
                let msg = parsed
                    .get("return_msg")
                    .or_else(|| parsed.get("msg1"))
                    .or_else(|| parsed.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("키움 API 요청 실패");
                return Err(map_kiwoom_error(status, api_id, msg));
            }
            let ret_code = parsed
                .get("return_code")
                .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                .unwrap_or(0);
            if ret_code != 0 {
                let msg = parsed
                    .get("return_msg")
                    .and_then(|v| v.as_str())
                    .unwrap_or("키움 API 오류");
                if (ret_code == 8005 || msg.contains("토큰") || msg.to_lowercase().contains("token"))
                    && attempt == 0
                {
                    token = self.get_token(true, Some(&token)).await?;
                    continue;
                }
                return Err(ApiError::bad_gateway(
                    format!("kiwoom_{api_id}_{ret_code}"),
                    format!("[키움 {api_id}] {msg}"),
                ));
            }
            return Ok((parsed, resp_cont, resp_next));
        }
        Err(ApiError::bad_gateway(
            "kiwoom_retry_exhausted",
            "키움 API 재시도 한도를 초과했습니다",
        ))
    }

    // ---------------- Phase 3: WebSocket Condition Search ----------------

    pub async fn list_conditions(&self) -> ApiResult<Vec<(String, String)>> {
        if let Some(ref ov) = self.override_backend {
            return ov.ws_conditions(self.is_mock).await;
        }
        let token = self.access_token(None).await?;
        let (mut write, mut read) = tokio::time::timeout(
            Duration::from_secs(8),
            tokio_tungstenite::connect_async(&self.ws_url),
        )
        .await
        .map_err(|_| ApiError::bad_gateway("kiwoom_ws_timeout", "키움 웹소켓 연결 시간 초과"))?
        .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?
        .0
        .split();

        write
            .send(Message::Text(json!({ "trnm": "LOGIN", "token": token }).to_string().into()))
            .await
            .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?;

        let mut sent_list = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let rem = deadline.saturating_duration_since(Instant::now());
            let Some(msg) = tokio::time::timeout(rem, read.next()).await.ok().flatten() else {
                break;
            };
            let Ok(Message::Text(txt)) = msg else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else {
                continue;
            };
            match v.get("trnm").and_then(|x| x.as_str()) {
                Some("PING") => {
                    let _ = write.send(Message::Text(txt)).await;
                }
                Some("LOGIN") => {
                    let rc = v.get("return_code").and_then(|x| x.as_i64()).unwrap_or(0);
                    if rc != 0 {
                        let m = v.get("return_msg").and_then(|x| x.as_str()).unwrap_or("로그인 실패");
                        return Err(ApiError::bad_gateway("kiwoom_ws_login_failed", m));
                    }
                    if !sent_list {
                        sent_list = true;
                        write
                            .send(Message::Text(json!({ "trnm": "CNSRLST" }).to_string().into()))
                            .await
                            .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?;
                    }
                }
                Some("CNSRLST") => {
                    let _ = write.close().await;
                    let mut out = Vec::new();
                    if let Some(arr) = v.get("data").and_then(|x| x.as_array()) {
                        for row in arr {
                            if let Some(pair) = row.as_array() {
                                let seq = pair.first().and_then(|x| x.as_str()).unwrap_or("").trim();
                                let name = pair.get(1).and_then(|x| x.as_str()).unwrap_or("").trim();
                                if !seq.is_empty() && !name.is_empty() {
                                    out.push((seq.to_string(), name.to_string()));
                                }
                            }
                        }
                    }
                    return Ok(out);
                }
                _ => {}
            }
        }
        Err(ApiError::bad_gateway(
            "kiwoom_conditions_timeout",
            "영웅문 조건검색식 목록 응답이 지연되었습니다",
        ))
    }

    pub async fn query_condition_matches(&self, seq: &str) -> ApiResult<Vec<Value>> {
        if let Some(ref ov) = self.override_backend {
            return ov.ws_condition_matches(self.is_mock, seq).await;
        }
        let token = self.access_token(None).await?;
        let (mut write, mut read) = tokio::time::timeout(
            Duration::from_secs(8),
            tokio_tungstenite::connect_async(&self.ws_url),
        )
        .await
        .map_err(|_| ApiError::bad_gateway("kiwoom_ws_timeout", "키움 웹소켓 연결 시간 초과"))?
        .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?
        .0
        .split();

        write
            .send(Message::Text(json!({ "trnm": "LOGIN", "token": token }).to_string().into()))
            .await
            .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?;

        let mut sent_req = false;
        let deadline = Instant::now() + Duration::from_secs(12);
        while Instant::now() < deadline {
            let rem = deadline.saturating_duration_since(Instant::now());
            let Some(msg) = tokio::time::timeout(rem, read.next()).await.ok().flatten() else {
                break;
            };
            let Ok(Message::Text(txt)) = msg else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&txt) else {
                continue;
            };
            match v.get("trnm").and_then(|x| x.as_str()) {
                Some("PING") => {
                    let _ = write.send(Message::Text(txt)).await;
                }
                Some("LOGIN") => {
                    let rc = v.get("return_code").and_then(|x| x.as_i64()).unwrap_or(0);
                    if rc != 0 {
                        let m = v.get("return_msg").and_then(|x| x.as_str()).unwrap_or("로그인 실패");
                        return Err(ApiError::bad_gateway("kiwoom_ws_login_failed", m));
                    }
                    if !sent_req {
                        sent_req = true;
                        let req = json!({
                            "trnm": "CNSRREQ",
                            "seq": seq,
                            "search_type": "0",
                            "stex_tp": "K",
                        });
                        write
                            .send(Message::Text(req.to_string().into()))
                            .await
                            .map_err(|e| ApiError::bad_gateway("kiwoom_ws_error", e.to_string()))?;
                    }
                }
                Some("CNSRREQ") => {
                    let _ = write.close().await;
                    let rows = v
                        .get("data")
                        .and_then(|x| x.as_array())
                        .cloned()
                        .unwrap_or_default();
                    return Ok(rows);
                }
                _ => {}
            }
        }
        Err(ApiError::bad_gateway(
            "kiwoom_condition_query_timeout",
            "영웅문 조건검색 결과 응답이 지연되었습니다",
        ))
    }

    // ---------------- Canonical Broker Adapter ----------------

    pub async fn canonical_request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        json_body: Option<&Value>,
    ) -> ApiResult<Option<Value>> {
        let p = |k: &str| params.and_then(|m| m.get(k)).map(|s| s.as_str()).unwrap_or("");

        if method == "GET" && path == "/api/v1/prices" {
            let syms: Vec<&str> = p("symbols")
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if syms.len() == 1 {
                let row = self.fetch_price(syms[0]).await?;
                return Ok(Some(Value::Array(vec![row])));
            }
            let mut rows = Vec::with_capacity(syms.len());
            for sym in syms {
                if let Ok(row) = self.fetch_price(sym).await {
                    rows.push(row);
                }
            }
            return Ok(Some(Value::Array(rows)));
        }

        if method == "GET" && path == "/api/v1/candles" {
            let sym = p("symbol");
            let interval = p("interval");
            let before = params.and_then(|m| m.get("before")).map(|s| s.as_str());
            let count = p("count").parse::<usize>().unwrap_or(200);
            return Ok(Some(self.fetch_candles(sym, interval, count, before).await?));
        }

        if method == "GET" && path == "/api/v1/stocks/all" {
            let mkt = p("market");
            return Ok(Some(Value::Array(self.fetch_stocks_all(mkt).await?)));
        }

        if method == "GET" && path == "/api/v1/stocks" {
            return Ok(Some(json!([])));
        }

        if method == "GET" && path == "/api/v1/price-limits" {
            let sym = p("symbol");
            return Ok(Some(self.fetch_price_limits(sym).await?));
        }

        if method == "GET" && path == "/api/v1/orderbook" {
            let sym = p("symbol");
            return Ok(Some(self.fetch_orderbook(sym).await?));
        }

        if method == "GET" && path == "/api/v1/trades" {
            let sym = p("symbol");
            let count = p("count").parse::<usize>().unwrap_or(30);
            return Ok(Some(Value::Array(self.fetch_trades(sym, count).await?)));
        }

        if method == "GET" && path.starts_with("/api/v1/market-calendar/") {
            let mkt = path.trim_start_matches("/api/v1/market-calendar/");
            return Ok(Some(synthesize_market_calendar(mkt)));
        }

        if method == "GET" && path == "/api/v1/market-indicators/prices" {
            let syms: Vec<&str> = p("symbols").split(',').filter(|s| !s.is_empty()).collect();
            return Ok(Some(Value::Array(self.fetch_indices_prices(&syms).await)));
        }

        if method == "GET" && path.starts_with("/api/v1/market-indicators/") && path.ends_with("/candles") {
            let sym = path
                .trim_start_matches("/api/v1/market-indicators/")
                .trim_end_matches("/candles");
            return Ok(Some(self.fetch_index_candles(sym).await));
        }

        if method == "GET" && path.starts_with("/api/v1/stocks/") && path.ends_with("/warnings") {
            return Ok(Some(json!([])));
        }

        if method == "GET" && path.starts_with("/api/v1/stocks/") && path.ends_with("/investor-trading") {
            let sym = path
                .trim_start_matches("/api/v1/stocks/")
                .trim_end_matches("/investor-trading");
            let count = p("count").parse::<usize>().unwrap_or(20);
            return Ok(Some(self.fetch_investor_trading(sym, count).await?));
        }

        if method == "GET" && path == "/api/v1/commissions" {
            return Ok(Some(json!([])));
        }

        if method == "GET" && path == "/api/v1/holdings" {
            return Ok(Some(self.fetch_holdings().await?));
        }

        if method == "GET" && path == "/api/v1/buying-power" {
            let cur = p("currency");
            return Ok(Some(self.fetch_buying_power(cur).await?));
        }

        if method == "GET" && path == "/api/v1/sellable-quantity" {
            let sym = p("symbol");
            return Ok(Some(self.fetch_sellable_quantity(sym).await?));
        }

        if method == "POST" && path == "/api/v1/orders" {
            let body = json_body.ok_or_else(|| ApiError::code_only(400, "order_body_required"))?;
            return Ok(Some(self.place_order(body).await?));
        }

        if method == "GET" && path == "/api/v1/orders" {
            let status = p("status");
            let sym_filter = params.and_then(|m| m.get("symbol")).map(|s| s.as_str());
            return Ok(Some(self.list_orders(status, sym_filter).await?));
        }

        if method == "GET" && path.starts_with("/api/v1/orders/") {
            let oid = path.trim_start_matches("/api/v1/orders/");
            return Ok(Some(self.get_order(oid).await?));
        }

        if method == "POST" && path.starts_with("/api/v1/orders/") && path.ends_with("/cancel") {
            let oid = path
                .trim_start_matches("/api/v1/orders/")
                .trim_end_matches("/cancel");
            return Ok(Some(self.cancel_order(oid).await?));
        }

        if method == "POST" && path.starts_with("/api/v1/orders/") && path.ends_with("/modify") {
            let oid = path
                .trim_start_matches("/api/v1/orders/")
                .trim_end_matches("/modify");
            let body = json_body.cloned().unwrap_or_else(|| json!({}));
            return Ok(Some(self.modify_order(oid, &body).await?));
        }

        if path.starts_with("/api/v1/conditional-orders") {
            if method == "GET" {
                return Ok(Some(json!({ "conditionalOrders": [], "hasNext": false })));
            }
            if method == "DELETE" {
                return Ok(None);
            }
            return Err(ApiError::not_implemented(
                "kiwoom_oco_unsupported",
                "키움 REST API는 증권사 서버 OCO 조건주문을 직접 지원하지 않습니다. 자동매매 봇의 손절·익절·트레일링 보호막을 이용해 주세요.",
            ));
        }

        Err(ApiError::not_implemented(
            "kiwoom_unsupported_path",
            format!("키움 REST 어댑터 미지원 경로: {method} {path}"),
        ))
    }

    async fn fetch_price(&self, sym: &str) -> ApiResult<Value> {
        if is_kr_symbol(sym) {
            let (v, _, _) = self
                .tr_post("/api/dostk/stkinfo", "ka10001", &json!({ "stk_cd": sym }), "N", "")
                .await?;
            let px = parse_kiwoom_abs(v.get("cur_prc"))
                .or_else(|| parse_kiwoom_abs(v.get("stck_prpr")))
                .ok_or_else(|| ApiError::not_found("stock_not_found", sym))?;
            let prev = parse_kiwoom_abs(v.get("base_pric"))
                .filter(|x| *x > 0.0)
                .or_else(|| {
                    parse_kiwoom_signed(v.get("pred_pre"))
                        .map(|d| (px - d).max(0.0))
                        .filter(|x| *x > 0.0)
                });
            Ok(json!({
                "symbol": sym,
                "lastPrice": px,
                "prevClose": prev,
                "currency": "KRW",
                "timestamp": Utc::now().to_rfc3339(),
            }))
        } else {
            let primary = self.resolve_us_exch(sym).await;
            let mut tried = Vec::with_capacity(3);
            let mut candidates = vec![primary];
            for fallback in ["ND", "NY", "NA"] {
                if !candidates.iter().any(|c| c == fallback) {
                    candidates.push(fallback.to_string());
                }
            }
            let mut last_err = None;
            for exch in candidates {
                if tried.contains(&exch) {
                    continue;
                }
                tried.push(exch.clone());
                let res = self
                    .tr_post(
                        "/api/us/mrkcond",
                        "usa20100",
                        &json!({ "stex_tp": exch, "stk_cd": sym }),
                        "N",
                        "",
                    )
                    .await;
                match res {
                    Ok((v, _, _)) => {
                        let px = parse_kiwoom_abs(v.get("cur_prc"))
                            .filter(|p| *p > 0.0)
                            .or_else(|| parse_kiwoom_abs(v.get("base_close_pric")).filter(|p| *p > 0.0))
                            .or_else(|| parse_kiwoom_abs(v.get("pre_open_pric")).filter(|p| *p > 0.0));
                        if let Some(p) = px {
                            let prev = parse_kiwoom_abs(v.get("base_close_pric"))
                                .filter(|x| *x > 0.0)
                                .or_else(|| {
                                    parse_kiwoom_signed(v.get("pred_pre"))
                                        .map(|d| (p - d).max(0.0))
                                        .filter(|x| *x > 0.0)
                                });
                            self.us_exch_cache.insert(sym.to_uppercase(), exch);
                            return Ok(json!({
                                "symbol": sym,
                                "lastPrice": p,
                                "prevClose": prev,
                                "currency": "USD",
                                "timestamp": Utc::now().to_rfc3339(),
                            }));
                        }
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            Err(last_err.unwrap_or_else(|| ApiError::not_found("stock_not_found", sym)))
        }
    }

    async fn fetch_candles(
        &self,
        sym: &str,
        interval: &str,
        count: usize,
        before: Option<&str>,
    ) -> ApiResult<Value> {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let now_kst = Utc::now().with_timezone(&kst);
        let cont_token = before.and_then(|b| b.strip_prefix("CONT:"));
        let base_dt = before
            .filter(|b| !b.starts_with("CONT:"))
            .and_then(|s| s.get(..10))
            .map(|s| s.replace('-', ""))
            .filter(|s| s.len() == 8 && s.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or_else(|| now_kst.format("%Y%m%d").to_string());

        let (v, cont_yn, next_key) = if is_kr_symbol(sym) {
            if interval == "1m" {
                let (c_yn, n_key) = match cont_token {
                    Some(k) => ("Y", k),
                    None => (if before.is_some() { "Y" } else { "N" }, before.unwrap_or("")),
                };
                self.tr_post(
                    "/api/dostk/chart",
                    "ka10080",
                    &json!({
                        "stk_cd": sym,
                        "tic_scope": "1",
                        "upd_stkpc_tp": "1",
                        "base_dt": now_kst.format("%Y%m%d").to_string(),
                    }),
                    c_yn,
                    n_key,
                )
                .await?
            } else {
                self.tr_post(
                    "/api/dostk/chart",
                    "ka10081",
                    &json!({
                        "stk_cd": sym,
                        "base_dt": base_dt,
                        "upd_stkpc_tp": "1",
                    }),
                    "N",
                    "",
                )
                .await?
            }
        } else {
            let primary = self.resolve_us_exch(sym).await;
            let (c_yn, n_key) = match cont_token {
                Some(k) => ("Y", k),
                None => ("N", ""),
            };
            let mut candidates = vec![primary];
            if cont_token.is_none() {
                for fallback in ["ND", "NY", "NA"] {
                    if !candidates.iter().any(|c| c == fallback) {
                        candidates.push(fallback.to_string());
                    }
                }
            }
            let (api_id, start_dt) = if interval == "1m" {
                (
                    "usa06011",
                    (now_kst.date_naive() - ChronoDuration::days(14))
                        .format("%Y%m%d")
                        .to_string(),
                )
            } else {
                // `usa06012` uses `strt_dt` as earliest cutoff; `exrt_appl_tp: "0"` returns native USD prices
                (
                    "usa06012",
                    (now_kst.date_naive() - ChronoDuration::days(365 * 5))
                        .format("%Y%m%d")
                        .to_string(),
                )
            };
            let mut last_err: Option<ApiError> = None;
            let mut last_ok: Option<(Value, String, String)> = None;
            let mut resolved: Option<(Value, String, String)> = None;
            for exch in candidates {
                let body = if interval == "1m" {
                    json!({
                        "stex_tp": exch,
                        "stk_cd": sym,
                        "strt_dt": start_dt,
                        "tic_scope": "1",
                        "upd_stkpc_tp": "1",
                        "exrt_appl_tp": "0",
                    })
                } else {
                    json!({
                        "stex_tp": exch,
                        "stk_cd": sym,
                        "strt_dt": start_dt,
                        "upd_stkpc_tp": "1",
                        "exrt_appl_tp": "0",
                    })
                };
                match self.tr_post("/api/us/chart", api_id, &body, c_yn, n_key).await {
                    Ok(triple) => {
                        let has_rows = !first_arr(&triple.0, &["result_list", "output", "candles"]).is_empty();
                        if has_rows {
                            self.us_exch_cache.insert(sym.to_uppercase(), exch);
                            resolved = Some(triple);
                            break;
                        }
                        last_ok = Some(triple);
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            if let Some(triple) = resolved.or(last_ok) {
                triple
            } else {
                return Err(last_err.unwrap_or_else(|| ApiError::not_found("stock_not_found", sym)));
            }
        };

        let raw_rows = first_arr(
            &v,
            &[
                "stk_dt_pole_chart_qry",
                "stk_min_pole_chart_qry",
                "result_list",
                "output",
                "candles",
            ],
        );

        let kr_sym = is_kr_symbol(sym);
        let mut candles = Vec::with_capacity(raw_rows.len().min(count));
        let mut oldest_dt: Option<String> = None;
        for r in raw_rows.iter().take(count) {
            let ts = if interval == "1m" {
                let combined = first_str(r, &["cntr_tm", "dt_tm", "timestamp"]).map(|s| s.to_string()).or_else(|| {
                    let d = first_str(r, &["stck_bsop_date", "bus_dt", "dt"])?;
                    let h = first_str(r, &["stck_cntg_hour", "tm"])?;
                    Some(format!("{d}{h}"))
                });
                combined.and_then(|s| {
                    if kr_sym {
                        parse_kst_minute_rfc3339(&s)
                    } else {
                        parse_us_et_minute_rfc3339(&s)
                    }
                })
            } else {
                let d_str = first_str(r, &["dt", "stck_bsop_date", "xymd", "date"]);
                if let Some(d) = d_str {
                    oldest_dt = Some(d.to_string());
                }
                d_str.and_then(parse_yyyymmdd_utc)
            };
            let Some(timestamp) = ts else {
                continue;
            };
            let open = parse_kiwoom_abs(r.get("open_pric").or_else(|| r.get("open")));
            let high = parse_kiwoom_abs(r.get("high_pric").or_else(|| r.get("high")));
            let low = parse_kiwoom_abs(r.get("low_pric").or_else(|| r.get("low")));
            let close = parse_kiwoom_abs(
                r.get("cur_prc")
                    .or_else(|| r.get("clos_pric"))
                    .or_else(|| r.get("close")),
            );
            let vol = parse_kiwoom_abs(
                r.get("trde_qty")
                    .or_else(|| r.get("acc_trde_qty"))
                    .or_else(|| r.get("vol")),
            )
            .unwrap_or(0.0);
            if let (Some(o), Some(h), Some(l), Some(c)) = (open, high, low, close) {
                candles.push(json!({
                    "timestamp": timestamp,
                    "openPrice": o,
                    "highPrice": h,
                    "lowPrice": l,
                    "closePrice": c,
                    "volume": vol,
                }));
            }
        }

        let next_before = if !is_kr_symbol(sym) || interval == "1m" {
            if cont_yn == "Y" && !next_key.is_empty() {
                Value::String(format!("CONT:{next_key}"))
            } else {
                Value::Null
            }
        } else if cont_yn == "Y" {
            oldest_dt
                .and_then(|d| {
                    let digits: String = d.chars().filter(|c| c.is_ascii_digit()).collect();
                    if digits.len() >= 8 {
                        let nd = NaiveDate::parse_from_str(&digits[..8], "%Y%m%d").ok()?;
                        Some((nd - ChronoDuration::days(1)).to_string())
                    } else {
                        None
                    }
                })
                .map(Value::String)
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };

        Ok(json!({
            "candles": candles,
            "nextBefore": next_before,
        }))
    }

    async fn fetch_stocks_all(&self, market_code: &str) -> ApiResult<Vec<Value>> {
        if matches!(market_code, "KOSPI" | "KOSDAQ" | "KR_ETC") {
            let mrkt_tp = match market_code {
                "KOSPI" => "0",
                "KOSDAQ" => "10",
                _ => "8",
            };
            let (v, _, _) = self
                .tr_post("/api/dostk/stkinfo", "ka10099", &json!({ "mrkt_tp": mrkt_tp }), "N", "")
                .await?;
            let rows = first_arr(&v, &["list", "output"]);
            let mut out = Vec::with_capacity(rows.len());
            for r in rows {
                let code = first_str(r, &["code", "stk_cd"]).map(strip_kr_prefix).unwrap_or_default();
                let name = first_str(r, &["name", "stk_nm"]).unwrap_or("").to_string();
                if code.len() == 6 && !name.is_empty() {
                    out.push(json!({
                        "symbol": code,
                        "name": name,
                        "market": market_code,
                        "englishName": "",
                    }));
                }
            }
            return Ok(out);
        }

        // Official Kiwoom US stock list TR (`usa10099`) + seed fallback if empty
        let exch = match market_code {
            "NASDAQ" => "ND",
            "NYSE" => "NY",
            "AMEX" => "NA",
            _ => return Ok(vec![]),
        };
        if let Ok((v, _, _)) = self
            .tr_post("/api/us/stkinfo", "usa10099", &json!({ "stex_tp": exch }), "N", "")
            .await
        {
            let rows = first_arr(&v, &["list", "result_list", "output"]);
            if !rows.is_empty() {
                let out = rows
                    .iter()
                    .filter_map(|r| {
                        let sym = first_str(r, &["stk_cd", "symb", "code"])?.to_uppercase();
                        let stex = first_str(r, &["stex_tp"]).unwrap_or(exch);
                        if matches!(stex, "ND" | "NY" | "NA") {
                            self.us_exch_cache.insert(sym.clone(), stex.to_string());
                        }
                        let name = first_str(r, &["stk_nm", "kor_nm", "name"]).unwrap_or(&sym);
                        let eng = first_str(r, &["stk_enm", "eng_nm", "englishName"]).unwrap_or(name);
                        Some(json!({
                            "symbol": sym,
                            "name": name,
                            "market": market_code,
                            "englishName": eng,
                        }))
                    })
                    .collect::<Vec<_>>();
                if !out.is_empty() {
                    return Ok(out);
                }
            }
        }
        Ok(default_us_seed(market_code))
    }

    async fn fetch_price_limits(&self, sym: &str) -> ApiResult<Value> {
        if !is_kr_symbol(sym) {
            return Ok(json!({
                "upperLimitPrice": Value::Null,
                "lowerLimitPrice": Value::Null,
            }));
        }
        let (v, _, _) = self
            .tr_post("/api/dostk/stkinfo", "ka10001", &json!({ "stk_cd": sym }), "N", "")
            .await?;
        Ok(json!({
            "upperLimitPrice": parse_kiwoom_abs(v.get("upl_pric")),
            "lowerLimitPrice": parse_kiwoom_abs(v.get("lst_pric")),
        }))
    }

    async fn fetch_orderbook(&self, sym: &str) -> ApiResult<Value> {
        if !is_kr_symbol(sym) {
            let exch = self.resolve_us_exch(sym).await;
            let Ok((v, _, _)) = self
                .tr_post(
                    "/api/us/mrkcond",
                    "usa20101",
                    &json!({ "stex_tp": exch, "stk_cd": sym }),
                    "N",
                    "",
                )
                .await
            else {
                return Ok(json!({ "asks": [], "bids": [], "totalAskVolume": 0, "totalBidVolume": 0 }));
            };
            let mut asks = Vec::new();
            let mut bids = Vec::new();
            for i in 1..=10 {
                let apk = format!("sel_{i}bid");
                let aqk = format!("sel_{i}bid_req");
                let px_opt = parse_kiwoom_abs(v.get(&apk)).or_else(|| {
                    if i == 1 { parse_kiwoom_abs(v.get("fpr_sel_bid")) } else { None }
                });
                if let Some(px) = px_opt {
                    if px > 0.0 {
                        asks.push(json!({
                            "price": px,
                            "volume": parse_kiwoom_abs(v.get(&aqk)).unwrap_or(0.0),
                        }));
                    }
                }
                let bpk = format!("buy_{i}bid");
                let bqk = format!("buy_{i}bid_req");
                let bpx_opt = parse_kiwoom_abs(v.get(&bpk)).or_else(|| {
                    if i == 1 { parse_kiwoom_abs(v.get("fpr_buy_bid")) } else { None }
                });
                if let Some(px) = bpx_opt {
                    if px > 0.0 {
                        bids.push(json!({
                            "price": px,
                            "volume": parse_kiwoom_abs(v.get(&bqk)).unwrap_or(0.0),
                        }));
                    }
                }
            }
            let tot_ask = parse_kiwoom_abs(v.get("tot_sel_req")).unwrap_or_else(|| {
                asks.iter().filter_map(|x| x["volume"].as_f64()).sum()
            });
            let tot_bid = parse_kiwoom_abs(v.get("tot_buy_req")).unwrap_or_else(|| {
                bids.iter().filter_map(|x| x["volume"].as_f64()).sum()
            });
            return Ok(json!({
                "asks": asks,
                "bids": bids,
                "totalAskVolume": tot_ask,
                "totalBidVolume": tot_bid,
            }));
        }
        let (v, _, _) = self
            .tr_post("/api/dostk/mrkcond", "ka10004", &json!({ "stk_cd": sym }), "N", "")
            .await?;

        let ask_keys = [
            ("sel_fpr_bid", "sel_fpr_req"),
            ("sel_2th_pre_bid", "sel_2th_pre_req"),
            ("sel_3th_pre_bid", "sel_3th_pre_req"),
            ("sel_4th_pre_bid", "sel_4th_pre_req"),
            ("sel_5th_pre_bid", "sel_5th_pre_req"),
            ("sel_6th_pre_bid", "sel_6th_pre_req"),
            ("sel_7th_pre_bid", "sel_7th_pre_req"),
            ("sel_8th_pre_bid", "sel_8th_pre_req"),
            ("sel_9th_pre_bid", "sel_9th_pre_req"),
            ("sel_10th_pre_bid", "sel_10th_pre_req"),
        ];
        let bid_keys = [
            ("buy_fpr_bid", "buy_fpr_req"),
            ("buy_2th_pre_bid", "buy_2th_pre_req"),
            ("buy_3th_pre_bid", "buy_3th_pre_req"),
            ("buy_4th_pre_bid", "buy_4th_pre_req"),
            ("buy_5th_pre_bid", "buy_5th_pre_req"),
            ("buy_6th_pre_bid", "buy_6th_pre_req"),
            ("buy_7th_pre_bid", "buy_7th_pre_req"),
            ("buy_8th_pre_bid", "buy_8th_pre_req"),
            ("buy_9th_pre_bid", "buy_9th_pre_req"),
            ("buy_10th_pre_bid", "buy_10th_pre_req"),
        ];

        let mut asks = Vec::new();
        for (pk, qk) in ask_keys {
            if let Some(px) = parse_kiwoom_abs(v.get(pk)) {
                if px > 0.0 {
                    let qty = parse_kiwoom_abs(v.get(qk)).unwrap_or(0.0);
                    asks.push(json!({ "price": px, "volume": qty }));
                }
            }
        }
        let mut bids = Vec::new();
        for (pk, qk) in bid_keys {
            if let Some(px) = parse_kiwoom_abs(v.get(pk)) {
                if px > 0.0 {
                    let qty = parse_kiwoom_abs(v.get(qk)).unwrap_or(0.0);
                    bids.push(json!({ "price": px, "volume": qty }));
                }
            }
        }
        let tot_ask = parse_kiwoom_abs(v.get("tot_sel_req")).unwrap_or_else(|| {
            asks.iter().filter_map(|x| x["volume"].as_f64()).sum()
        });
        let tot_bid = parse_kiwoom_abs(v.get("tot_buy_req")).unwrap_or_else(|| {
            bids.iter().filter_map(|x| x["volume"].as_f64()).sum()
        });
        Ok(json!({
            "asks": asks,
            "bids": bids,
            "totalAskVolume": tot_ask,
            "totalBidVolume": tot_bid,
        }))
    }

    async fn fetch_trades(&self, sym: &str, count: usize) -> ApiResult<Vec<Value>> {
        if !is_kr_symbol(sym) {
            let exch = self.resolve_us_exch(sym).await;
            let Ok((v, _, _)) = self
                .tr_post(
                    "/api/us/mrkcond",
                    "usa20151",
                    &json!({ "stex_tp": exch, "stk_cd": sym, "base_dt": "" }),
                    "N",
                    "",
                )
                .await
            else {
                return Ok(vec![]);
            };
            let rows = first_arr(&v, &["result_list", "output"]);
            let mut out = Vec::with_capacity(rows.len().min(count));
            for r in rows.iter().take(count) {
                let px = parse_kiwoom_abs(r.get("cur_prc")).unwrap_or(0.0);
                let diff = parse_kiwoom_signed(r.get("pred_pre")).unwrap_or(0.0);
                let vol = parse_kiwoom_abs(r.get("acc_trde_qty").or_else(|| r.get("trde_qty"))).unwrap_or(0.0);
                let tm = first_str(r, &["dt", "tm"]).unwrap_or("");
                if px > 0.0 {
                    out.push(json!({
                        "price": px,
                        "volume": vol,
                        "side": if diff < 0.0 { "SELL" } else { "BUY" },
                        "timestamp": tm,
                    }));
                }
            }
            return Ok(out);
        }
        let (v, _, _) = self
            .tr_post("/api/dostk/stkinfo", "ka10003", &json!({ "stk_cd": sym }), "N", "")
            .await?;
        let rows = first_arr(&v, &["cntr_infr", "output"]);
        let mut out = Vec::with_capacity(rows.len().min(count));
        for r in rows.iter().take(count) {
            let raw_px = parse_kiwoom_signed(r.get("cur_prc")).unwrap_or(0.0);
            let px = raw_px.abs();
            let raw_vol = parse_kiwoom_signed(r.get("cntr_trde_qty").or_else(|| r.get("trde_qty"))).unwrap_or(0.0);
            let side = if raw_vol < 0.0 { "SELL" } else { "BUY" };
            let tm = first_str(r, &["tm", "cntr_tm"]).unwrap_or("");
            if px > 0.0 {
                out.push(json!({
                    "price": px,
                    "volume": raw_vol.abs(),
                    "side": side,
                    "timestamp": tm,
                }));
            }
        }
        Ok(out)
    }

    async fn fetch_indices_prices(&self, syms: &[&str]) -> Vec<Value> {
        let mut out = Vec::new();
        for sym in syms {
            let (mrkt_tp, inds_cd) = match *sym {
                "KOSPI" => ("0", "001"),
                "KOSDAQ" => ("1", "101"),
                _ => continue,
            };
            if let Ok((v, _, _)) = self
                .tr_post(
                    "/api/dostk/sect",
                    "ka20001",
                    &json!({ "mrkt_tp": mrkt_tp, "inds_cd": inds_cd }),
                    "N",
                    "",
                )
                .await
            {
                if let Some(px) = parse_kiwoom_abs(v.get("cur_prc")) {
                    out.push(json!({ "symbol": sym, "lastPrice": px }));
                }
            }
        }
        out
    }

    async fn fetch_index_candles(&self, sym: &str) -> Value {
        let (mrkt_tp, inds_cd) = match sym {
            "KOSPI" => ("0", "001"),
            "KOSDAQ" => ("1", "101"),
            _ => return json!({ "candles": [] }),
        };
        if let Ok((v, _, _)) = self
            .tr_post(
                "/api/dostk/sect",
                "ka20001",
                &json!({ "mrkt_tp": mrkt_tp, "inds_cd": inds_cd }),
                "N",
                "",
            )
            .await
        {
            let cur = parse_kiwoom_abs(v.get("cur_prc")).unwrap_or(0.0);
            let diff = parse_kiwoom_signed(v.get("pred_pre")).unwrap_or(0.0);
            let prev = (cur - diff).max(0.0);
            return json!({
                "candles": [
                    { "closePrice": cur },
                    { "closePrice": prev },
                ]
            });
        }
        json!({ "candles": [] })
    }

    async fn fetch_investor_trading(&self, sym: &str, count: usize) -> ApiResult<Value> {
        let kst = FixedOffset::east_opt(9 * 3600).unwrap();
        let today = Utc::now().with_timezone(&kst).format("%Y%m%d").to_string();
        let (v, _, _) = self
            .tr_post(
                "/api/dostk/stkinfo",
                "ka10059",
                &json!({
                    "dt": today,
                    "stk_cd": sym,
                    "amt_qty_tp": "2",
                    "trde_tp": "0",
                    "unit_tp": "1",
                }),
                "N",
                "",
            )
            .await?;
        let rows = first_arr(&v, &["stk_invsr_orgn", "output"]);
        let mut records = Vec::with_capacity(rows.len().min(count));
        for r in rows.iter().take(count) {
            let d = first_str(r, &["dt"]).unwrap_or("");
            let date_iso = if d.len() == 8 {
                format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..8])
            } else {
                d.to_string()
            };
            let ind = parse_kiwoom_signed(r.get("ind_invsr")).unwrap_or(0.0);
            let frg = parse_kiwoom_signed(r.get("frgnr_invsr")).unwrap_or(0.0);
            let inst = parse_kiwoom_signed(r.get("orgn")).unwrap_or(0.0);
            records.push(json!({
                "date": date_iso,
                "individual": { "netBuyVolume": ind },
                "foreigner": { "netBuyVolume": frg },
                "institution": { "netBuyVolume": inst },
                "foreignerHolding": { "holdingRate": Value::Null },
            }));
        }
        Ok(json!({ "records": records }))
    }

    async fn fetch_holdings(&self) -> ApiResult<Value> {
        let (v, _, _) = self
            .tr_post(
                "/api/dostk/acnt",
                "kt00018",
                &json!({ "qry_tp": "1", "dmst_stex_tp": "KRX" }),
                "N",
                "",
            )
            .await?;

        let raw_items = first_arr(
            &v,
            &["acnt_evlu_remn_indv_tot", "acnt_evlu_remn_indict", "stk_acnt_evlt_prst", "output"],
        );

        let mut items = Vec::new();
        let mut krw_mv = 0.0;
        let mut krw_pl = 0.0;
        let mut krw_cost = 0.0;

        for r in raw_items {
            let qty = parse_kiwoom_abs(r.get("rmnd_qty").or_else(|| r.get("hldg_qty"))).unwrap_or(0.0);
            if qty <= 0.0 {
                continue;
            }
            let sym = first_str(r, &["stk_cd"]).map(strip_kr_prefix).unwrap_or_default();
            let name = first_str(r, &["stk_nm"]).unwrap_or(&sym).to_string();
            let avg_px = parse_kiwoom_abs(r.get("pur_pric").or_else(|| r.get("pchs_avg_uv"))).unwrap_or(0.0);
            let cur_px = parse_kiwoom_abs(r.get("cur_prc")).unwrap_or(avg_px);
            let mv = parse_kiwoom_abs(r.get("evlt_amt").or_else(|| r.get("evlu_amt")))
                .unwrap_or(qty * cur_px);
            let pl = parse_kiwoom_signed(r.get("evltv_prft").or_else(|| r.get("evlu_pfls_amt")))
                .unwrap_or(mv - qty * avg_px);
            let rate = parse_kiwoom_signed(r.get("prft_rt").or_else(|| r.get("earn_rt")))
                .map(|pct| pct / 100.0)
                .unwrap_or_else(|| if qty * avg_px > 0.0 { pl / (qty * avg_px) } else { 0.0 });
            krw_mv += mv;
            krw_pl += pl;
            krw_cost += qty * avg_px;
            items.push(json!({
                "symbol": sym,
                "name": name,
                "market": "KR",
                "currency": "KRW",
                "quantity": qty,
                "averagePurchasePrice": avg_px,
                "lastPrice": cur_px,
                "marketValue": { "amount": mv, "amountAfterCost": mv },
                "profitLoss": {
                    "amount": pl,
                    "amountAfterCost": pl,
                    "rate": rate,
                    "rateAfterCost": rate,
                },
                "dailyProfitLoss": {
                    "amount": 0.0,
                    "rate": 0.0,
                },
            }));
        }

        // Official Kiwoom US holdings (`ust21070`, graceful skip if US account not activated)
        let mut usd_mv: Option<f64> = None;
        let mut usd_pl: Option<f64> = None;
        if let Ok((usv, _, _)) = self
            .tr_post(
                "/api/us/acnt",
                "ust21070",
                &json!({ "stex_tp": "", "stk_cd": "" }),
                "N",
                "",
            )
            .await
        {
            let us_rows = first_arr(&usv, &["result_list", "result_lsit", "output", "list"]);
            for r in us_rows {
                let qty = parse_kiwoom_abs(
                    r.get("poss_qty")
                        .or_else(|| r.get("qty"))
                        .or_else(|| r.get("rmnd_qty"))
                        .or_else(|| r.get("hldg_qty")),
                )
                .unwrap_or(0.0);
                if qty <= 0.0 {
                    continue;
                }
                let sym = first_str(r, &["stk_cd", "symb"]).unwrap_or("").to_uppercase();
                let name = first_str(r, &["frgn_stk_nm", "stk_nm", "name"]).unwrap_or(&sym).to_string();
                let avg_px = parse_kiwoom_abs(
                    r.get("frgn_stk_book_uv")
                        .or_else(|| r.get("pur_pric"))
                        .or_else(|| r.get("pchs_avg_uv")),
                )
                .unwrap_or(0.0);
                let cur_px = parse_kiwoom_abs(r.get("now_pric").or_else(|| r.get("cur_prc"))).unwrap_or(avg_px);
                let mv = parse_kiwoom_abs(r.get("evlt_amt").or_else(|| r.get("evlu_amt")))
                    .unwrap_or(qty * cur_px);
                let pl = parse_kiwoom_signed(
                    r.get("pl_amt")
                        .or_else(|| r.get("evltv_prft"))
                        .or_else(|| r.get("evlu_pfls_amt")),
                )
                .unwrap_or(mv - qty * avg_px);
                let rate = parse_kiwoom_signed(r.get("pl_rt").or_else(|| r.get("prft_rt")))
                    .map(|pct| pct / 100.0)
                    .unwrap_or_else(|| if qty * avg_px > 0.0 { pl / (qty * avg_px) } else { 0.0 });
                usd_mv = Some(usd_mv.unwrap_or(0.0) + mv);
                usd_pl = Some(usd_pl.unwrap_or(0.0) + pl);
                items.push(json!({
                    "symbol": sym,
                    "name": name,
                    "market": "US",
                    "currency": "USD",
                    "quantity": qty,
                    "averagePurchasePrice": avg_px,
                    "lastPrice": cur_px,
                    "marketValue": { "amount": mv, "amountAfterCost": mv },
                    "profitLoss": {
                        "amount": pl,
                        "amountAfterCost": pl,
                        "rate": rate,
                        "rateAfterCost": rate,
                    },
                    "dailyProfitLoss": { "amount": 0.0, "rate": 0.0 },
                }));
            }
            if usd_mv.is_none() {
                usd_mv = parse_kiwoom_abs(usv.get("tot_evlt_amt"));
            }
            if usd_pl.is_none() {
                usd_pl = parse_kiwoom_signed(usv.get("tot_pl_amt"));
            }
        }

        let tot_mv = parse_kiwoom_abs(v.get("tot_evlt_amt").or_else(|| v.get("evlu_amt"))).unwrap_or(krw_mv);
        let tot_pl = parse_kiwoom_signed(v.get("tot_evlt_pl").or_else(|| v.get("evlu_pfls_amt"))).unwrap_or(krw_pl);
        let tot_rate = parse_kiwoom_signed(v.get("tot_prft_rt").or_else(|| v.get("tot_earn_rt")))
            .map(|pct| pct / 100.0)
            .unwrap_or_else(|| if krw_cost > 0.0 { tot_pl / krw_cost } else { 0.0 });

        Ok(json!({
            "marketValue": {
                "amount": { "krw": tot_mv, "usd": usd_mv },
                "amountAfterCost": { "krw": tot_mv, "usd": usd_mv },
            },
            "profitLoss": {
                "amount": { "krw": tot_pl, "usd": usd_pl },
                "amountAfterCost": { "krw": tot_pl, "usd": usd_pl },
                "rate": tot_rate,
                "rateAfterCost": tot_rate,
            },
            "dailyProfitLoss": {
                "amount": { "krw": 0.0, "usd": Value::Null },
                "rate": 0.0,
            },
            "items": items,
        }))
    }

    async fn fetch_buying_power(&self, currency: &str) -> ApiResult<Value> {
        if currency.eq_ignore_ascii_case("USD") {
            let cash = match self.tr_post("/api/us/acnt", "ust21110", &json!({}), "N", "").await {
                Ok((v, _, _)) => {
                    let rows = first_arr(&v, &["result_list", "result_lsit", "output"]);
                    let usd_row = rows.iter().find(|r| {
                        first_str(r, &["crnc_code"]).is_some_and(|c| c.eq_ignore_ascii_case("USD"))
                    });
                    usd_row
                        .and_then(|r| {
                            parse_kiwoom_abs(r.get("fc_ord_alowa"))
                                .or_else(|| parse_kiwoom_abs(r.get("fc_pymn_alowa")))
                                .or_else(|| parse_kiwoom_abs(r.get("fc_entra")))
                        })
                        .or_else(|| parse_kiwoom_abs(v.get("fc_ord_alowa")))
                        .unwrap_or(0.0)
                }
                Err(_) => 0.0,
            };
            return Ok(json!({ "cashBuyingPower": cash }));
        }
        let (v, _, _) = self
            .tr_post("/api/dostk/acnt", "kt00001", &json!({ "qry_tp": "3" }), "N", "")
            .await?;
        let cash = parse_kiwoom_abs(v.get("ord_alow_amt"))
            .or_else(|| parse_kiwoom_abs(v.get("ord_psbl_cash")))
            .or_else(|| parse_kiwoom_abs(v.get("d2_entra")))
            .or_else(|| parse_kiwoom_abs(v.get("d2_excc_amt")))
            .or_else(|| parse_kiwoom_abs(v.get("entr")))
            .unwrap_or(0.0);
        Ok(json!({ "cashBuyingPower": cash }))
    }

    async fn fetch_sellable_quantity(&self, sym: &str) -> ApiResult<Value> {
        if !is_kr_symbol(sym) {
            if let Ok((usv, _, _)) = self
                .tr_post(
                    "/api/us/acnt",
                    "ust21070",
                    &json!({ "stex_tp": "", "stk_cd": sym }),
                    "N",
                    "",
                )
                .await
            {
                let rows = first_arr(&usv, &["result_list", "result_lsit", "output"]);
                if let Some(r) = rows.iter().find(|r| {
                    first_str(r, &["stk_cd", "symb"]).is_some_and(|s| s.eq_ignore_ascii_case(sym))
                }) {
                    let q = parse_kiwoom_abs(
                        r.get("sell_alowq")
                            .or_else(|| r.get("poss_qty"))
                            .or_else(|| r.get("qty")),
                    )
                    .unwrap_or(0.0);
                    return Ok(json!({ "sellableQuantity": q }));
                }
            }
        }
        let holdings = self.fetch_holdings().await?;
        let qty = holdings
            .get("items")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|it| it.get("symbol").and_then(|s| s.as_str()) == Some(sym))
            })
            .and_then(|it| it.get("quantity").and_then(|q| q.as_f64()))
            .unwrap_or(0.0);
        Ok(json!({ "sellableQuantity": qty }))
    }

    async fn place_order(&self, payload: &Value) -> ApiResult<Value> {
        let sym = payload.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let side = payload.get("side").and_then(|v| v.as_str()).unwrap_or("BUY");
        let otype = payload.get("orderType").and_then(|v| v.as_str()).unwrap_or("LIMIT");
        let tif = payload.get("timeInForce").and_then(|v| v.as_str()).unwrap_or("DAY");
        let qty = payload.get("quantity").and_then(|v| v.as_str()).unwrap_or("0");
        let price = payload.get("price").and_then(|v| v.as_str()).unwrap_or("");
        let cid = payload.get("clientOrderId").and_then(|v| v.as_str()).unwrap_or("");

        let (v, currency) = if is_kr_symbol(sym) {
            let api_id = if side == "SELL" { "kt10001" } else { "kt10000" };
            let trde_tp = if otype == "MARKET" { "3" } else { "0" };
            let ord_uv = if otype == "MARKET" { "" } else { price };
            let body = json!({
                "dmst_stex_tp": "KRX",
                "stk_cd": sym,
                "ord_qty": qty,
                "ord_uv": ord_uv,
                "trde_tp": trde_tp,
                "cond_uv": "",
            });
            let (resp, _, _) = self.tr_post("/api/dostk/ordr", api_id, &body, "N", "").await?;
            (resp, "KRW")
        } else {
            let exch = self.resolve_us_exch(sym).await;
            let api_id = if side == "SELL" { "ust20001" } else { "ust20000" };
            let trde_tp = match (otype, tif) {
                ("MARKET", _) => "03",
                (_, "CLS") => "30",
                _ => "00",
            };
            let ord_uv = if otype == "MARKET" { "" } else { price };
            let body = if side == "SELL" {
                json!({
                    "stex_tp": exch,
                    "stk_cd": sym,
                    "ord_qty": qty,
                    "ord_uv": ord_uv,
                    "stop_pric": "",
                    "trde_tp": trde_tp,
                })
            } else {
                json!({
                    "stex_tp": exch,
                    "stk_cd": sym,
                    "ord_qty": qty,
                    "ord_uv": ord_uv,
                    "trde_tp": trde_tp,
                })
            };
            let (resp, _, _) = self.tr_post("/api/us/ordr", api_id, &body, "N", "").await?;
            (resp, "USD")
        };

        let ord_no = first_str(&v, &["ord_no", "order_no", "odno"])
            .unwrap_or(cid)
            .to_string();
        Ok(json!({
            "orderId": ord_no,
            "clientOrderId": cid,
            "symbol": sym,
            "side": side,
            "orderType": otype,
            "quantity": qty.parse::<f64>().unwrap_or(0.0),
            "price": price.parse::<f64>().ok(),
            "filledQuantity": 0.0,
            "status": "PENDING",
            "currency": currency,
            "createdAt": Utc::now().to_rfc3339(),
        }))
    }

    async fn list_orders(&self, status: &str, sym_filter: Option<&str>) -> ApiResult<Value> {
        let mut orders = Vec::new();
        let want_kr = sym_filter.map(is_kr_symbol).unwrap_or(true);
        let want_us = sym_filter.map(|s| !is_kr_symbol(s)).unwrap_or(true);

        if want_kr {
            if status == "OPEN" {
                if let Ok((v, _, _)) = self
                    .tr_post(
                        "/api/dostk/acnt",
                        "ka10075",
                        &json!({ "all_stk_tp": "0", "trde_tp": "0", "stk_cd": "", "stex_tp": "0" }),
                        "N",
                        "",
                    )
                    .await
                {
                    for r in first_arr(&v, &["oso", "output", "list"]) {
                        let sym = first_str(r, &["stk_cd"]).map(strip_kr_prefix).unwrap_or_default();
                        if let Some(f) = sym_filter {
                            if sym != f {
                                continue;
                            }
                        }
                        let ord_no = first_str(r, &["ord_no"]).unwrap_or("").to_string();
                        let qty = parse_kiwoom_abs(r.get("ord_qty")).unwrap_or(0.0);
                        let rem = parse_kiwoom_abs(r.get("oso_qty").or_else(|| r.get("un_cntr_qty"))).unwrap_or(qty);
                        let filled = (qty - rem).max(0.0);
                        let px = parse_kiwoom_abs(r.get("ord_pric").or_else(|| r.get("ord_uv")));
                        let io_tp = first_str(r, &["io_tp_nm", "sll_buy_tp_nm", "trde_tp"]).unwrap_or("");
                        let side = if io_tp.contains("매도") || io_tp.eq_ignore_ascii_case("SELL") {
                            "SELL"
                        } else {
                            "BUY"
                        };
                        orders.push(json!({
                            "orderId": ord_no,
                            "symbol": sym,
                            "name": first_str(r, &["stk_nm"]).unwrap_or(&sym),
                            "side": side,
                            "orderType": "LIMIT",
                            "quantity": qty,
                            "filledQuantity": filled,
                            "price": px,
                            "status": if filled > 0.0 { "PARTIAL" } else { "PENDING" },
                            "currency": "KRW",
                            "createdAt": first_str(r, &["tm"]).unwrap_or(""),
                        }));
                    }
                }
            } else if let Ok((v, _, _)) = self
                .tr_post(
                    "/api/dostk/acnt",
                    "ka10076",
                    &json!({ "stk_cd": "", "qry_tp": "0", "sell_tp": "0", "ord_no": "", "stex_tp": "0" }),
                    "N",
                    "",
                )
                .await
            {
                for r in first_arr(&v, &["cntr", "output", "list"]) {
                    let sym = first_str(r, &["stk_cd"]).map(strip_kr_prefix).unwrap_or_default();
                    if let Some(f) = sym_filter {
                        if sym != f {
                            continue;
                        }
                    }
                    let ord_no = first_str(r, &["ord_no"]).unwrap_or("").to_string();
                    let qty = parse_kiwoom_abs(r.get("ord_qty")).unwrap_or(0.0);
                    let filled = parse_kiwoom_abs(r.get("cntr_qty")).unwrap_or(qty);
                    let fill_px = parse_kiwoom_abs(r.get("cntr_pric").or_else(|| r.get("ord_pric")));
                    let io_tp = first_str(r, &["io_tp_nm", "sll_buy_tp_nm"]).unwrap_or("");
                    let side = if io_tp.contains("매도") || io_tp.eq_ignore_ascii_case("SELL") {
                        "SELL"
                    } else {
                        "BUY"
                    };
                    orders.push(json!({
                        "orderId": ord_no,
                        "symbol": sym,
                        "name": first_str(r, &["stk_nm"]).unwrap_or(&sym),
                        "side": side,
                        "orderType": "LIMIT",
                        "quantity": qty,
                        "filledQuantity": filled,
                        "price": fill_px,
                        "averageFilledPrice": fill_px,
                        "status": "FILLED",
                        "currency": "KRW",
                        "createdAt": first_str(r, &["ord_tm", "tm"]).unwrap_or(""),
                    }));
                }
            }
        }

        // Official Kiwoom US today's orders (`ust21510` — note Kiwoom's response key typo `result_lsit`)
        if want_us {
            if let Ok((usv, _, _)) = self
                .tr_post(
                    "/api/us/acnt",
                    "ust21510",
                    &json!({ "slby_tp": "0", "stex_tp": "", "stk_cd": sym_filter.unwrap_or("") }),
                    "N",
                    "",
                )
                .await
            {
                for r in first_arr(&usv, &["result_lsit", "result_list", "output"]) {
                    let sym = first_str(r, &["stk_cd"]).unwrap_or("").to_uppercase();
                    if sym.is_empty() {
                        continue;
                    }
                    if let Some(f) = sym_filter {
                        if !sym.eq_ignore_ascii_case(f) {
                            continue;
                        }
                    }
                    let ord_no = first_str(r, &["ord_no"]).unwrap_or("").to_string();
                    let qty = parse_kiwoom_abs(r.get("ord_qty")).unwrap_or(0.0);
                    let filled = parse_kiwoom_abs(r.get("cntr_qty")).unwrap_or(0.0);
                    let rem = parse_kiwoom_abs(r.get("ord_remnq")).unwrap_or((qty - filled).max(0.0));
                    let stat = first_str(r, &["ord_stat", "ord_stat_nm"]).unwrap_or("");
                    let is_open = rem > 0.0 && !stat.contains("취소") && !stat.contains("체결완료");
                    if (status == "OPEN" && !is_open) || (status != "OPEN" && is_open) {
                        continue;
                    }
                    let px = parse_kiwoom_abs(r.get("ord_uv"));
                    let fill_px = parse_kiwoom_abs(r.get("cntr_uv")).filter(|x| *x > 0.0).or(px);
                    let slby = first_str(r, &["slby_tp_nm", "slby_tp"]).unwrap_or("");
                    let side = if slby.contains("매도") || slby == "1" { "SELL" } else { "BUY" };
                    let o_stat = if stat.contains("취소") {
                        "CANCELED"
                    } else if !is_open && filled > 0.0 {
                        "FILLED"
                    } else if filled > 0.0 {
                        "PARTIAL"
                    } else {
                        "PENDING"
                    };
                    orders.push(json!({
                        "orderId": ord_no,
                        "symbol": sym,
                        "name": first_str(r, &["frgn_stk_nm", "stk_nm"]).unwrap_or(&sym),
                        "side": side,
                        "orderType": if first_str(r, &["frgn_trde_tp"]) == Some("03") { "MARKET" } else { "LIMIT" },
                        "quantity": qty,
                        "filledQuantity": filled,
                        "price": px,
                        "averageFilledPrice": fill_px,
                        "status": o_stat,
                        "currency": "USD",
                        "createdAt": first_str(r, &["ord_time", "cntr_time"]).unwrap_or(""),
                    }));
                }
            }
        }

        Ok(json!({ "orders": orders, "hasNext": false }))
    }

    async fn get_order(&self, order_id: &str) -> ApiResult<Value> {
        if let Ok(closed) = self.list_orders("CLOSED", None).await {
            if let Some(found) = closed
                .get("orders")
                .and_then(|v| v.as_array())
                .and_then(|a| a.iter().find(|o| o.get("orderId").and_then(|x| x.as_str()) == Some(order_id)))
            {
                return Ok(found.clone());
            }
        }
        if let Ok(open) = self.list_orders("OPEN", None).await {
            if let Some(found) = open
                .get("orders")
                .and_then(|v| v.as_array())
                .and_then(|a| a.iter().find(|o| o.get("orderId").and_then(|x| x.as_str()) == Some(order_id)))
            {
                return Ok(found.clone());
            }
        }
        Err(ApiError::not_found("order_not_found", order_id))
    }

    async fn cancel_order(&self, order_id: &str) -> ApiResult<Value> {
        let cur = self.get_order(order_id).await.ok();
        let sym = cur
            .as_ref()
            .and_then(|o| o.get("symbol").and_then(|v| v.as_str()))
            .unwrap_or("");
        if sym.is_empty() || is_kr_symbol(sym) {
            let (v, _, _) = self
                .tr_post(
                    "/api/dostk/ordr",
                    "kt10003",
                    &json!({
                        "dmst_stex_tp": "KRX",
                        "orig_ord_no": order_id,
                        "stk_cd": sym,
                        "cncl_qty": "0",
                    }),
                    "N",
                    "",
                )
                .await?;
            Ok(json!({
                "orderId": first_str(&v, &["ord_no"]).unwrap_or(order_id),
                "status": "CANCELED",
            }))
        } else {
            let exch = self.resolve_us_exch(sym).await;
            let (v, _, _) = self
                .tr_post(
                    "/api/us/ordr",
                    "ust20003",
                    &json!({
                        "orig_ord_no": order_id,
                        "stex_tp": exch,
                        "stk_cd": sym,
                    }),
                    "N",
                    "",
                )
                .await?;
            Ok(json!({
                "orderId": first_str(&v, &["ord_no"]).unwrap_or(order_id),
                "status": "CANCELED",
            }))
        }
    }

    async fn modify_order(&self, order_id: &str, payload: &Value) -> ApiResult<Value> {
        let cur = self.get_order(order_id).await?;
        let sym = cur.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        let qty = payload
            .get("quantity")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| cur.get("quantity").and_then(|v| v.as_f64()).unwrap_or(0.0).round().to_string());
        let uv = payload.get("price").and_then(|v| v.as_str()).unwrap_or("0");

        if is_kr_symbol(sym) {
            let (v, _, _) = self
                .tr_post(
                    "/api/dostk/ordr",
                    "kt10002",
                    &json!({
                        "dmst_stex_tp": "KRX",
                        "orig_ord_no": order_id,
                        "stk_cd": sym,
                        "mdfy_qty": qty,
                        "mdfy_uv": uv,
                        "mdfy_cond_uv": "",
                    }),
                    "N",
                    "",
                )
                .await?;
            Ok(json!({
                "orderId": first_str(&v, &["ord_no"]).unwrap_or(order_id),
                "status": "PENDING",
            }))
        } else {
            let exch = self.resolve_us_exch(sym).await;
            let (v, _, _) = self
                .tr_post(
                    "/api/us/ordr",
                    "ust20002",
                    &json!({
                        "orig_ord_no": order_id,
                        "stex_tp": exch,
                        "stk_cd": sym,
                        "mdfy_uv": uv,
                        "stop_pric": "",
                    }),
                    "N",
                    "",
                )
                .await?;
            Ok(json!({
                "orderId": first_str(&v, &["ord_no"]).unwrap_or(order_id),
                "status": "PENDING",
            }))
        }
    }
}

fn map_kiwoom_error(status: u16, api_id: &str, message: &str) -> ApiError {
    if status == 401 || status == 403 {
        return ApiError::bad_gateway(format!("kiwoom_{api_id}_auth"), message);
    }
    if status == 429 {
        return ApiError::too_many_requests("kiwoom_rate_limited", message);
    }
    if status >= 500 {
        return ApiError::bad_gateway(format!("kiwoom_{api_id}_server"), message);
    }
    ApiError::new(status, format!("kiwoom_{api_id}"), message)
}

pub async fn verify_credentials(
    state: &AppState,
    appkey: &str,
    secretkey: &str,
    account_no: &str,
    is_mock: bool,
) -> ApiResult<()> {
    let client = KiwoomClient::new(
        appkey.to_string(),
        secretkey.to_string(),
        account_no.to_string(),
        is_mock,
        state.store.clone(),
        state.crypto.clone(),
        state.http.clone(),
        state.kiwoom_override.clone(),
    )?;
    if let Err(e) = client.issue_token().await {
        let lower = e.message.to_lowercase();
        let friendly = if lower.contains("ip") || e.message.contains("아이피") {
            "키움 REST API 포털(허용 IP)에 이 서버의 IP가 등록되어 있지 않습니다."
        } else if lower.contains("appkey") || lower.contains("secret") || lower.contains("invalid") {
            "App Key 또는 Secret Key가 올바르지 않습니다. (실전/모의 투자 구분도 함께 확인해 주세요)"
        } else {
            e.message.as_str()
        };
        return Err(ApiError::bad_request("kiwoom_verify_failed", friendly));
    }
    Ok(())
}

fn default_us_seed(market_code: &str) -> Vec<Value> {
    let list: &[(&str, &str, &str)] = match market_code {
        "NASDAQ" => &[
            ("AAPL", "애플", "Apple Inc."),
            ("NVDA", "엔비디아", "NVIDIA Corporation"),
            ("MSFT", "마이크로소프트", "Microsoft Corporation"),
            ("GOOGL", "알파벳 A", "Alphabet Inc."),
            ("AMZN", "아마존", "Amazon.com Inc."),
            ("META", "메타", "Meta Platforms Inc."),
            ("TSLA", "테슬라", "Tesla Inc."),
            ("AVGO", "브로드컴", "Broadcom Inc."),
            ("AMD", "AMD", "Advanced Micro Devices"),
            ("QQQ", "Invesco QQQ", "Invesco QQQ Trust"),
            ("TQQQ", "ProShares 3x 나스닥", "ProShares UltraPro QQQ"),
            ("TLT", "미국 20년 국채 ETF", "iShares 20+ Year Treasury Bond ETF"),
            ("SHY", "미국 단기 국채 ETF", "iShares 1-3 Year Treasury Bond ETF"),
            ("IEF", "미국 중기 국채 ETF", "iShares 7-10 Year Treasury Bond ETF"),
        ],
        "NYSE" => &[
            ("TSM", "TSMC", "Taiwan Semiconductor Manufacturing"),
            ("LLY", "일라이 릴리", "Eli Lilly and Company"),
            ("BRK.B", "버크셔 해서웨이 B", "Berkshire Hathaway Inc."),
            ("JPM", "JP모건", "JPMorgan Chase & Co."),
            ("V", "비자", "Visa Inc."),
            ("KO", "코카콜라", "The Coca-Cola Company"),
            ("ORCL", "오라클", "Oracle Corporation"),
        ],
        "AMEX" => &[
            ("SPY", "S&P500 ETF", "SPDR S&P 500 ETF Trust"),
            ("VOO", "뱅가드 S&P500", "Vanguard S&P 500 ETF"),
            ("VTI", "미국 전체주식 ETF", "Vanguard Total Stock Market ETF"),
            ("SCHD", "슈왑 미국 배당주", "Schwab U.S. Dividend Equity ETF"),
            ("GLD", "금 현물 ETF", "SPDR Gold Shares"),
            ("SOXL", "반도체 3배 ETF", "Direxion Daily Semiconductor Bull 3X"),
            ("SGOV", "초단기 국채 ETF", "iShares 0-3 Month Treasury Bond ETF"),
            ("BIL", "미국 1-3개월 국채", "SPDR Bloomberg 1-3 Month T-Bill ETF"),
        ],
        _ => &[],
    };
    list.iter()
        .map(|(sym, kr, eng)| {
            json!({
                "symbol": sym,
                "name": kr,
                "market": market_code,
                "englishName": eng,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_signed_kiwoom_numbers_and_strips_kr_prefix() {
        assert_eq!(parse_kiwoom_abs(Some(&json!("+71,500"))), Some(71500.0));
        assert_eq!(parse_kiwoom_abs(Some(&json!("-69800"))), Some(69800.0));
        assert_eq!(parse_kiwoom_signed(Some(&json!("-1,250"))), Some(-1250.0));
        assert_eq!(strip_kr_prefix("A005930"), "005930");
        assert_eq!(strip_kr_prefix("000660"), "000660");
        assert_eq!(strip_kr_prefix("AAPL"), "AAPL");
    }

    #[test]
    fn parses_kiwoom_expires_dt_kst_and_synthesizes_calendar() {
        let ts = parse_expires_dt("20261123180000").unwrap();
        assert!(ts > 1_700_000_000.0);
        let cal_kr = synthesize_market_calendar("KR");
        assert!(cal_kr.get("today").is_some());
        let cal_us = synthesize_market_calendar("US");
        assert!(cal_us.get("today").is_some());
    }
}
