use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::auth::extractors::AuthUser;
use crate::auth::otp;
use crate::errors::{ApiError, ApiResult};
use crate::integrations::toss::verify_credentials;
use crate::state::AppState;
use crate::users::{credentials, repository::now_iso, requests_repo};

const WATCHLISTS: &str = "watchlists";
const SETTINGS: &str = "settings";

fn default_watchlist() -> Value {
    json!([
        {"market": "KR", "symbol": "005930", "name": "삼성전자"},
        {"market": "KR", "symbol": "000660", "name": "SK하이닉스"},
        {"market": "US", "symbol": "AAPL", "name": "Apple"},
        {"market": "US", "symbol": "NVDA", "name": "NVIDIA"},
    ])
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/watchlist", get(get_watchlist).put(put_watchlist))
        .route("/me/settings", get(get_settings).put(put_settings))
        .route("/transport-key", get(transport_key))
        .route(
            "/me/broker/toss",
            get(get_toss_credentials)
                .put(put_toss_credentials)
                .delete(delete_toss_credentials),
        )
        .route(
            "/me/notifications",
            get(get_notifications)
                .put(put_notifications)
                .delete(delete_notifications),
        )
        .route("/me/notifications/test", post(test_notifications))
        .route(
            "/me/requests",
            get(list_my_requests).post(create_my_request),
        )
}

async fn get_watchlist(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    let items = state
        .store
        .get(WATCHLISTS, uid)
        .and_then(|r| r.get("items").cloned())
        .unwrap_or_else(default_watchlist);
    Json(json!({ "items": items }))
}

async fn put_watchlist(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let items = body
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::code_only(400, "items_required"))?;
    let mut clean = Vec::with_capacity(items.len());
    for it in items {
        let market = it
            .get("market")
            .and_then(|v| v.as_str())
            .filter(|m| matches!(*m, "KR" | "US"))
            .ok_or_else(|| ApiError::bad_request("invalid_item", format!("invalid watchlist item: {it}")))?;
        let symbol = it
            .get("symbol")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::bad_request("invalid_item", format!("invalid watchlist item: {it}")))?;
        let name = it.get("name").and_then(|v| v.as_str()).unwrap_or("");
        clean.push(json!({ "market": market, "symbol": symbol, "name": name }));
    }
    let uid = user["id"].as_str().unwrap_or("");
    let saved = state
        .store
        .put(WATCHLISTS, uid, json!({ "items": clean, "updated_at": now_iso() }));
    Ok(Json(json!({ "items": saved["items"] })))
}

async fn get_settings(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    let settings = state
        .store
        .get(SETTINGS, uid)
        .and_then(|r| r.get("values").cloned())
        .unwrap_or_else(|| json!({}));
    Json(json!({ "settings": settings }))
}

async fn put_settings(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let values = body
        .get("settings")
        .filter(|v| v.is_object())
        .cloned()
        .ok_or_else(|| ApiError::code_only(400, "settings_required"))?;
    let uid = user["id"].as_str().unwrap_or("");
    let saved = state
        .store
        .put(SETTINGS, uid, json!({ "values": values, "updated_at": now_iso() }));
    Ok(Json(json!({ "settings": saved["values"] })))
}

async fn transport_key(
    State(state): State<AppState>,
    AuthUser(_): AuthUser,
) -> ApiResult<Json<Value>> {
    let info = state
        .transport
        .public_key_info(state.store.as_ref(), &state.crypto)?;
    Ok(Json(info))
}

fn is_public_ipv4(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified())
}

fn extract_host_for_ip(raw_host: &str) -> Option<String> {
    let trimmed = raw_host.trim().to_lowercase();
    if trimmed.is_empty() {
        return None;
    }
    let host = if trimmed.starts_with('[') {
        trimmed
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or("")
            .to_string()
    } else {
        trimmed.split(':').next().unwrap_or("").to_string()
    };
    if host.is_empty()
        || host == "localhost"
        || host == "127.0.0.1"
        || host == "::1"
        || host == "0.0.0.0"
        || host.ends_with(".local")
        || host.ends_with(".internal")
    {
        return None;
    }
    Some(host)
}

async fn resolve_server_public_ip(state: &AppState, headers: &HeaderMap) -> Option<String> {
    if let Ok(explicit) = std::env::var("SERVER_PUBLIC_IP") {
        let s = explicit.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    if state.config.testing {
        return None;
    }
    let raw_host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(axum::http::header::HOST))
        .and_then(|v| v.to_str().ok())?;
    let host = extract_host_for_ip(raw_host)?;
    if let Ok(IpAddr::V4(v4)) = host.parse::<IpAddr>() {
        return is_public_ipv4(v4).then(|| v4.to_string());
    }
    let lookup = tokio::time::timeout(
        Duration::from_millis(400),
        tokio::net::lookup_host((host.as_str(), 443)),
    )
    .await
    .ok()?
    .ok()?;
    for addr in lookup {
        if let IpAddr::V4(v4) = addr.ip() {
            if is_public_ipv4(v4) {
                return Some(v4.to_string());
            }
        }
    }
    None
}

async fn toss_status_view(state: &AppState, uid: &str, headers: &HeaderMap) -> Value {
    let mut view = credentials::public_view(state, uid);
    let ip = resolve_server_public_ip(state, headers).await;
    if let Some(obj) = view.as_object_mut() {
        obj.insert(
            "server_ip".to_string(),
            ip.map(Value::String).unwrap_or(Value::Null),
        );
    }
    view
}

async fn get_toss_credentials(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    Json(toss_status_view(&state, uid, &headers).await)
}

async fn put_toss_credentials(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let payload = state.transport.open_envelope(
        state.store.as_ref(),
        &state.crypto,
        body.get("envelope"),
        uid,
    )?;

    let otp_code = payload.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
    otp::verify(&state, &user, otp_code)?;

    let client_id = payload
        .get("client_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let client_secret = payload
        .get("client_secret")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if client_id.is_empty()
        || client_id.len() > 200
        || client_secret.is_empty()
        || client_secret.len() > 500
    {
        return Err(ApiError::bad_request(
            "invalid_credentials",
            "client_id / client_secret을 입력하세요",
        ));
    }

    let previous = credentials::get_toss(&state, uid);
    let account_seq = verify_credentials(&state, client_id, client_secret).await?;
    credentials::save_toss(&state, uid, client_id, client_secret, account_seq);
    let old_cid = previous
        .as_ref()
        .filter(|p| p.client_id != client_id)
        .map(|p| p.client_id.as_str());
    state.forget_toss_client(uid, old_cid);
    Ok(Json(toss_status_view(&state, uid, &headers).await))
}

async fn delete_toss_credentials(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    headers: HeaderMap,
) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    let previous = credentials::get_toss(&state, uid);
    credentials::delete_toss(&state, uid);
    state.forget_toss_client(uid, previous.as_ref().map(|p| p.client_id.as_str()));
    Json(toss_status_view(&state, uid, &headers).await)
}

async fn get_notifications(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    Json(credentials::public_notifications_view(&state, uid))
}

async fn put_notifications(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    Ok(Json(credentials::save_notifications(&state, uid, &body)?))
}

async fn delete_notifications(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    Json(credentials::delete_notifications(&state, uid))
}

async fn test_notifications(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(credentials::send_test_notification(&state, uid).await?))
}

async fn list_my_requests(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    Json(json!({ "items": requests_repo::list_for_user(&state, uid) }))
}

async fn create_my_request(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let item = requests_repo::create_request(&state, &user, &body).await?;
    Ok((StatusCode::CREATED, Json(json!({ "item": item }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_extraction_filters_local_and_strips_port() {
        assert_eq!(extract_host_for_ip("localhost:5173"), None);
        assert_eq!(extract_host_for_ip("127.0.0.1:5001"), None);
        assert_eq!(extract_host_for_ip("[::1]:5001"), None);
        assert_eq!(
            extract_host_for_ip("MightyTrade.DuckDNS.org:443"),
            Some("mightytrade.duckdns.org".to_string())
        );
    }

    #[test]
    fn public_ipv4_check_blocks_private_and_loopback() {
        assert!(!is_public_ipv4(Ipv4Addr::new(127, 0, 0, 1)));
        assert!(!is_public_ipv4(Ipv4Addr::new(10, 0, 0, 5)));
        assert!(!is_public_ipv4(Ipv4Addr::new(192, 168, 1, 10)));
        assert!(is_public_ipv4(Ipv4Addr::new(1, 1, 1, 1)));
    }
}
