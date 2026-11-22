//! Async Toss WebSocket -> Browser SSE relay (1 Toss WS per user, 5Hz orderbook coalescing).

use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;
use tokio_tungstenite::tungstenite::Message;

use crate::state::AppState;

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
    uid: String,
    inner: Mutex<StreamInner>,
    decl_notify: Notify,
}

impl UserStream {
    fn new(uid: String) -> Self {
        Self {
            uid,
            inner: Mutex::new(StreamInner {
                subs: HashMap::new(),
                empty_since: None,
                status: json!({ "state": "connecting" }),
            }),
            decl_notify: Notify::new(),
        }
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
        let g = self.inner.lock();
        g.empty_since
            .map(|t| t.elapsed() > IDLE_STOP)
            .unwrap_or(false)
    }

    async fn run(self: Arc<Self>, hub: Arc<Hub>, state: AppState) {
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

        hub.cleanup(&self.uid, &self);
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
        let sub = Arc::new(Subscriber::new(topics));
        if let Some(stream) = self.streams.get(uid) {
            stream.add(sub.clone());
            return sub;
        }
        let stream = Arc::new(UserStream::new(uid.to_string()));
        stream.add(sub.clone());
        self.streams.insert(uid.to_string(), stream.clone());
        let hub_clone = Arc::clone(self);
        let state_clone = state.clone();
        tokio::spawn(async move {
            stream.run(hub_clone, state_clone).await;
        });
        sub
    }

    pub fn unsubscribe(&self, uid: &str, sub_id: u64) {
        if let Some(stream) = self.streams.get(uid) {
            stream.remove(sub_id);
        }
    }

    fn cleanup(&self, uid: &str, stopped: &Arc<UserStream>) {
        if let Some(entry) = self.streams.get(uid) {
            if Arc::ptr_eq(entry.value(), stopped) {
                drop(entry);
                self.streams.remove(uid);
            }
        }
    }

    pub fn active_users(&self) -> usize {
        self.streams.len()
    }
}
