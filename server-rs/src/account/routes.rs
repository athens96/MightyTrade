use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::toss_account;
use crate::auth::extractors::AuthUser;
use crate::auth::otp;
use crate::errors::{ApiError, ApiResult};
use crate::market::models::Market;
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::users::repository::now_iso;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/summary", get(summary))
        .route("/conditional-orders", get(list_conditional))
        .route("/conditional-orders/oco", post(create_oco))
        .route("/conditional-orders/{order_id}", delete(cancel_conditional))
}

#[derive(Deserialize)]
struct StatusQuery {
    status: Option<String>,
}

async fn summary(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(toss_account::summary(&state, uid).await?))
}

async fn list_conditional(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    let st = q.status.unwrap_or_else(|| "OPEN".to_string()).to_uppercase();
    if st != "OPEN" && st != "CLOSED" {
        return Err(ApiError::code_only(400, "invalid_status"));
    }
    let uid = user["id"].as_str().unwrap_or("");
    Ok(Json(json!({
        "items": toss_account::conditional_orders(&state, uid, &st).await?,
    })))
}

async fn create_oco(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("").to_string();
    let built = toss_account::build_oco(&state, &uid, &body).await?;
    if body.get("dry_run").and_then(|v| v.as_bool()) != Some(false) {
        return Ok(Json(json!({
            "dry_run": true,
            "payload": built["payload"],
            "checks": built["checks"],
        })));
    }

    let otp_code = body.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
    otp::verify(&state, &user, otp_code)?;
    let market = Market::parse(built["market"].as_str().unwrap_or(""))?;
    let order_value = built["checks"]["order_value"].as_f64().unwrap_or(0.0);
    trading::check_limits(&state, &uid, market, order_value)?;
    let result = toss_account::submit_oco(&state, &uid, &built).await?;
    let p = &built["payload"];
    let cid = p["clientOrderId"].as_str().unwrap_or("");
    state.store.put(
        "orders",
        cid,
        json!({
            "user_id": uid,
            "kind": "conditional_oco",
            "source": "manual_oco",
            "market": built["market"],
            "symbol": p["symbol"],
            "side": "SELL",
            "payload": p,
            "result": result,
            "date": trading::kst_today(),
            "created_at": now_iso(),
        }),
    );
    Ok(Json(json!({
        "dry_run": false,
        "payload": p,
        "checks": built["checks"],
        "result": result,
    })))
}

async fn cancel_conditional(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(order_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    toss_account::cancel_conditional(&state, uid, &order_id).await?;
    Ok(Json(json!({ "ok": true })))
}
