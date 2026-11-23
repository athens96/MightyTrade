//! Async Toss WebSocket -> Browser SSE relay (1 Toss WS per user, 5Hz orderbook coalescing).

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::Message;

use crate::integrations::kiwoom::{parse_kiwoom_abs, parse_kiwoom_signed, strip_kr_prefix};
use crate::state::AppState;
use crate::users::credentials::{self, BrokerKind};

const WS_URL: &str = "wss://openapi-ws.tossinvest.com/ws/v1";
const MAX_CODES: usize = 100;
const PING_EVERY: Duration = Duration::from_secs(60);
const IDLE_STOP: Duration = Duration::from_secs(30);
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const TOKEN_ERRORS: [&str; 4] = [
    "auth-revoked",
    "token-revoked",
    "expired-token",
    "invalid-token",
];

static SUB_SEQ: AtomicU64 = AtomicU64::new(1);

fn normalize_ws_nums(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, val) in map {
                let next = match val {
                    Value::Array(arr) => Value::Array(arr.into_iter().map(normalize_ws_nums).collect()),
                    Value::String(ref s) if k == "price" || k == "volume" => s
                        .parse::<f64>()
                        .ok()
                        .filter(|f| f.is_finite())
                        .map(|f| json!(f))
                        .unwrap_or(val),
                    Value::Object(_) => normalize_ws_nums(val),
                    other => other,
                };
                out.insert(k, next);
            }
            Value::Object(out)
        }
        other => other,
    }
}

struct SubMailbox {
    books: HashMap<String, Value>,
    trades: VecDeque<Value>,
    orders: Vec<Value>,
    status: Option<Value>,
}

pub struct Subscriber {
    pub id: u64,
    pub topics: HashSet<String>,
    mailbox: Mutex<SubMailbox>,
    wake: Notify,
}

impl Subscriber {
    pub fn new(topics: HashSet<String>) -> Self {
        Self {
            id: SUB_SEQ.fetch_add(1, Ordering::Relaxed),
            topics,
            mailbox: Mutex::new(SubMailbox {
                books: HashMap::new(),
                trades: VecDeque::with_capacity(300),
                orders: Vec::new(),
                status: None,
            }),
            wake: Notify::new(),
        }
    }

    pub fn push(&self, topic: &str, data: Value) {
        {
            let mut mb = self.mailbox.lock();
            if topic.starts_with("orderbook:") {
                mb.books.insert(topic.to_string(), data);
            } else if topic.starts_with("personal:") {
                mb.orders.push(data);
            } else {
                if mb.trades.len() >= 300 {
                    mb.trades.pop_front();
                }
                mb.trades.push_back(data);
            }
        }
        self.wake.notify_one();
    }

    pub fn push_status(&self, status: Value) {
        {
            self.mailbox.lock().status = Some(status);
        }
        self.wake.notify_one();
    }

    pub fn drain(&self) -> Vec<(&'static str, Value)> {
        let mut mb = self.mailbox.lock();
        let mut out = Vec::new();
        if let Some(st) = mb.status.take() {
            out.push(("status", st));
        }
        for o in mb.orders.drain(..) {
            out.push(("order", o));
        }
        for t in mb.trades.drain(..) {
            out.push(("trade", t));
        }
        for (_, b) in mb.books.drain() {
            out.push(("orderbook", b));
        }
        out
    }

    pub async fn wait_or_timeout(&self, dur: Duration) -> bool {
        tokio::time::timeout(dur, self.wake.notified()).await.is_ok()
    }
}

struct StreamInner {
    subs: HashMap<u64, Arc<Subscriber>>,
    empty_since: Option<Instant>,
    status: Value,
}

pub struct UserStream {
    stream_key: String,
    uid: String,
    kiwoom_us: bool,
    inner: Mutex<StreamInner>,
    decl_notify: Notify,
    stopped: AtomicBool,
}

impl UserStream {
    fn new(stream_key: String, uid: String, kiwoom_us: bool) -> Self {
        Self {
            stream_key,
            uid,
            kiwoom_us,
            inner: Mutex::new(StreamInner {
                subs: HashMap::new(),
                empty_since: None,
                status: json!({ "state": "connecting" }),
            }),
            decl_notify: Notify::new(),
            stopped: AtomicBool::new(false),
        }
    }

    fn force_stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.decl_notify.notify_one();
    }

    fn add(&self, sub: Arc<Subscriber>) {
        let status = {
            let mut g = self.inner.lock();
            g.subs.insert(sub.id, sub.clone());
            g.empty_since = None;
            g.status.clone()
        };
        sub.push_status(status);
        self.decl_notify.notify_one();
    }

    fn remove(&self, sub_id: u64) {
        {
            let mut g = self.inner.lock();
            g.subs.remove(&sub_id);
            if g.subs.is_empty() {
                g.empty_since = Some(Instant::now());
            }
        }
        self.decl_notify.notify_one();
    }

    fn declaration(&self) -> Vec<Value> {
        let mut topics: Vec<String> = {
            let g = self.inner.lock();
            g.subs
                .values()
                .flat_map(|s| s.topics.iter().cloned())
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        topics.sort();
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for t in topics.into_iter().take(MAX_CODES) {
            let parts: Vec<&str> = t.splitn(3, ':').collect();
            if parts.len() == 3 {
                groups
                    .entry(format!("{}:{}", parts[0], parts[1]))
                    .or_default()
                    .push(parts[2].to_string());
            }
        }
        groups
            .into_iter()
            .map(|(k, v)| json!({ "type": k, "codes": v }))
            .collect()
    }

    fn set_status(&self, state: &str, detail: &str) {
        let st = if detail.is_empty() {
            json!({ "state": state })
        } else {
            json!({ "state": state, "detail": detail })
        };
        let subs: Vec<Arc<Subscriber>> = {
            let mut g = self.inner.lock();
            g.status = st.clone();
            g.subs.values().cloned().collect()
        };
        for s in subs {
            s.push_status(st.clone());
        }
    }

    fn dispatch(&self, topic: &str, data: Value) {
        let symbol = topic.rsplit(':').next().unwrap_or("");
        let mut payload = normalize_ws_nums(data);
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("topic".to_string(), json!(topic));
            obj.insert("symbol".to_string(), json!(symbol));
        }
        let subs: Vec<Arc<Subscriber>> = {
            let g = self.inner.lock();
            g.subs
                .values()
                .filter(|s| s.topics.contains(topic))
                .cloned()
                .collect()
        };
        for s in subs {
            s.push(topic, payload.clone());
        }
    }

    fn idle_expired(&self) -> bool {
        if self.stopped.load(Ordering::SeqCst) {
            return true;
        }
        let g = self.inner.lock();
        g.empty_since
            .map(|t| t.elapsed() > IDLE_STOP)
            .unwrap_or(false)
    }

    fn kiwoom_symbols(&self, market_code: &str) -> Vec<String> {
        let mut syms: Vec<String> = {
            let g = self.inner.lock();
            g.subs
                .values()
                .flat_map(|s| s.topics.iter())
                .filter_map(|t| {
                    let parts: Vec<&str> = t.splitn(3, ':').collect();
                    if parts.len() == 3
                        && parts[1] == market_code
                        && matches!(parts[0], "trade" | "orderbook")
                    {
                        Some(parts[2].to_string())
                    } else {
                        None
                    }
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        syms.sort();
        syms.truncate(MAX_CODES);
        syms
    }

    async fn run(self: Arc<Self>, hub: Arc<Hub>, state: AppState) {
        if credentials::active_broker(&state, &self.uid) == BrokerKind::Kiwoom {
            self.run_kiwoom(hub, state).await;
        } else {
            self.run_toss(hub, state).await;
        }
    }

    async fn run_kiwoom(self: Arc<Self>, hub: Arc<Hub>, state: AppState) {
        let mut backoff = Duration::from_secs(1);
        let mut rejected: Option<String> = None;

        while !self.idle_expired() {
            let conn_info = async {
                let client = state.get_kiwoom_client(&self.uid)?;
                let token = client.access_token(rejected.as_deref()).await?;
                let url = if self.kiwoom_us {
                    client.us_ws_url().to_string()
                } else {
                    client.ws_url().to_string()
                };
                Ok::<(Arc<crate::integrations::kiwoom::KiwoomClient>, String, String), crate::errors::ApiError>((client, url, token))
            }
            .await;

            let (client, ws_url, token) = match conn_info {
                Ok(triple) => {
                    rejected = None;
                    triple
                }
                Err(e) => {
                    self.set_status("reconnecting", &e.message);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };

            match tokio_tungstenite::connect_async(&ws_url).await {
                Ok((ws_stream, _)) => {
                    let (mut write, mut read) = ws_stream.split();
                    let login = json!({ "trnm": "LOGIN", "token": token }).to_string();
                    if write.send(Message::Text(login.into())).await.is_err() {
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(30));
                        continue;
                    }
                    let mut logged_in = false;
                    let mut sent_syms: Option<Vec<String>> = None;
                    let mut last_reg = Instant::now() - Duration::from_secs(1);
                    let mut tick = tokio::time::interval(Duration::from_millis(250));

                    loop {
                        if self.idle_expired() {
                            let _ = write.close().await;
                            break;
                        }
                        if logged_in {
                            let syms = self.kiwoom_symbols(if self.kiwoom_us { "us" } else { "kr" });
                            if sent_syms.as_ref() != Some(&syms)
                                && last_reg.elapsed() >= Duration::from_millis(250)
                            {
                                let reg = if self.kiwoom_us {
                                    let mut pairs = Vec::with_capacity(syms.len());
                                    for s in &syms {
                                        let exch = client.resolve_us_exch(s).await;
                                        pairs.push((s.clone(), exch));
                                    }
                                    build_kiwoom_us_reg_frame(&pairs)
                                } else {
                                    build_kiwoom_reg_frame(&syms)
                                };
                                if write.send(Message::Text(reg.to_string().into())).await.is_err() {
                                    break;
                                }
                                sent_syms = Some(syms);
                                last_reg = Instant::now();
                            }
                        }

                        tokio::select! {
                            _ = tick.tick() => {}
                            _ = self.decl_notify.notified() => {}
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(txt))) => {
                                        if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                            match parsed.get("trnm").and_then(|v| v.as_str()) {
                                                Some("PING") => {
                                                    if write.send(Message::Text(txt)).await.is_err() {
                                                        break;
                                                    }
                                                }
                                                Some("LOGIN") => {
                                                    let rc = parsed
                                                        .get("return_code")
                                                        .and_then(|v| v.as_i64())
                                                        .unwrap_or(0);
                                                    if rc == 0 {
                                                        logged_in = true;
                                                        backoff = Duration::from_secs(1);
                                                        self.set_status("connected", "");
                                                    } else {
                                                        let msg = parsed
                                                            .get("return_msg")
                                                            .and_then(|v| v.as_str())
                                                            .unwrap_or("키움 웹소켓 인증 실패");
                                                        self.set_status("reconnecting", msg);
                                                        rejected = Some(token.clone());
                                                        break;
                                                    }
                                                }
                                                Some("REAL") => {
                                                    for (topic, payload) in parse_kiwoom_real_frame(&parsed) {
                                                        self.dispatch(&topic, payload);
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Ping(p))) => {
                                        let _ = write.send(Message::Pong(p)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg: String = e.to_string().chars().take(200).collect();
                    self.set_status("reconnecting", &msg);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }

        hub.cleanup(&self.stream_key, &self);
    }

    async fn run_toss(self: Arc<Self>, hub: Arc<Hub>, state: AppState) {
        let mut backoff = Duration::from_secs(1);
        let mut rejected: Option<String> = None;
        let mut seq: u64 = 0;

        while !self.idle_expired() {
            let token_res = async {
                let client = state.get_toss_client(&self.uid)?;
                client.access_token(rejected.as_deref()).await
            }
            .await;

            let token = match token_res {
                Ok(t) => {
                    rejected = None;
                    t
                }
                Err(e) => {
                    self.set_status("reconnecting", &e.message);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };

            let req = match WS_URL.into_client_request() {
                Ok(mut r) => {
                    if let Ok(hv) = header::HeaderValue::from_str(&format!("Bearer {token}")) {
                        r.headers_mut().insert(header::AUTHORIZATION, hv);
                    }
                    r
                }
                Err(e) => {
                    self.set_status("reconnecting", &e.to_string());
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            };

            match tokio_tungstenite::connect_async(req).await {
                Ok((ws_stream, _)) => {
                    self.set_status("connected", "");
                    backoff = Duration::from_secs(1);
                    let (mut write, mut read) = ws_stream.split();
                    let mut sent: Option<Vec<Value>> = None;
                    let mut last_ping = Instant::now();
                    let mut last_decl = Instant::now() - Duration::from_secs(1);
                    let mut tick = tokio::time::interval(Duration::from_millis(250));

                    loop {
                        if self.idle_expired() {
                            let _ = write.close().await;
                            break;
                        }
                        let decl = self.declaration();
                        if sent.as_ref() != Some(&decl) && last_decl.elapsed() >= Duration::from_millis(250) {
                            seq += 1;
                            let mut frames = Vec::with_capacity(decl.len() + 1);
                            frames.push(json!({ "id": format!("mt-{seq}") }));
                            frames.extend(decl.iter().cloned());
                            if write
                                .send(Message::Text(Value::Array(frames).to_string().into()))
                                .await
                                .is_err()
                            {
                                break;
                            }
                            sent = Some(decl);
                            last_decl = Instant::now();
                        }
                        if last_ping.elapsed() > PING_EVERY {
                            if write.send(Message::Text("PING".into())).await.is_err() {
                                break;
                            }
                            last_ping = Instant::now();
                        }

                        tokio::select! {
                            _ = tick.tick() => {}
                            _ = self.decl_notify.notified() => {}
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(txt))) => {
                                        if let Ok(parsed) = serde_json::from_str::<Value>(&txt) {
                                            match parsed.get("type").and_then(|v| v.as_str()) {
                                                Some("message") => {
                                                    if let (Some(topic), Some(data)) = (
                                                        parsed.get("topic").and_then(|v| v.as_str()),
                                                        parsed.get("data").filter(|v| v.is_object()),
                                                    ) {
                                                        self.dispatch(topic, data.clone());
                                                    }
                                                }
                                                Some("error") => {
                                                    let code = parsed
                                                        .get("error")
                                                        .and_then(|e| e.get("code"))
                                                        .and_then(|v| v.as_str())
                                                        .unwrap_or("");
                                                    if TOKEN_ERRORS.contains(&code) {
                                                        rejected = Some(token.clone());
                                                        break;
                                                    }
                                                    if code == "rate-limit-exceeded" {
                                                        tokio::time::sleep(Duration::from_secs(1)).await;
                                                        sent = None;
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                    Some(Ok(Message::Ping(p))) => {
                                        let _ = write.send(Message::Pong(p)).await;
                                    }
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let msg: String = e.to_string().chars().take(200).collect();
                    self.set_status("reconnecting", &msg);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }

        hub.cleanup(&self.stream_key, &self);
    }
}

pub struct Hub {
    streams: DashMap<String, Arc<UserStream>>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

impl Hub {
    pub fn new() -> Self {
        Self {
            streams: DashMap::new(),
        }
    }

    pub fn subscribe(
        self: &Arc<Self>,
        state: &AppState,
        uid: &str,
        topics: HashSet<String>,
    ) -> Arc<Subscriber> {
        let is_kiwoom = credentials::active_broker(state, uid) == BrokerKind::Kiwoom;
        let is_us_topic = topics.iter().any(|t| t.contains(":us:"));
        let (stream_key, kiwoom_us) = if is_kiwoom {
            if is_us_topic {
                (format!("{uid}:us"), true)
            } else {
                (format!("{uid}:kr"), false)
            }
        } else {
            (uid.to_string(), false)
        };
        let sub = Arc::new(Subscriber::new(topics));
        if let Some(stream) = self.streams.get(&stream_key) {
            stream.add(sub.clone());
            return sub;
        }
        let stream = Arc::new(UserStream::new(stream_key.clone(), uid.to_string(), kiwoom_us));
        stream.add(sub.clone());
        self.streams.insert(stream_key, stream.clone());
        let hub_clone = Arc::clone(self);
        let state_clone = state.clone();
        tokio::spawn(async move {
            stream.run(hub_clone, state_clone).await;
        });
        sub
    }

    pub fn unsubscribe(&self, uid: &str, sub_id: u64) {
        for key in [uid.to_string(), format!("{uid}:kr"), format!("{uid}:us")] {
            if let Some(stream) = self.streams.get(&key) {
                stream.remove(sub_id);
            }
        }
    }

    fn cleanup(&self, key: &str, stopped: &Arc<UserStream>) {
        if let Some(entry) = self.streams.get(key) {
            if Arc::ptr_eq(entry.value(), stopped) {
                drop(entry);
                self.streams.remove(key);
            }
        }
    }

    pub fn active_users(&self) -> usize {
        self.streams.len()
    }

    pub fn disconnect_user(&self, uid: &str) {
        for key in [uid.to_string(), format!("{uid}:kr"), format!("{uid}:us")] {
            if let Some((_, stream)) = self.streams.remove(&key) {
                stream.force_stop();
            }
        }
    }
}

pub fn build_kiwoom_reg_frame(syms: &[String]) -> Value {
    if syms.is_empty() {
        return json!({
            "trnm": "REMOVE",
            "grp_no": "1",
            "data": [{ "item": [""], "type": ["0B", "0D"] }],
        });
    }
    json!({
        "trnm": "REG",
        "grp_no": "1",
        "refresh": "0",
        "data": [{ "item": syms, "type": ["0B", "0D"] }],
    })
}

pub fn build_kiwoom_us_reg_frame(items: &[(String, String)]) -> Value {
    if items.is_empty() {
        return json!({
            "trnm": "REMOVE",
            "grp_no": "1",
            "data": [{ "item": [], "type": ["FE", "FT"] }],
        });
    }
    let mapped: Vec<Value> = items
        .iter()
        .map(|(sym, exch)| json!({ "jmcode": sym, "stex_tp": exch }))
        .collect();
    json!({
        "trnm": "REG",
        "grp_no": "1",
        "refresh": "0",
        "data": [{ "item": mapped, "type": ["FE", "FT"] }],
    })
}

pub fn parse_kiwoom_real_frame(parsed: &Value) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    // Kiwoom REAL frames can be either a single object or carry a "data" array of entries.
    if let Some(arr) = parsed.get("data").and_then(|v| v.as_array()) {
        for entry in arr {
            if let Some(ev) = parse_kiwoom_real_entry(entry) {
                out.push(ev);
            }
        }
        return out;
    }
    if let Some(ev) = parse_kiwoom_real_entry(parsed) {
        out.push(ev);
    }
    out
}

fn parse_kiwoom_real_entry(entry: &Value) -> Option<(String, Value)> {
    let rtype = entry.get("type").and_then(|v| v.as_str())?;
    let raw_item = entry.get("item").and_then(|v| v.as_str())?;
    let sym = strip_kr_prefix(raw_item);
    if sym.is_empty() {
        return None;
    }
    let vals = entry.get("values")?.as_object()?;

    match rtype {
        "0B" | "FE" => {
            let mkt = if rtype == "FE" { "us" } else { "kr" };
            let price = parse_kiwoom_abs(vals.get("10"))?;
            let raw_vol = parse_kiwoom_signed(vals.get("15")).unwrap_or(0.0);
            let side = if raw_vol < 0.0 { "SELL" } else { "BUY" };
            let ts = vals.get("20").and_then(|v| v.as_str()).unwrap_or("");
            Some((
                format!("trade:{mkt}:{sym}"),
                json!({
                    "price": price,
                    "volume": raw_vol.abs(),
                    "side": side,
                    "timestamp": ts,
                }),
            ))
        }
        "0D" | "FT" => {
            let mkt = if rtype == "FT" { "us" } else { "kr" };
            let mut asks = Vec::new();
            let mut bids = Vec::new();
            for i in 0..10 {
                let ap_k = (41 + i).to_string();
                let aq_k = (61 + i).to_string();
                if let Some(px) = parse_kiwoom_abs(vals.get(&ap_k)) {
                    if px > 0.0 {
                        let q = parse_kiwoom_abs(vals.get(&aq_k)).unwrap_or(0.0);
                        asks.push(json!({ "price": px, "volume": q }));
                    }
                }
                let bp_k = (51 + i).to_string();
                let bq_k = (71 + i).to_string();
                if let Some(px) = parse_kiwoom_abs(vals.get(&bp_k)) {
                    if px > 0.0 {
                        let q = parse_kiwoom_abs(vals.get(&bq_k)).unwrap_or(0.0);
                        bids.push(json!({ "price": px, "volume": q }));
                    }
                }
            }
            let tot_ask = parse_kiwoom_abs(vals.get("121")).unwrap_or_else(|| {
                asks.iter().filter_map(|x| x["volume"].as_f64()).sum()
            });
            let tot_bid = parse_kiwoom_abs(vals.get("125")).unwrap_or_else(|| {
                bids.iter().filter_map(|x| x["volume"].as_f64()).sum()
            });
            Some((
                format!("orderbook:{mkt}:{sym}"),
                json!({
                    "asks": asks,
                    "bids": bids,
                    "totalAskVolume": tot_ask,
                    "totalBidVolume": tot_bid,
                }),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod kiwoom_ws_tests {
    use super::*;

    #[test]
    fn parses_kiwoom_0b_trade_and_0d_orderbook_frames() {
        let trade_frame = json!({
            "trnm": "REAL",
            "type": "0B",
            "item": "A005930",
            "values": { "10": "+71200", "15": "-42", "20": "101530" }
        });
        let evs = parse_kiwoom_real_frame(&trade_frame);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "trade:kr:005930");
        assert_eq!(evs[0].1["price"], json!(71200.0));
        assert_eq!(evs[0].1["volume"], json!(42.0));
        assert_eq!(evs[0].1["side"], json!("SELL"));

        let book_frame = json!({
            "trnm": "REAL",
            "data": [{
                "type": "0D",
                "item": "005930",
                "values": {
                    "41": "71300", "61": "1200",
                    "42": "71400", "62": "800",
                    "51": "71200", "71": "1500",
                    "52": "71100", "72": "900",
                    "121": "2000", "125": "2400"
                }
            }]
        });
        let books = parse_kiwoom_real_frame(&book_frame);
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].0, "orderbook:kr:005930");
        assert_eq!(books[0].1["asks"].as_array().unwrap().len(), 2);
        assert_eq!(books[0].1["bids"].as_array().unwrap().len(), 2);
        assert_eq!(books[0].1["totalAskVolume"], json!(2000.0));
        assert_eq!(books[0].1["totalBidVolume"], json!(2400.0));

        // US stock FE (trade) and FT (10-level orderbook)
        let us_trade = json!({
            "trnm": "REAL",
            "data": [{
                "type": "FE",
                "item": "NVDA",
                "values": { "10": "+198.5000", "15": "+15", "20": "215300" }
            }]
        });
        let ut = parse_kiwoom_real_frame(&us_trade);
        assert_eq!(ut.len(), 1);
        assert_eq!(ut[0].0, "trade:us:NVDA");
        assert_eq!(ut[0].1["price"], json!(198.5));
        assert_eq!(ut[0].1["side"], json!("BUY"));

        let us_book = json!({
            "trnm": "REAL",
            "data": [{
                "type": "FT",
                "item": "NVDA",
                "values": { "41": "+198.5400", "61": "20", "51": "+198.4800", "71": "35", "121": "590", "125": "437" }
            }]
        });
        let ub = parse_kiwoom_real_frame(&us_book);
        assert_eq!(ub.len(), 1);
        assert_eq!(ub[0].0, "orderbook:us:NVDA");
        assert_eq!(ub[0].1["asks"][0]["price"], json!(198.54));
        assert_eq!(ub[0].1["bids"][0]["price"], json!(198.48));
    }

    #[test]
    fn builds_kiwoom_reg_and_remove_frames() {
        let reg = build_kiwoom_reg_frame(&["005930".to_string(), "000660".to_string()]);
        assert_eq!(reg["trnm"], json!("REG"));
        assert_eq!(reg["data"][0]["item"], json!(["005930", "000660"]));
        let rem = build_kiwoom_reg_frame(&[]);
        assert_eq!(rem["trnm"], json!("REMOVE"));
        let us_reg = build_kiwoom_us_reg_frame(&[("NVDA".to_string(), "ND".to_string())]);
        assert_eq!(us_reg["data"][0]["type"], json!(["FE", "FT"]));
        assert_eq!(us_reg["data"][0]["item"][0]["jmcode"], json!("NVDA"));
    }
}
