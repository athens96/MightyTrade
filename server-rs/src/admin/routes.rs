use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::auth::extractors::AdminUser;
use crate::auth::otp;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::find_where;
use crate::users::{
    credentials,
    repository::{self as users, now_iso, STATUSES, USERS},
    requests_repo,
};

const BY_ID: [&str; 4] = ["users", "watchlists", "settings", "broker_credentials"];
const BY_USER_ID: [&str; 4] = ["bots", "bot_events", "orders", "user_requests"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/summary", get(summary))
        .route("/users", get(list_users))
        .route("/users/{user_id}/approve", post(approve))
        .route("/users/{user_id}/reject", post(reject))
        .route("/users/{user_id}", delete(delete_user))
        .route("/requests", get(list_requests))
        .route(
            "/requests/{req_id}",
            patch(update_request).delete(delete_request),
        )
}

#[derive(Deserialize)]
struct StatusQuery {
    status: Option<String>,
}

fn user_summary(state: &AppState, u: &Value) -> Value {
    let uid = u["id"].as_str().unwrap_or("");
    let user_bots = find_where(state.store.as_ref(), "bots", |b| {
        b.get("user_id").and_then(|v| v.as_str()) == Some(uid)
    });
    let live_bots = user_bots
        .iter()
        .filter(|b| {
            b.get("mode").and_then(|v| v.as_str()) == Some("live")
                && b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
        })
        .count();
    let has_toss_key = state
        .store
        .get("broker_credentials", uid)
        .and_then(|r| r.get("toss").cloned())
        .map(|v| !v.is_null())
        .unwrap_or(false);

    let mut base = users::public_view(&state.config, u);
    if let Some(obj) = base.as_object_mut() {
        obj.insert("created_at".to_string(), u.get("created_at").cloned().unwrap_or(Value::Null));
        obj.insert("last_login_at".to_string(), u.get("last_login_at").cloned().unwrap_or(Value::Null));
        obj.insert("approved_at".to_string(), u.get("approved_at").cloned().unwrap_or(Value::Null));
        obj.insert("approved_by".to_string(), u.get("approved_by").cloned().unwrap_or(Value::Null));
        obj.insert("has_toss_key".to_string(), json!(has_toss_key));
        obj.insert("bots".to_string(), json!(user_bots.len()));
        obj.insert("live_bots".to_string(), json!(live_bots));
    }
    base
}

fn target_user(state: &AppState, user_id: &str) -> ApiResult<Value> {
    users::get_user(state, user_id).ok_or_else(|| ApiError::code_only(404, "user_not_found"))
}

fn guard_target(state: &AppState, admin: &Value, target: &Value, action: &str) -> ApiResult<()> {
    if target["id"] == admin["id"] {
        return Err(ApiError::bad_request(
            "cannot_target_self",
            format!("자기 자신은 {action}할 수 없습니다"),
        ));
    }
    if users::is_admin(&state.config, target) {
        return Err(ApiError::bad_request(
            "cannot_target_admin",
            format!("관리자 계정은 {action}할 수 없습니다 (ADMIN_EMAILS에서 먼저 빼세요)"),
        ));
    }
    Ok(())
}

fn bot_has_live_open(b: &Value) -> bool {
    let live = b.get("state").and_then(|s| s.get("live"));
    let Some(live) = live else {
        return false;
    };
    let single_qty = live
        .get("position")
        .and_then(|p| p.get("qty"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    if single_qty > 0.0 {
        return true;
    }
    if let Some(positions) = live.get("positions").and_then(|v| v.as_object()) {
        for p in positions.values() {
            if p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0 {
                return true;
            }
        }
    }
    if let Some(pending) = live.get("pending") {
        if let Some(arr) = pending.as_array() {
            if !arr.is_empty() {
                return true;
            }
        } else if let Some(obj) = pending.as_object() {
            if !obj.is_empty() {
                return true;
            }
        }
    }
    false
}

async fn summary(State(state): State<AppState>, AdminUser(_): AdminUser) -> Json<Value> {
    let all_users = state.store.find_all(USERS);
    let pending = all_users
        .iter()
        .filter(|u| users::status_of(u) == "pending")
        .count();
    let open_requests = requests_repo::open_count(&state);
    Json(json!({ "pending": pending, "open_requests": open_requests }))
}

async fn list_users(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    let status = q.status.unwrap_or_else(|| "all".to_string());
    if status != "all" && !STATUSES.contains(&status.as_str()) {
        return Err(ApiError::code_only(400, "invalid_status"));
    }
    let all_users = state.store.find_all(USERS);
    let mut counts = Map::new();
    for s in STATUSES {
        let c = all_users.iter().filter(|u| users::status_of(u) == s).count();
        counts.insert(s.to_string(), json!(c));
    }
    counts.insert("all".to_string(), json!(all_users.len()));

    let mut rows: Vec<Value> = all_users
        .into_iter()
        .filter(|u| status == "all" || users::status_of(u) == status)
        .collect();
    rows.sort_by(|a, b| {
        let pa = users::status_of(a) != "pending";
        let pb = users::status_of(b) != "pending";
        let ca = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let cb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        pa.cmp(&pb).then_with(|| ca.cmp(cb))
    });
    let items: Vec<Value> = rows.iter().map(|u| user_summary(&state, u)).collect();
    Ok(Json(json!({ "items": items, "counts": Value::Object(counts) })))
}

async fn approve(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let u = target_user(&state, &user_id)?;
    let updated = users::update_user(
        &state,
        u["id"].as_str().unwrap_or(""),
        json!({
            "status": "approved",
            "approved_at": now_iso(),
            "approved_by": admin.get("email").cloned().unwrap_or(Value::Null),
        }),
    )?;
    Ok(Json(json!({ "user": user_summary(&state, &updated) })))
}

async fn reject(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let u = target_user(&state, &user_id)?;
    guard_target(&state, &admin, &u, "거절")?;
    let uid = u["id"].as_str().unwrap_or("").to_string();
    let updated = users::update_user(
        &state,
        &uid,
        json!({
            "status": "rejected",
            "approved_at": Value::Null,
            "approved_by": admin.get("email").cloned().unwrap_or(Value::Null),
        }),
    )?;

    // Stop every enabled bot owned by the rejected user
    let user_bots = find_where(state.store.as_ref(), "bots", |b| {
        b.get("user_id").and_then(|v| v.as_str()) == Some(&uid)
    });
    for mut b in user_bots {
        if b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            let bid = b["id"].as_str().unwrap_or("").to_string();
            if let Some(obj) = b.as_object_mut() {
                obj.insert("enabled".to_string(), json!(false));
                obj.insert("updated_at".to_string(), json!(now_iso()));
            }
            state.store.put("bots", &bid, b);
            let ev_id = Uuid::new_v4().simple().to_string();
            state.store.put(
                "bot_events",
                &ev_id,
                json!({
                    "bot_id": bid,
                    "user_id": uid,
                    "kind": "config",
                    "level": "warn",
                    "message": "계정 승인이 취소되어 꺼졌습니다",
                    "at": now_iso(),
                }),
            );
        }
    }
    Ok(Json(json!({ "user": user_summary(&state, &updated) })))
}

async fn delete_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(user_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let u = target_user(&state, &user_id)?;
    guard_target(&state, &admin, &u, "삭제")?;

    let fresh_admin = users::get_user(&state, admin["id"].as_str().unwrap_or("")).unwrap_or(admin);
    let otp_code = body.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
    otp::verify(&state, &fresh_admin, otp_code)?;

    let uid = u["id"].as_str().unwrap_or("").to_string();
    let user_bots = find_where(state.store.as_ref(), "bots", |b| {
        b.get("user_id").and_then(|v| v.as_str()) == Some(&uid)
    });
    let live_open_cnt = user_bots.iter().filter(|b| bot_has_live_open(b)).count();
    let force = body.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    if live_open_cnt > 0 && !force {
        return Err(ApiError::conflict(
            "live_positions",
            format!(
                "실거래 봇 {live_open_cnt}개에 보유·미체결 주문이 있습니다. 사용자가 정리한 뒤 삭제하세요 (토스 계좌의 주문·보유는 삭제와 무관하게 남습니다)"
            ),
        ));
    }

    let creds = credentials::get_toss(&state, &uid);
    if let Some(c) = creds {
        state.forget_toss_client(&uid, Some(&c.client_id));
    }

    let mut removed = Map::new();
    for coll in BY_USER_ID {
        let rows = find_where(state.store.as_ref(), coll, |r| {
            r.get("user_id").and_then(|v| v.as_str()) == Some(&uid)
        });
        let n = rows.len();
        for r in rows {
            if let Some(id) = r.get("id").and_then(|v| v.as_str()) {
                state.store.delete(coll, id);
            }
        }
        removed.insert(coll.to_string(), json!(n));
    }
    for coll in BY_ID {
        let deleted = state.store.delete(coll, &uid);
        removed.insert(coll.to_string(), json!(if deleted { 1 } else { 0 }));
    }
    Ok(Json(json!({ "ok": true, "removed": Value::Object(removed) })))
}

async fn list_requests(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(q): Query<StatusQuery>,
) -> ApiResult<Json<Value>> {
    let status = q.status.unwrap_or_else(|| "all".to_string());
    let (items, counts) = requests_repo::list_all_for_admin(&state, &status)?;
    Ok(Json(json!({ "items": items, "counts": counts })))
}

async fn update_request(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(req_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let item = requests_repo::update_by_admin(&state, &req_id, &admin, &body)?;
    Ok(Json(json!({ "item": item })))
}

async fn delete_request(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(req_id): Path<String>,
) -> ApiResult<Json<Value>> {
    requests_repo::delete_by_admin(&state, &req_id)?;
    Ok(Json(json!({ "ok": true })))
}
