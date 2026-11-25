//! Auto-trading API: bots, paper/semi/live mode, kill switch, analytics, webhook receiver.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::extractors::AuthUser;
use crate::auth::otp;
use crate::autotrade::{analytics, bots, executor, protections, scalp, scheduler, semi, webhook};
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::users::{credentials, repository as users};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/status", get(status))
        .route("/halt", post(halt))
        .route("/resume", post(resume))
        .route("/overview", get(overview))
        .route("/bots", get(list_bots).post(create_bot))
        .route("/bots/{bot_id}", get(get_bot).put(update_bot).delete(delete_bot))
        .route("/bots/{bot_id}/enable", post(enable_bot))
        .route("/bots/{bot_id}/disable", post(disable_bot))
        .route("/bots/{bot_id}/mode", post(set_mode))
        .route("/bots/{bot_id}/approve", post(approve_bot_order))
        .route("/bots/{bot_id}/skip", post(skip_bot_order))
        .route("/bots/{bot_id}/evaluate", post(evaluate_bot))
        .route("/bots/{bot_id}/run", post(run_bot_now))
        .route("/bots/{bot_id}/webhook-secret", post(manage_webhook_secret))
        .route("/bots/{bot_id}/reset", post(reset_bot))
        .route("/webhook/{bot_id}", post(inbound_webhook))
}

fn fresh_user(state: &AppState, fallback: &Value) -> Value {
    let uid = fallback["id"].as_str().unwrap_or("");
    users::get_user(state, uid).unwrap_or_else(|| fallback.clone())
}

fn need_toss(state: &AppState, uid: &str) -> ApiResult<()> {
    if !credentials::has_active_broker(state, uid) {
        return Err(ApiError::conflict(
            "toss_credentials_required",
            "증권사 API 키(토스증권 또는 키움증권)를 먼저 등록하세요 (설정)",
        ));
    }
    Ok(())
}

async fn status(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let u = fresh_user(&state, &user);
    Json(json!({
        "halted": u.get("autotrade_halted").and_then(|v| v.as_bool()).unwrap_or(false),
        "scheduler": state.scheduler.status_json(),
    }))
}

async fn halt(State(state): State<AppState>, AuthUser(user): AuthUser) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let _ = users::update_user(&state, uid, json!({ "autotrade_halted": true }))?;
    let mut cancelled = 0usize;
    for mut bot in bots::list_for(&state, uid) {
        let has_pend = bot["state"]["live"]["pending"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        if has_pend {
            match executor::cancel_live_pending(&state, &bot).await {
                Ok(n) => cancelled += n,
                Err(e) => {
                    bots::log(
                        &state,
                        &bot,
                        "error",
                        &format!("전체 중지 중 취소 실패: {}", e.message),
                        "error",
                        None,
                    )
                    .await;
                }
            }
        }
        bots::log(
            &state,
            &bot,
            "halt",
            "전체 중지 — 자동매매가 멈췄습니다",
            "warn",
            None,
        )
        .await;
        let _ = &mut bot;
    }
    Ok(Json(json!({ "halted": true, "cancelled": cancelled })))
}

async fn resume(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let u = fresh_user(&state, &user);
    let code = body.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
    otp::verify(&state, &u, code)?;
    let uid = u["id"].as_str().unwrap_or("");
    let _ = users::update_user(&state, uid, json!({ "autotrade_halted": false }))?;
    Ok(Json(json!({ "halted": false })))
}

async fn overview(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    Json(analytics::build_overview(&state, uid))
}

async fn list_bots(State(state): State<AppState>, AuthUser(user): AuthUser) -> Json<Value> {
    let uid = user["id"].as_str().unwrap_or("");
    let items: Vec<Value> = bots::list_for(&state, uid)
        .iter()
        .map(bots::public)
        .collect();
    Json(json!({ "items": items }))
}

async fn create_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let rec = bots::create(&state, uid, &body)?;
    Ok(Json(json!({ "bot": bots::public(&rec) })))
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<usize>,
}

async fn get_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
    Query(q): Query<EventsQuery>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let bot = bots::get(&state, uid, &bot_id)?;
    let evs = bots::events(&state, &bot, q.limit.unwrap_or(100));
    Ok(Json(json!({ "bot": bots::public(&bot), "events": evs })))
}

async fn update_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let holding = bots::has_open_position(&bot, None);
    let kind = bots::bot_kind(&bot);
    let locked_keys: &[&str] = match kind {
        "rebalance" => &["market", "model", "assets"],
        "rotation" => &["market", "preset", "universe"],
        "scalp" => &["market", "source", "condition_seq"],
        "dca" | "webhook" => &["market", "symbol"],
        _ => &["market", "symbol", "strategy"],
    };
    if holding {
        if let Some(bo) = body.as_object() {
            if locked_keys
                .iter()
                .any(|k| bo.get(*k).is_some_and(|v| Some(v) != bot.get(*k)))
            {
                return Err(ApiError::conflict(
                    "bot_has_position",
                    "보유 중인 봇은 종목·전략(이론)을 바꿀 수 없습니다",
                ));
            }
        }
    }
    let fields = bots::validate(&state, &body, Some(&bot))?;
    let new_cap = fields["capital"].as_f64().unwrap_or(0.0);
    let old_cap = bot["capital"].as_f64().unwrap_or(0.0);
    if (new_cap - old_cap).abs() > 1e-9 && !bots::has_open_position(&bot, Some("paper")) {
        let cur_cash = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
        bot["state"]["paper"]["cash"] = json!(cur_cash + (new_cap - old_cap));
    }
    if let (Some(dst), Some(src)) = (bot.as_object_mut(), fields.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    bots::log(&state, &bot, "config", "설정이 변경되었습니다", "info", None).await;
    let saved = bots::save(&state, &mut bot);
    Ok(Json(json!({ "bot": bots::public(&saved) })))
}

async fn delete_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let bot = bots::get(&state, uid, &bot_id)?;
    let has_pend = bot["state"]["live"]["pending"]
        .as_array()
        .is_some_and(|a| !a.is_empty());
    if bots::has_open_position(&bot, Some("live")) || has_pend {
        return Err(ApiError::conflict(
            "bot_has_position",
            "실거래 보유분이나 미체결 주문이 있는 봇은 삭제할 수 없습니다",
        ));
    }
    bots::delete(&state, &bot_id);
    Ok(Json(json!({ "ok": true })))
}

async fn enable_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    need_toss(&state, uid)?;
    protections::acknowledge_on_enable(&mut bot);
    bot["enabled"] = json!(true);
    let mode_lbl = match bot["mode"].as_str().unwrap_or("paper") {
        "paper" => "모의",
        "semi" => "승인 후 주문(반자동)",
        _ => "실거래",
    };
    bots::log(&state, &bot, "config", &format!("켜짐 ({mode_lbl})"), "info", None).await;
    let saved = bots::save(&state, &mut bot);
    Ok(Json(json!({ "bot": bots::public(&saved) })))
}

async fn disable_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    bot["enabled"] = json!(false);
    bots::log(
        &state,
        &bot,
        "config",
        "꺼짐 (이미 들어간 주문·보유는 그대로입니다)",
        "info",
        None,
    )
    .await;
    let saved = bots::save(&state, &mut bot);
    Ok(Json(json!({ "bot": bots::public(&saved) })))
}

async fn set_mode(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("");
    if !matches!(mode, "paper" | "semi" | "live") {
        return Err(ApiError::code_only(400, "invalid_mode"));
    }
    let prev_mode = bot["mode"].as_str().unwrap_or("paper");
    if (mode == "semi" || mode == "live") && !(prev_mode == "live" && mode == "semi") {
        need_toss(&state, uid)?;
        let u = fresh_user(&state, &user);
        let code = body.get("otp_code").and_then(|v| v.as_str()).unwrap_or("");
        otp::verify(&state, &u, code)?;
    }
    bot["mode"] = json!(mode);
    bot["enabled"] = json!(false);
    let (msg, lvl) = match mode {
        "paper" => ("모의 모드로 전환", "info"),
        "semi" => (
            "승인 후 주문(반자동) 모드로 전환 (꺼진 상태 — 신호 발생 시 1클릭 승인 후 주문 전송)",
            "info",
        ),
        _ => ("실거래 모드로 전환 (꺼진 상태 — 켜면 실제 주문이 나갑니다)", "warn"),
    };
    bots::log(&state, &bot, "config", msg, lvl, None).await;
    let saved = bots::save(&state, &mut bot);
    Ok(Json(json!({ "bot": bots::public(&saved) })))
}

async fn approve_bot_order(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    need_toss(&state, uid)?;
    let u = fresh_user(&state, &user);
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let otp_code = body
        .get("otp_code")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !otp_code.is_empty() {
        trading::unlock(&state, &u, otp_code)?;
    } else {
        trading::require_unlocked(&state, &u)?;
    }
    let res = semi::approve_proposal(&state, &mut bot, None).await?;
    let fresh_bot = bots::get(&state, uid, &bot_id)?;
    Ok(Json(json!({ "result": res, "bot": bots::public(&fresh_bot) })))
}

async fn skip_bot_order(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    semi::skip_proposal(&state, &mut bot).await?;
    let fresh_bot = bots::get(&state, uid, &bot_id)?;
    Ok(Json(json!({ "bot": bots::public(&fresh_bot) })))
}

async fn evaluate_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let bot = bots::get(&state, uid, &bot_id)?;
    need_toss(&state, uid)?;
    let market = bot["market"].as_str().unwrap_or("KR");
    let td = scheduler::target_date(&state, uid, market, Utc::now()).await?;
    Ok(Json(executor::preview(&state, &bot, &td).await?))
}

async fn run_bot_now(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    if bot["mode"].as_str() != Some("paper") {
        return Err(ApiError::bad_request(
            "paper_only",
            "즉시 실행은 모의 모드에서만 가능합니다 (실거래는 스케줄러가 실행)",
        ));
    }
    need_toss(&state, uid)?;
    let market = bot["market"].as_str().unwrap_or("KR").to_string();
    let now = Utc::now();
    let sess = scheduler::active_session(&state, uid, &market, now)
        .await?
        .ok_or_else(|| {
            ApiError::conflict("market_closed", "정규장 시간에만 실행할 수 있습니다")
        })?;
    let res = if executor::is_scalp(&bot) {
        scalp::run_intraday(&state, &mut bot, &sess.0, now, sess.1, sess.2).await?
    } else if executor::is_closing_bet(&bot) {
        executor::run_closing_bet(&state, &mut bot, &sess.0, now, sess.1, sess.2, true).await?
    } else if executor::is_intraday(&bot) {
        executor::run_intraday(&state, &mut bot, &sess.0, now, sess.2).await?
    } else {
        executor::run(&state, &mut bot, &sess.0).await?
    };
    let fresh_bot = bots::get(&state, uid, &bot_id)?;
    Ok(Json(json!({ "result": res, "bot": bots::public(&fresh_bot) })))
}

async fn manage_webhook_secret(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    let rotate = body
        .and_then(|Json(v)| v.get("rotate").and_then(|x| x.as_bool()))
        .unwrap_or(false);
    let secret = webhook::get_or_rotate_secret(&state, &mut bot, rotate).await?;
    Ok(Json(json!({ "secret": secret, "bot": bots::public(&bot) })))
}

async fn inbound_webhook(
    State(state): State<AppState>,
    Path(bot_id): Path<String>,
    headers: HeaderMap,
    body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
    let body = body.map(|Json(v)| v).unwrap_or_else(|| json!({}));
    let hdr_secret = headers
        .get("X-Webhook-Secret")
        .and_then(|v| v.to_str().ok());
    let ip = headers
        .get("X-Forwarded-For")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .or_else(|| headers.get("X-Real-IP").and_then(|v| v.to_str().ok()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    Ok(Json(
        webhook::receive_webhook(&state, &bot_id, &body, hdr_secret, &ip).await?,
    ))
}

async fn reset_bot(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(bot_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let uid = user["id"].as_str().unwrap_or("");
    let mut bot = bots::get(&state, uid, &bot_id)?;
    if bot["mode"].as_str() != Some("paper") {
        return Err(ApiError::bad_request(
            "paper_only",
            "모의 모드 기록만 초기화할 수 있습니다",
        ));
    }
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let kind = bots::bot_kind(&bot);
    bot["state"]["paper"] = if kind == "scalp" {
        scalp::empty_state(cap)
    } else if kind == "rebalance" || kind == "rotation" {
        bots::empty_rebalance_state(cap)
    } else {
        bots::empty_state(cap)
    };
    bots::log(&state, &bot, "config", "모의 기록 초기화", "info", None).await;
    let saved = bots::save(&state, &mut bot);
    Ok(Json(json!({ "bot": bots::public(&saved) })))
}
