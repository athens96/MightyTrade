use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::IpAddr;
use url::Url;

use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::users::repository::now_iso;

pub const CREDENTIALS: &str = "broker_credentials";
pub const DEFAULT_NOTIFY_EVENT_KEYS: [(&str, bool); 5] = [
    ("order", true),
    ("fill", true),
    ("error", true),
    ("halt", true),
    ("briefing", true),
];

#[derive(Debug, Clone)]
pub struct TossCreds {
    pub client_id: String,
    pub client_secret: String,
    pub account_seq: Option<i64>,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct NotificationConfig {
    pub enabled: bool,
    pub paper_notify: bool,
    pub telegram_bot_token: Option<String>,
    pub telegram_chat_id: String,
    pub discord_webhook_url: Option<String>,
    pub slack_webhook_url: Option<String>,
    pub events: HashMap<String, bool>,
}

pub fn mask(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= 12 {
        let prefix: String = chars.iter().take(2).collect();
        format!("{prefix}…")
    } else {
        let head: String = chars.iter().take(10).collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    }
}

fn mask_url(url: &str) -> String {
    let chars: Vec<char> = url.chars().collect();
    if chars.len() <= 28 {
        let head: String = chars.iter().take(14).collect();
        format!("{head}…")
    } else {
        let head: String = chars.iter().take(24).collect();
        let tail: String = chars[chars.len() - 6..].iter().collect();
        format!("{head}…{tail}")
    }
}

pub fn save_toss(
    state: &AppState,
    user_id: &str,
    client_id: &str,
    client_secret: &str,
    account_seq: Option<i64>,
) -> Value {
    let mut rec = state
        .store
        .get(CREDENTIALS, user_id)
        .unwrap_or_else(|| json!({}));
    let now = now_iso();
    let toss_val = json!({
        "client_id_enc": state.crypto.encrypt(client_id),
        "client_secret_enc": state.crypto.encrypt(client_secret),
        "client_id_masked": mask(client_id),
        "account_seq": account_seq,
        "verified_at": now,
        "updated_at": now,
    });
    if let Some(obj) = rec.as_object_mut() {
        obj.insert("toss".to_string(), toss_val);
    }
    state.store.put(CREDENTIALS, user_id, rec)["toss"].clone()
}

pub fn get_toss(state: &AppState, user_id: &str) -> Option<TossCreds> {
    let rec = state.store.get(CREDENTIALS, user_id)?;
    let toss = rec.get("toss")?.as_object()?;
    let cid_enc = toss.get("client_id_enc")?.as_str()?;
    let sec_enc = toss.get("client_secret_enc")?.as_str()?;
    let client_id = state.crypto.decrypt(cid_enc).ok()?;
    let client_secret = state.crypto.decrypt(sec_enc).ok()?;
    let account_seq = toss.get("account_seq").and_then(|v| v.as_i64());
    let version = toss
        .get("updated_at")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(TossCreds {
        client_id,
        client_secret,
        account_seq,
        version,
    })
}

pub fn delete_toss(state: &AppState, user_id: &str) -> bool {
    let Some(mut rec) = state.store.get(CREDENTIALS, user_id) else {
        return false;
    };
    let removed = rec.as_object_mut().and_then(|m| m.remove("toss")).is_some();
    if removed {
        state.store.put(CREDENTIALS, user_id, rec);
    }
    removed
}

pub fn public_view(state: &AppState, user_id: &str) -> Value {
    let Some(rec) = state.store.get(CREDENTIALS, user_id) else {
        return json!({ "configured": false });
    };
    let Some(toss) = rec.get("toss").and_then(|v| v.as_object()) else {
        return json!({ "configured": false });
    };
    json!({
        "configured": true,
        "client_id_masked": toss.get("client_id_masked").cloned().unwrap_or(Value::Null),
        "account_seq": toss.get("account_seq").cloned().unwrap_or(Value::Null),
        "verified_at": toss.get("verified_at").cloned().unwrap_or(Value::Null),
    })
}

fn validate_webhook_url(raw: &str, label: &str) -> ApiResult<String> {
    let parsed = Url::parse(raw).map_err(|_| {
        ApiError::bad_request(
            "invalid_webhook_url",
            format!("{label} 주소는 https:// 로 시작해야 합니다"),
        )
    })?;
    if parsed.scheme() != "https" {
        return Err(ApiError::bad_request(
            "invalid_webhook_url",
            format!("{label} 주소는 https:// 로 시작해야 합니다"),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_webhook_url",
                format!("{label} 주소는 https:// 로 시작해야 합니다"),
            )
        })?
        .to_lowercase();
    let host_unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    if host_unbracketed == "localhost"
        || host_unbracketed == "localhost.localdomain"
        || host_unbracketed.ends_with(".local")
        || host_unbracketed.ends_with(".internal")
    {
        return Err(ApiError::bad_request(
            "invalid_webhook_url",
            format!("{label} 주소에 내부 네트워크 호스트를 사용할 수 없습니다"),
        ));
    }
    if let Ok(ip) = host_unbracketed.parse::<IpAddr>() {
        let blocked = match ip {
            IpAddr::V4(v4) => {
                v4.is_private()
                    || v4.is_loopback()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_documentation()
                    || v4.is_multicast()
                    || v4.is_unspecified()
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_multicast()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
        if blocked {
            return Err(ApiError::bad_request(
                "invalid_webhook_url",
                format!("{label} 주소에 사설 IP를 사용할 수 없습니다"),
            ));
        }
    }
    Ok(raw.to_string())
}

fn default_events_map() -> Map<String, Value> {
    let mut m = Map::new();
    for (k, d) in DEFAULT_NOTIFY_EVENT_KEYS {
        m.insert(k.to_string(), Value::Bool(d));
    }
    m
}

pub fn save_notifications(state: &AppState, user_id: &str, body: &Value) -> ApiResult<Value> {
    let mut rec = state
        .store
        .get(CREDENTIALS, user_id)
        .unwrap_or_else(|| json!({}));
    let prev = rec
        .get("notifications")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let body_obj = body.as_object();

    let resolve_secret = |field_in: &str,
                          enc_key: &str,
                          mask_fn: fn(&str) -> String,
                          validator: Option<(&str, fn(&str, &str) -> ApiResult<String>)>|
     -> ApiResult<(Option<String>, Option<String>)> {
        let val = body_obj.and_then(|o| o.get(field_in));
        let keep = match val {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(s)) if s == "__KEEP__" => true,
            _ => false,
        };
        if keep {
            let prev_enc = prev.get(enc_key).and_then(|v| v.as_str()).map(|s| s.to_string());
            let prev_mask = prev
                .get(&format!("{field_in}_masked"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            return Ok((prev_enc, prev_mask));
        }
        let raw = val
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if raw.is_empty() {
            return Ok((None, None));
        }
        let validated = if let Some((label, vf)) = validator {
            vf(&raw, label)?
        } else {
            raw
        };
        Ok((
            Some(state.crypto.encrypt(&validated)),
            Some(mask_fn(&validated)),
        ))
    };

    let (tg_enc, tg_masked) = resolve_secret("telegram_bot_token", "telegram_bot_token_enc", mask, None)?;
    let (dc_enc, dc_masked) = resolve_secret(
        "discord_webhook_url",
        "discord_webhook_enc",
        mask_url,
        Some(("디스코드 웹훅", validate_webhook_url)),
    )?;
    let (sl_enc, sl_masked) = resolve_secret(
        "slack_webhook_url",
        "slack_webhook_enc",
        mask_url,
        Some(("슬랙 웹훅", validate_webhook_url)),
    )?;

    let chat_id = body_obj
        .and_then(|o| o.get("telegram_chat_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| {
            prev.get("telegram_chat_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string()
        });
    if tg_enc.is_some() && chat_id.is_empty() {
        return Err(ApiError::bad_request(
            "telegram_chat_id_required",
            "텔레그램 알림을 받으려면 Chat ID도 함께 입력하세요",
        ));
    }

    let raw_events = body_obj
        .and_then(|o| o.get("events"))
        .and_then(|v| v.as_object())
        .or_else(|| prev.get("events").and_then(|v| v.as_object()));
    let mut events = Map::new();
    for (k, d) in DEFAULT_NOTIFY_EVENT_KEYS {
        let b = raw_events
            .and_then(|m| m.get(k))
            .and_then(|v| v.as_bool())
            .unwrap_or(d);
        events.insert(k.to_string(), Value::Bool(b));
    }

    let enabled = body_obj
        .and_then(|o| o.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| prev.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true));
    let paper_notify = body_obj
        .and_then(|o| o.get("paper_notify"))
        .and_then(|v| v.as_bool())
        .unwrap_or_else(|| {
            prev.get("paper_notify")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });

    let notif = json!({
        "enabled": enabled,
        "paper_notify": paper_notify,
        "telegram_bot_token_enc": tg_enc,
        "telegram_bot_token_masked": tg_masked,
        "telegram_chat_id": chat_id,
        "discord_webhook_enc": dc_enc,
        "discord_webhook_url_masked": dc_masked,
        "slack_webhook_enc": sl_enc,
        "slack_webhook_url_masked": sl_masked,
        "events": Value::Object(events),
        "updated_at": now_iso(),
    });
    if let Some(obj) = rec.as_object_mut() {
        obj.insert("notifications".to_string(), notif);
    }
    state.store.put(CREDENTIALS, user_id, rec);
    Ok(public_notifications_view(state, user_id))
}

pub fn get_notifications(state: &AppState, user_id: &str) -> Option<NotificationConfig> {
    let rec = state.store.get(CREDENTIALS, user_id)?;
    let n = rec.get("notifications")?.as_object()?;
    let tg = n
        .get("telegram_bot_token_enc")
        .and_then(|v| v.as_str())
        .and_then(|s| state.crypto.decrypt(s).ok());
    let dc = n
        .get("discord_webhook_enc")
        .and_then(|v| v.as_str())
        .and_then(|s| state.crypto.decrypt(s).ok());
    let sl = n
        .get("slack_webhook_enc")
        .and_then(|v| v.as_str())
        .and_then(|s| state.crypto.decrypt(s).ok());
    let chat_id = n
        .get("telegram_chat_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if !(tg.is_some() && !chat_id.is_empty()) && dc.is_none() && sl.is_none() {
        return None;
    }

    let mut events = HashMap::new();
    let stored_ev = n.get("events").and_then(|v| v.as_object());
    for (k, d) in DEFAULT_NOTIFY_EVENT_KEYS {
        let v = stored_ev
            .and_then(|m| m.get(k))
            .and_then(|x| x.as_bool())
            .unwrap_or(d);
        events.insert(k.to_string(), v);
    }

    Some(NotificationConfig {
        enabled: n.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        paper_notify: n
            .get("paper_notify")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        telegram_bot_token: tg,
        telegram_chat_id: chat_id,
        discord_webhook_url: dc,
        slack_webhook_url: sl,
        events,
    })
}

pub fn delete_notifications(state: &AppState, user_id: &str) -> Value {
    if let Some(mut rec) = state.store.get(CREDENTIALS, user_id) {
        if rec
            .as_object_mut()
            .and_then(|m| m.remove("notifications"))
            .is_some()
        {
            state.store.put(CREDENTIALS, user_id, rec);
        }
    }
    public_notifications_view(state, user_id)
}

pub fn public_notifications_view(state: &AppState, user_id: &str) -> Value {
    let Some(rec) = state.store.get(CREDENTIALS, user_id) else {
        return empty_notifications_view();
    };
    let Some(n) = rec.get("notifications").and_then(|v| v.as_object()) else {
        return empty_notifications_view();
    };
    let tg_ok = n
        .get("telegram_bot_token_enc")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
        && n.get("telegram_chat_id")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false);
    let dc_ok = n
        .get("discord_webhook_enc")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let sl_ok = n
        .get("slack_webhook_enc")
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty())
        .unwrap_or(false);

    let mut events = default_events_map();
    if let Some(ev) = n.get("events").and_then(|v| v.as_object()) {
        for (k, _) in DEFAULT_NOTIFY_EVENT_KEYS {
            if let Some(v) = ev.get(k).and_then(|x| x.as_bool()) {
                events.insert(k.to_string(), Value::Bool(v));
            }
        }
    }

    json!({
        "configured": tg_ok || dc_ok || sl_ok,
        "enabled": n.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
        "paper_notify": n.get("paper_notify").and_then(|v| v.as_bool()).unwrap_or(false),
        "telegram_configured": tg_ok,
        "telegram_bot_token_masked": n.get("telegram_bot_token_masked").cloned().unwrap_or(Value::Null),
        "telegram_chat_id": n.get("telegram_chat_id").and_then(|v| v.as_str()).unwrap_or(""),
        "discord_configured": dc_ok,
        "discord_webhook_masked": n.get("discord_webhook_url_masked").cloned().unwrap_or(Value::Null),
        "slack_configured": sl_ok,
        "slack_webhook_masked": n.get("slack_webhook_url_masked").cloned().unwrap_or(Value::Null),
        "events": Value::Object(events),
        "updated_at": n.get("updated_at").cloned().unwrap_or(Value::Null),
    })
}

fn empty_notifications_view() -> Value {
    json!({
        "configured": false,
        "enabled": true,
        "paper_notify": false,
        "telegram_configured": false,
        "telegram_bot_token_masked": Value::Null,
        "telegram_chat_id": "",
        "discord_configured": false,
        "discord_webhook_masked": Value::Null,
        "slack_configured": false,
        "slack_webhook_masked": Value::Null,
        "events": Value::Object(default_events_map()),
        "updated_at": Value::Null,
    })
}

pub async fn deliver_notification(
    state: &AppState,
    cfg: &NotificationConfig,
    text: &str,
) -> (Vec<String>, HashMap<String, String>) {
    let mut sent = Vec::new();
    let mut errors = HashMap::new();

    if let (Some(tok), false) = (&cfg.telegram_bot_token, cfg.telegram_chat_id.is_empty()) {
        let tg_url = format!("https://api.telegram.org/bot{tok}/sendMessage");
        let payload = json!({ "chat_id": cfg.telegram_chat_id, "text": text });
        match state.notifier_hook.send_http(&tg_url, &payload).await {
            Ok(()) => sent.push("telegram".to_string()),
            Err(e) => {
                errors.insert("telegram".to_string(), e.chars().take(160).collect());
            }
        }
    }
    if let Some(url) = &cfg.discord_webhook_url {
        let preview: String = text.chars().take(1950).collect();
        let payload = json!({ "content": preview });
        match state.notifier_hook.send_http(url, &payload).await {
            Ok(()) => sent.push("discord".to_string()),
            Err(e) => {
                errors.insert("discord".to_string(), e.chars().take(160).collect());
            }
        }
    }
    if let Some(url) = &cfg.slack_webhook_url {
        let payload = json!({ "text": text });
        match state.notifier_hook.send_http(url, &payload).await {
            Ok(()) => sent.push("slack".to_string()),
            Err(e) => {
                errors.insert("slack".to_string(), e.chars().take(160).collect());
            }
        }
    }
    (sent, errors)
}

pub async fn send_test_notification(state: &AppState, user_id: &str) -> ApiResult<Value> {
    let cfg = get_notifications(state, user_id).ok_or_else(|| {
        ApiError::bad_request(
            "notifications_not_configured",
            "먼저 텔레그램 또는 디스코드·슬랙 웹훅 주소를 저장하세요",
        )
    })?;
    let text = "🔔 [Mighty Trade 알림 연결 테스트]\n실시간 매매·체결·안전장치 알림과 장 마감 일일 브리핑이 이 채널로 전송됩니다.";
    let (sent, errors) = deliver_notification(state, &cfg, text).await;
    if sent.is_empty() {
        let detail = if errors.is_empty() {
            "전송 가능한 채널 없음".to_string()
        } else {
            errors
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        return Err(ApiError::bad_gateway(
            "notification_send_failed",
            format!("테스트 알림 전송에 실패했습니다 ({detail})"),
        ));
    }
    Ok(json!({ "sent": sent, "errors": errors }))
}
