use axum::extract::{Query, State};
use axum::http::{header, HeaderValue};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use super::hub::{Hub, Subscriber, FLUSH_INTERVAL, KEEPALIVE_INTERVAL};
use crate::auth::extractors::AuthUser;
use crate::errors::{ApiError, ApiResult};
use crate::market::models::Market;
use crate::state::AppState;
use crate::users::credentials;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/stream", get(stream))
        .route("/orders", get(order_stream))
}

#[derive(Deserialize)]
struct StreamQuery {
    market: Option<String>,
    symbol: Option<String>,
}

struct SubDropGuard {
    hub: Arc<Hub>,
    uid: String,
    sub_id: u64,
}

impl Drop for SubDropGuard {
    fn drop(&mut self) {
        self.hub.unsubscribe(&self.uid, self.sub_id);
    }
}

struct SseState {
    sub: Arc<Subscriber>,
    _guard: SubDropGuard,
    pending: VecDeque<Event>,
    pause_before_drain: bool,
}

fn build_sse_response(
    hub: Arc<Hub>,
    uid: String,
    sub: Arc<Subscriber>,
    retry_ms: u64,
) -> Response {
    let sub_id = sub.id;
    let mut pending = VecDeque::new();
    pending.push_back(Event::default().retry(Duration::from_millis(retry_ms)));
    let init = SseState {
        sub,
        _guard: SubDropGuard { hub, uid, sub_id },
        pending,
        pause_before_drain: false,
    };

    let event_stream = futures_util::stream::unfold(init, |mut st| async move {
        if let Some(ev) = st.pending.pop_front() {
            return Some((Ok::<Event, Infallible>(ev), st));
        }
        if st.pause_before_drain {
            tokio::time::sleep(FLUSH_INTERVAL).await;
            st.pause_before_drain = false;
        }
        loop {
            let drained = st.sub.drain();
            if !drained.is_empty() {
                for (name, data) in drained {
                    st.pending
                        .push_back(Event::default().event(name).data(data.to_string()));
                }
                st.pause_before_drain = true;
                let ev = st.pending.pop_front().unwrap();
                return Some((Ok::<Event, Infallible>(ev), st));
            }
            if !st.sub.wait_or_timeout(KEEPALIVE_INTERVAL).await {
                return Some((Ok::<Event, Infallible>(Event::default().comment("keepalive")), st));
            }
        }
    });

    let mut resp = Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(KEEPALIVE_INTERVAL).text("keepalive"))
        .into_response();
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    resp.headers_mut()
        .insert("x-accel-buffering", HeaderValue::from_static("no"));
    resp
}

async fn stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<StreamQuery>,
) -> ApiResult<Response> {
    let market = Market::parse(q.market.as_deref().unwrap_or(""))?;
    let symbol = q
        .symbol
        .unwrap_or_default()
        .trim()
        .to_uppercase();
    if !market.validate_symbol(&symbol) {
        return Err(ApiError::code_only(400, "invalid_symbol"));
    }
    let prov = state
        .config
        .market_providers
        .get(market.as_str())
        .map(|s| s.as_str())
        .unwrap_or("mock");
    if !matches!(prov, "toss" | "kiwoom" | "broker") {
        return Err(ApiError::not_implemented(
            "requires_toss",
            "실시간 시세는 증권사 연동에서만 제공됩니다",
        ));
    }
    let uid = user["id"].as_str().unwrap_or("").to_string();
    if !credentials::has_active_broker(&state, &uid) {
        return Err(ApiError::conflict(
            "toss_credentials_required",
            "증권사 API 키(토스증권 또는 키움증권)를 먼저 등록하세요 (설정)",
        ));
    }
    let m = market.as_str().to_lowercase();
    let mut topics = HashSet::new();
    topics.insert(format!("trade:{m}:{symbol}"));
    topics.insert(format!("orderbook:{m}:{symbol}"));
    let sub = state.realtime_hub.subscribe(&state, &uid, topics);
    Ok(build_sse_response(state.realtime_hub.clone(), uid, sub, 3000))
}

async fn order_stream(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Response> {
    let has_live = state
        .config
        .market_providers
        .values()
        .any(|v| matches!(v.as_str(), "toss" | "kiwoom" | "broker"));
    if !has_live {
        return Err(ApiError::not_implemented(
            "requires_toss",
            "증권사 연동에서만 제공됩니다",
        ));
    }
    let uid = user["id"].as_str().unwrap_or("").to_string();
    let mut topics = HashSet::new();
    match credentials::active_broker(&state, &uid) {
        credentials::BrokerKind::Kiwoom => {
            if credentials::get_kiwoom(&state, &uid).is_none() {
                return Err(ApiError::conflict(
                    "kiwoom_credentials_required",
                    "키움증권 REST API 키를 먼저 등록하세요 (설정)",
                ));
            }
            topics.insert("personal:order:kiwoom".to_string());
        }
        credentials::BrokerKind::Toss => {
            let creds = credentials::get_toss(&state, &uid);
            let Some(account_seq) = creds.and_then(|c| c.account_seq) else {
                return Err(ApiError::conflict(
                    "toss_credentials_required",
                    "토스증권 API 키를 먼저 등록하세요 (설정)",
                ));
            };
            topics.insert(format!("personal:order:{account_seq}"));
        }
    }
    let sub = state.realtime_hub.subscribe(&state, &uid, topics);
    Ok(build_sse_response(state.realtime_hub.clone(), uid, sub, 5000))
}
