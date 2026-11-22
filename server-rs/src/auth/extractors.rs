use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum_extra::extract::CookieJar;
use serde_json::Value;

use crate::auth::tokens::{self, Stage};
use crate::errors::ApiError;
use crate::state::AppState;
use crate::users::repository as users;

pub struct PendingUser(pub Value);
pub struct AuthUser(pub Value);
pub struct AdminUser(pub Value);

async fn extract_stage(parts: &mut Parts, state: &AppState, stage: Stage) -> Result<Value, ApiError> {
    let jar = CookieJar::from_request_parts(parts, state)
        .await
        .map_err(|_| ApiError::unauthorized("unauthorized", "로그인이 만료되었습니다. 다시 로그인해 주세요"))?;
    let user_id = tokens::read(&jar, &state.config, stage);
    let user = user_id.and_then(|id| users::get_user(state, &id));
    let Some(user) = user else {
        return Err(ApiError::unauthorized(
            "unauthorized",
            "로그인이 만료되었습니다. 다시 로그인해 주세요",
        ));
    };
    if users::status_of(&user) != "approved" {
        return Err(ApiError::unauthorized(
            "not_approved",
            "관리자 승인 후 이용할 수 있습니다",
        ));
    }
    if stage == Stage::Session
        && !user
            .get("otp_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return Err(ApiError::unauthorized(
            "otp_required",
            "OTP 2단계 인증이 필요합니다",
        ));
    }
    Ok(user)
}

impl FromRequestParts<AppState> for PendingUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(PendingUser(extract_stage(parts, state, Stage::Pending).await?))
    }
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        Ok(AuthUser(extract_stage(parts, state, Stage::Session).await?))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let user = extract_stage(parts, state, Stage::Session).await?;
        if !users::is_admin(&state.config, &user) {
            return Err(ApiError::forbidden("admin_only", "관리자만 사용할 수 있습니다"));
        }
        Ok(AdminUser(user))
    }
}
