use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::CookieJar;
use serde_json::{json, Value};

use crate::auth::extractors::{AuthUser, PendingUser};
use crate::auth::otp;
use crate::auth::tokens::{self, Stage};
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::users::repository as users;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/google", post(google_login))
        .route("/otp/setup", post(otp_setup))
        .route("/otp/verify", post(otp_verify))
        .route("/me", get(me))
        .route("/logout", post(logout))
}

async fn google_login(
    State(state): State<AppState>,
    jar: CookieJar,
    body: Option<Json<Value>>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let credential = body
        .get("credential")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::code_only(400, "credential_required"))?;

    let info = state.google_verifier.verify(&state.config, credential).await?;
    let user = users::upsert_from_google(&state, &info);
    let status = users::status_of(&user);
    if status != "approved" {
        let next = if status == "pending" {
            "approval_pending"
        } else {
            "rejected"
        };
        let jar = tokens::clear(jar, Stage::Pending);
        let jar = tokens::clear(jar, Stage::Session);
        return Ok((
            jar,
            Json(json!({
                "next": next,
                "user": users::public_view(&state.config, &user),
            })),
        ));
    }

    let next = if user.get("otp_enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        "otp_verify"
    } else {
        "otp_setup"
    };
    let uid = user["id"].as_str().unwrap_or("");
    let jar = tokens::clear(jar, Stage::Session);
    let jar = tokens::issue(jar, &state.config, uid, Stage::Pending);
    Ok((
        jar,
        Json(json!({
            "next": next,
            "user": users::public_view(&state.config, &user),
        })),
    ))
}

async fn otp_setup(
    State(state): State<AppState>,
    PendingUser(user): PendingUser,
) -> ApiResult<Json<Value>> {
    Ok(Json(otp::begin_setup(&state, &user)?))
}

async fn otp_verify(
    State(state): State<AppState>,
    PendingUser(user): PendingUser,
    jar: CookieJar,
    body: Option<Json<Value>>,
) -> ApiResult<(CookieJar, Json<Value>)> {
    let body = body.map(|Json(v)| v).unwrap_or(Value::Null);
    let code = body.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let updated = otp::verify(&state, &user, code)?;
    let uid = updated["id"].as_str().unwrap_or("");
    let jar = tokens::clear(jar, Stage::Pending);
    let jar = tokens::issue(jar, &state.config, uid, Stage::Session);
    Ok((
        jar,
        Json(json!({
            "user": users::public_view(&state.config, &updated),
        })),
    ))
}

async fn me(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    Json(json!({
        "user": users::public_view(&state.config, &user),
    }))
}

async fn logout(jar: CookieJar) -> (CookieJar, Json<Value>) {
    let jar = tokens::clear(jar, Stage::Pending);
    let jar = tokens::clear(jar, Stage::Session);
    (jar, Json(json!({ "ok": true })))
}
