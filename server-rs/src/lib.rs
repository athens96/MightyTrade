pub mod account;
pub mod admin;
pub mod auth;
pub mod autotrade;
pub mod config;
pub mod crypto;
pub mod errors;
pub mod integrations;
pub mod market;
pub mod quant;
pub mod realtime;
pub mod state;
pub mod storage;
pub mod trading;
pub mod transport_crypto;
pub mod users;
pub mod version;

use axum::extract::State;
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .nest("/api/auth", auth::routes::router())
        .nest("/api/users", users::routes::router())
        .nest("/api/admin", admin::routes::router())
        .nest("/api/market", market::routes::router())
        .nest("/api/quant", quant::routes::router())
        .nest("/api/account", account::routes::router())
        .nest("/api/trading", trading::routes::router())
        .nest("/api/realtime", realtime::routes::router())
        .nest("/api/autotrade", autotrade::routes::router())
        .nest("/api/kiwoom", integrations::kiwoom_routes::router())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Response {
    let _ = state.store.get("health_probe", "ping");
    let mut resp = Json(json!({
        "status": "ok",
        "version": state.app_version,
        "db": "ok",
    }))
    .into_response();
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    resp
}
