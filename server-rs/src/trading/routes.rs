use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::orders;
use crate::auth::extractors::AuthUser;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::users::repository as users;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/unlock", post(unlock))
        .route("/lock", post(lock))
        .route("/orders", get(list_orders).post(place_order))
        .route("/orders/{*order_rest}", get(get_order_wildcard).post(post_order_wildcard))
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
    symbol: Option<String>,
}

fn fresh_user(state: &AppState, user: Value) -> Value {
    let uid = user["id"].as_str().unwrap_or("");
    users::get_user(state, uid).unwrap_or(user)
}

async fn status(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let u = fresh_user(&state, user);
    Json(orders::trading_status(&state, &u))
}

async fn unlock(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let u = fresh_user(&state, user);
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let code = body.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
    Ok(Json(orders::unlock(&state, &u, code)?))
}

async fn lock(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> ApiResult<Json<Value>> {
    let u = fresh_user(&state, user);
    Ok(Json(orders::lock(&state, &u)?))
}

async fn place_order(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let uid = user["id"].as_str().unwrap_or("").to_string();
    let built = orders::build_order(&state, &uid, &body).await?;
    let dry_run = body.get("dry_run").and_then(|v| v.as_bool()) != Some(false);
    let mut out = built.clone();
    if dry_run {
        out.as_object_mut().unwrap().insert("dry_run".to_string(), json!(true));
        return Ok(Json(out));
    }
    let u = fresh_user(&state, user);
    orders::require_unlocked(&state, &u)?;
    let result = orders::submit(&state, &built, &uid, "manual").await?;
    let obj = out.as_object_mut().unwrap();
    obj.insert("dry_run".to_string(), json!(false));
    obj.insert("result".to_string(), result);
    Ok(Json(out))
}

async fn list_orders(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Value>> {
    let st = q.status.unwrap_or_else(|| "OPEN".to_string()).to_uppercase();
    if st != "OPEN" && st != "CLOSED" {
        return Err(ApiError::code_only(400, "invalid_status"));
    }
    let sym = q
        .symbol
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty());
    let uid = user["id"].as_str().unwrap_or("");
    let items = orders::list_orders(&state, uid, &st, sym.as_deref()).await?;
    Ok(Json(json!({ "items": items })))
}

async fn get_order_wildcard(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(order_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let order = orders::get_order(&state, uid, &order_id).await?;
    Ok(Json(json!({ "order": order })))
}

async fn post_order_wildcard(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(order_rest): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("").to_string();
    if let Some(order_id) = order_rest.strip_suffix("/cancel") {
        let res = orders::cancel(&state, &uid, order_id).await?;
        return Ok(Json(json!({ "result": res })));
    }
    if let Some(order_id) = order_rest.strip_suffix("/modify") {
        let u = fresh_user(&state, user);
        orders::require_unlocked(&state, &u)?;
        let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
        let res = orders::modify(&state, &uid, order_id, &body).await?;
        return Ok(Json(json!({ "result": res })));
    }
    Err(ApiError::code_only(404, "not_found"))
}
