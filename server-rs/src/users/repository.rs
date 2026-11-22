use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::google::GoogleUserInfo;
use crate::config::Config;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::find_one;

pub const USERS: &str = "users";
pub const STATUSES: [&str; 3] = ["pending", "approved", "rejected"];

pub fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

pub fn get_user(state: &AppState, user_id: &str) -> Option<Value> {
    state.store.get(USERS, user_id)
}

pub fn get_by_google_sub(state: &AppState, sub: &str) -> Option<Value> {
    find_one(state.store.as_ref(), USERS, |u| {
        u.get("google_sub").and_then(|v| v.as_str()) == Some(sub)
    })
}

pub fn is_admin(cfg: &Config, user: &Value) -> bool {
    let email = user
        .get("email")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    !email.is_empty() && cfg.admin_emails.contains(&email)
}

pub fn status_of(user: &Value) -> &str {
    user.get("status")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("approved")
}

pub fn upsert_from_google(state: &AppState, info: &GoogleUserInfo) -> Value {
    let admin = state.config.admin_emails.contains(&info.email.to_lowercase());
    let now = now_iso();
    let (user_id, mut user) = if let Some(existing) = get_by_google_sub(state, &info.sub) {
        let uid = existing["id"].as_str().unwrap_or("").to_string();
        let mut u = existing;
        if admin && status_of(&u) != "approved" {
            if let Some(obj) = u.as_object_mut() {
                obj.insert("status".to_string(), json!("approved"));
                obj.insert("approved_at".to_string(), json!(now));
                obj.insert("approved_by".to_string(), json!("admin-email"));
            }
        }
        (uid, u)
    } else {
        let uid = Uuid::new_v4().to_string();
        let u = json!({
            "google_sub": info.sub,
            "status": if admin { "approved" } else { "pending" },
            "approved_at": if admin { Value::String(now.clone()) } else { Value::Null },
            "approved_by": if admin { Value::String("admin-email".to_string()) } else { Value::Null },
            "otp_enabled": false,
            "otp_secret_enc": Value::Null,
            "otp_pending_secret_enc": Value::Null,
            "otp_fail_count": 0,
            "otp_locked_until": Value::Null,
            "created_at": now,
        });
        (uid, u)
    };

    if let Some(obj) = user.as_object_mut() {
        obj.insert("email".to_string(), json!(info.email));
        obj.insert("name".to_string(), match &info.name {
            Some(s) => Value::String(s.clone()),
            None => Value::Null,
        });
        obj.insert("picture".to_string(), match &info.picture {
            Some(s) => Value::String(s.clone()),
            None => Value::Null,
        });
        obj.insert("last_login_at".to_string(), json!(now_iso()));
    }
    state.store.put(USERS, &user_id, user)
}

pub fn update_user(state: &AppState, user_id: &str, fields: Value) -> ApiResult<Value> {
    let mut user = get_user(state, user_id)
        .ok_or_else(|| ApiError::not_found("user_not_found", user_id))?;
    if let (Some(dst), Some(src)) = (user.as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    Ok(state.store.put(USERS, user_id, user))
}

pub fn public_view(cfg: &Config, user: &Value) -> Value {
    json!({
        "id": user.get("id").cloned().unwrap_or(Value::Null),
        "email": user.get("email").cloned().unwrap_or(Value::Null),
        "name": user.get("name").cloned().unwrap_or(Value::Null),
        "picture": user.get("picture").cloned().unwrap_or(Value::Null),
        "otp_enabled": user.get("otp_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        "status": status_of(user),
        "role": if is_admin(cfg, user) { "admin" } else { "user" },
    })
}
