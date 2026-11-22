//! User requests repository (`user_requests` collection, MT-16).
//! Strictly isolates incoming requests so only administrators (`ADMIN_EMAILS`) receive & manage them.

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::find_where;
use crate::users::credentials::{deliver_notification, get_notifications};
use crate::users::repository::{is_admin, now_iso, USERS};

pub const USER_REQUESTS: &str = "user_requests";
pub const CATEGORIES: [&str; 4] = ["feature", "bug", "strategy", "other"];
pub const STATUSES: [&str; 4] = ["open", "in_review", "resolved", "closed"];
pub const MAX_OPEN_PER_USER: usize = 10;

fn category_label(cat: &str) -> &str {
    match cat {
        "feature" => "기능 건의",
        "bug" => "오류·버그 제보",
        "strategy" => "전략·지표 추가",
        "other" => "기타 문의",
        other => other,
    }
}

fn status_order(st: &str) -> usize {
    match st {
        "open" => 0,
        "in_review" => 1,
        "resolved" => 2,
        "closed" => 3,
        _ => 9,
    }
}

pub fn user_view(rec: &Value) -> Value {
    json!({
        "id": rec.get("id").cloned().unwrap_or(Value::Null),
        "category": rec.get("category").and_then(|v| v.as_str()).unwrap_or("other"),
        "title": rec.get("title").and_then(|v| v.as_str()).unwrap_or(""),
        "content": rec.get("content").and_then(|v| v.as_str()).unwrap_or(""),
        "page": rec.get("page").cloned().unwrap_or(Value::Null),
        "app_version": rec.get("app_version").cloned().unwrap_or(Value::Null),
        "status": rec.get("status").and_then(|v| v.as_str()).unwrap_or("open"),
        "admin_reply": rec.get("admin_reply").and_then(|v| v.as_str()).unwrap_or(""),
        "replied_at": rec.get("replied_at").cloned().unwrap_or(Value::Null),
        "created_at": rec.get("created_at").cloned().unwrap_or(Value::Null),
        "updated_at": rec.get("updated_at").cloned().unwrap_or(Value::Null),
    })
}

pub fn admin_view(rec: &Value) -> Value {
    let mut base = user_view(rec);
    if let Some(obj) = base.as_object_mut() {
        obj.insert("user_id".to_string(), rec.get("user_id").cloned().unwrap_or(Value::Null));
        obj.insert(
            "user_email".to_string(),
            rec.get("user_email").cloned().unwrap_or(Value::Null),
        );
        obj.insert(
            "user_name".to_string(),
            rec.get("user_name").cloned().unwrap_or(Value::Null),
        );
        obj.insert(
            "replied_by".to_string(),
            rec.get("replied_by").cloned().unwrap_or(Value::Null),
        );
    }
    base
}

async fn notify_admins_new_request(state: &AppState, rec: &Value) {
    let all_users = state.store.find_all(USERS);
    let admins: Vec<Value> = all_users
        .into_iter()
        .filter(|u| is_admin(&state.config, u))
        .collect();
    if admins.is_empty() {
        return;
    }

    let cat = rec.get("category").and_then(|v| v.as_str()).unwrap_or("other");
    let cat_label = category_label(cat);
    let name = rec
        .get("user_name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("사용자");
    let email = rec
        .get("user_email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("-");
    let mut meta = vec![cat_label.to_string()];
    if let Some(page) = rec.get("page").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        meta.push(format!("화면 {page}"));
    }
    if let Some(ver) = rec
        .get("app_version")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        meta.push(format!("v{ver}"));
    }
    let content = rec.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let body_preview = if content.chars().count() <= 350 {
        content.to_string()
    } else {
        let p: String = content.chars().take(350).collect();
        format!("{p}…")
    };
    let title = rec.get("title").and_then(|v| v.as_str()).unwrap_or("");
    let text = format!(
        "📩 [Mighty Trade · 사용자 요청사항 접수]\n보낸 사람: {name} ({email})\n분류: {}\n제목: {title}\n내용: {body_preview}",
        meta.join(" · ")
    );

    for admin in admins {
        let Some(uid) = admin.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(cfg) = get_notifications(state, uid) {
            if cfg.enabled {
                let _ = deliver_notification(state, &cfg, &text).await;
            }
        }
    }
}

pub async fn create_request(state: &AppState, user: &Value, body: &Value) -> ApiResult<Value> {
    let category = body
        .get("category")
        .and_then(|v| v.as_str())
        .unwrap_or("feature")
        .trim();
    if !CATEGORIES.contains(&category) {
        return Err(ApiError::bad_request(
            "invalid_category",
            "요청 분류를 올바르게 선택해 주세요",
        ));
    }

    let title = body
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let title_len = title.chars().count();
    if !(2..=100).contains(&title_len) {
        return Err(ApiError::bad_request(
            "invalid_title",
            "제목은 2자 이상 100자 이하로 입력해 주세요",
        ));
    }

    let content = body
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    let content_len = content.chars().count();
    if !(5..=2000).contains(&content_len) {
        return Err(ApiError::bad_request(
            "invalid_content",
            "요청 내용은 5자 이상 2,000자 이하로 적어 주세요",
        ));
    }

    let page: Option<String> = body
        .get("page")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().chars().take(120).collect::<String>())
        .filter(|s| !s.is_empty());
    let app_version: Option<String> = body
        .get("app_version")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().chars().take(32).collect::<String>())
        .filter(|s| !s.is_empty());

    let uid = user["id"].as_str().unwrap_or("");
    let open_cnt = find_where(state.store.as_ref(), USER_REQUESTS, |r| {
        r.get("user_id").and_then(|v| v.as_str()) == Some(uid)
            && r.get("status").and_then(|v| v.as_str()) == Some("open")
    })
    .len();
    if open_cnt >= MAX_OPEN_PER_USER {
        return Err(ApiError::too_many_requests(
            "too_many_open_requests",
            format!("아직 접수 대기 중인 요청이 {MAX_OPEN_PER_USER}건 있습니다. 관리자 검토 후 추가로 보내 주세요."),
        ));
    }

    let req_id = Uuid::new_v4().simple().to_string()[..12].to_string();
    let ts = now_iso();
    let rec = json!({
        "id": req_id,
        "user_id": uid,
        "user_email": user.get("email").cloned().unwrap_or(Value::Null),
        "user_name": user.get("name").cloned().unwrap_or(Value::Null),
        "category": category,
        "title": title,
        "content": content,
        "page": page,
        "app_version": app_version,
        "status": "open",
        "admin_reply": "",
        "replied_at": Value::Null,
        "replied_by": Value::Null,
        "created_at": ts,
        "updated_at": ts,
    });
    let saved = state.store.put(USER_REQUESTS, &req_id, rec);
    notify_admins_new_request(state, &saved).await;
    Ok(user_view(&saved))
}

pub fn list_for_user(state: &AppState, user_id: &str) -> Vec<Value> {
    let mut rows = find_where(state.store.as_ref(), USER_REQUESTS, |r| {
        r.get("user_id").and_then(|v| v.as_str()) == Some(user_id)
    });
    rows.sort_by(|a, b| {
        let ta = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        tb.cmp(ta)
    });
    rows.iter().map(user_view).collect()
}

pub fn open_count(state: &AppState) -> usize {
    state
        .store
        .find_all(USER_REQUESTS)
        .into_iter()
        .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some("open"))
        .count()
}

pub fn list_all_for_admin(state: &AppState, status: &str) -> ApiResult<(Vec<Value>, Value)> {
    if status != "all" && !STATUSES.contains(&status) {
        return Err(ApiError::code_only(400, "invalid_status"));
    }
    let all_rows = state.store.find_all(USER_REQUESTS);
    let mut counts = Map::new();
    for s in STATUSES {
        let c = all_rows
            .iter()
            .filter(|r| r.get("status").and_then(|v| v.as_str()) == Some(s))
            .count();
        counts.insert(s.to_string(), json!(c));
    }
    counts.insert("all".to_string(), json!(all_rows.len()));

    let mut rows: Vec<Value> = all_rows
        .into_iter()
        .filter(|r| status == "all" || r.get("status").and_then(|v| v.as_str()) == Some(status))
        .collect();
    rows.sort_by(|a, b| {
        let oa = status_order(a.get("status").and_then(|v| v.as_str()).unwrap_or("open"));
        let ob = status_order(b.get("status").and_then(|v| v.as_str()).unwrap_or("open"));
        let ta = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let tb = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        oa.cmp(&ob).then_with(|| tb.cmp(ta))
    });
    Ok((rows.iter().map(admin_view).collect(), Value::Object(counts)))
}

pub fn update_by_admin(
    state: &AppState,
    req_id: &str,
    admin_user: &Value,
    body: &Value,
) -> ApiResult<Value> {
    let mut rec = state.store.get(USER_REQUESTS, req_id).ok_or_else(|| {
        ApiError::not_found("request_not_found", "해당 요청사항을 찾을 수 없습니다")
    })?;
    let obj = rec.as_object_mut().unwrap();

    if let Some(st) = body.get("status").and_then(|v| v.as_str()) {
        let st = st.trim();
        if !STATUSES.contains(&st) {
            return Err(ApiError::code_only(400, "invalid_status"));
        }
        obj.insert("status".to_string(), json!(st));
    }

    if let Some(reply) = body.get("admin_reply").and_then(|v| v.as_str()) {
        let reply = reply.trim();
        if reply.chars().count() > 2000 {
            return Err(ApiError::bad_request(
                "reply_too_long",
                "관리자 답변은 2,000자 이하로 입력해 주세요",
            ));
        }
        obj.insert("admin_reply".to_string(), json!(reply));
        if !reply.is_empty() {
            obj.insert("replied_at".to_string(), json!(now_iso()));
            obj.insert(
                "replied_by".to_string(),
                admin_user.get("email").cloned().unwrap_or(Value::Null),
            );
        } else {
            obj.insert("replied_at".to_string(), Value::Null);
            obj.insert("replied_by".to_string(), Value::Null);
        }
    }

    obj.insert("updated_at".to_string(), json!(now_iso()));
    let saved = state.store.put(USER_REQUESTS, req_id, rec);
    Ok(admin_view(&saved))
}

pub fn delete_by_admin(state: &AppState, req_id: &str) -> ApiResult<()> {
    if !state.store.delete(USER_REQUESTS, req_id) {
        return Err(ApiError::not_found(
            "request_not_found",
            "해당 요청사항을 찾을 수 없습니다",
        ));
    }
    Ok(())
}
