//! Real-time trade notifications (Telegram, Discord, Slack) and end-of-day briefing (MT-2).

use serde_json::{json, Value};

use crate::autotrade::{bots, executor};
use crate::state::AppState;
use crate::users::{credentials, repository::now_iso};

pub const BRIEFINGS: &str = "bot_briefings";

fn icon_for(kind: &str) -> &'static str {
    match kind {
        "order" => "📤",
        "approval" => "✋",
        "fill" => "✅",
        "error" => "⚠️",
        "skip" => "ℹ️",
        "halt" => "🛑",
        "protection" => "🛡️",
        "briefing" => "📊",
        _ => "🔔",
    }
}

fn bucket_for(kind: &str) -> Option<&'static str> {
    match kind {
        "order" | "approval" => Some("order"),
        "fill" => Some("fill"),
        "error" | "skip" => Some("error"),
        "halt" | "protection" => Some("halt"),
        "briefing" => Some("briefing"),
        _ => None,
    }
}

pub async fn dispatch(
    state: &AppState,
    user_id: &str,
    kind: &str,
    title: &str,
    body: &str,
    mode: &str,
    level: &str,
) -> Value {
    let Some(cfg) = credentials::get_notifications(state, user_id) else {
        return json!({ "sent": [], "skipped": "disabled" });
    };
    if !cfg.enabled {
        return json!({ "sent": [], "skipped": "disabled" });
    }
    let Some(bucket) = bucket_for(kind) else {
        return json!({ "sent": [], "skipped": "event_off" });
    };
    if !cfg.events.get(bucket).copied().unwrap_or(true) {
        return json!({ "sent": [], "skipped": "event_off" });
    }
    if kind == "skip" && level == "info" {
        return json!({ "sent": [], "skipped": "info_skip" });
    }
    if mode == "paper" && kind != "halt" && kind != "briefing" && !cfg.paper_notify {
        return json!({ "sent": [], "skipped": "paper_muted" });
    }

    let icon = icon_for(kind);
    let mode_tag = match mode {
        "paper" => "모의",
        "semi" => "반자동",
        _ => "실거래",
    };
    let prefix = if kind != "briefing" {
        format!("{icon} [Mighty Trade · {mode_tag}] {title}")
    } else {
        format!("{icon} [Mighty Trade] {title}")
    };
    let text = format!("{prefix}\n{body}").trim().to_string();
    let (sent, errors) = credentials::deliver_notification(state, &cfg, &text).await;
    if !errors.is_empty() {
        tracing::warn!(
            "notification delivery errors for {}: {:?}",
            &user_id[..user_id.len().min(8)],
            errors
        );
    }
    json!({ "sent": sent, "errors": errors })
}

pub async fn notify_bot_log(state: &AppState, bot: &Value, kind: &str, message: &str, level: &str) {
    if bucket_for(kind).is_none() {
        return;
    }
    let uid = bot["user_id"].as_str().unwrap_or("");
    let name = bot
        .get("name")
        .and_then(|v| v.as_str())
        .or_else(|| bot.get("symbol").and_then(|v| v.as_str()))
        .unwrap_or("자동매매 봇");
    let market = bot.get("market").and_then(|v| v.as_str()).unwrap_or("");
    let mode = bot.get("mode").and_then(|v| v.as_str()).unwrap_or("paper");
    let title = format!("{name} ({market})");
    let _ = dispatch(state, uid, kind, &title, message, mode, level).await;
}

pub async fn build_daily_briefing_text(
    state: &AppState,
    user_id: &str,
    market: &str,
    session_date: &str,
) -> Option<String> {
    let enabled_bots: Vec<Value> = bots::list_for(state, user_id)
        .into_iter()
        .filter(|b| {
            b.get("market").and_then(|v| v.as_str()) == Some(market)
                && b.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)
        })
        .collect();
    if enabled_bots.is_empty() {
        return None;
    }

    let unit = if market == "KR" { "원" } else { "USD" };
    let m_label = if market == "KR" { "국내(KRX)" } else { "미국(US)" };
    let mut lines = vec![format!(
        "📅 {session_date} {m_label} 장 마감 일일 브리핑 (가동 봇 {}개)",
        enabled_bots.len()
    )];
    let mut total_cap = 0.0;
    let mut total_eq = 0.0;

    for b in &enabled_bots {
        let pub_b = bots::public(b);
        let st = &pub_b["state"];
        let cap = b["capital"].as_f64().unwrap_or(0.0);
        let mode = b["mode"].as_str().unwrap_or("paper");
        let eq_val = if mode == "paper" {
            st.get("equity_now").and_then(|v| v.as_f64())
        } else {
            None
        }
        .or_else(|| {
            st.get("equity")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|e| e.get("value"))
                .and_then(|v| v.as_f64())
        })
        .unwrap_or(cap);
        let pnl_pct = if cap > 0.0 {
            (eq_val / cap - 1.0) * 100.0
        } else {
            0.0
        };
        total_cap += cap;
        total_eq += eq_val;

        let kind = pub_b["kind"].as_str().unwrap_or("single");
        let held_desc = if kind == "rebalance" || kind == "rotation" {
            let cnt = st
                .get("positions")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.values()
                        .filter(|p| p.get("qty").and_then(|v| v.as_f64()).unwrap_or(0.0) > 0.0)
                        .count()
                })
                .unwrap_or(0);
            if cnt > 0 {
                format!("{cnt}종목 보유")
            } else {
                "현금 대기".to_string()
            }
        } else {
            let q = st["position"]["qty"].as_f64().unwrap_or(0.0);
            if q > 0.0 {
                format!("{}주 보유", crate::autotrade::protections::fmt_g(q))
            } else {
                "미보유".to_string()
            }
        };

        let mut next_hint = String::new();
        if let Ok(pv) = executor::preview(state, b, session_date).await {
            if let Some(first_txt) = pv
                .get("orders")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
            {
                let cut: String = first_txt.chars().take(32).collect();
                next_hint = format!(" · 다음 신호: {cut}");
            }
        }
        let mode_str = match mode {
            "paper" => "모의",
            "semi" => "반자동",
            _ => "실거래",
        };
        let bname = b["name"].as_str().unwrap_or("");
        lines.push(format!(
            "• [{mode_str}] {bname}: {pnl_pct:+.2}% ({held_desc}){next_hint}"
        ));
    }

    if total_cap > 0.0 {
        let tot_pct = (total_eq / total_cap - 1.0) * 100.0;
        let diff = total_eq - total_cap;
        let fmt_diff = if market == "KR" {
            format!("{diff:+.0}{unit}")
        } else {
            format!("{diff:+.2} {unit}")
        };
        lines.insert(1, format!("합산 성과: {fmt_diff} ({tot_pct:+.2}%)"));
    }
    Some(lines.join("\n"))
}

pub async fn maybe_send_daily_briefing(
    state: &AppState,
    user_id: &str,
    market: &str,
    session_date: &str,
) -> bool {
    let Some(cfg) = credentials::get_notifications(state, user_id) else {
        return false;
    };
    if !cfg.enabled || !cfg.events.get("briefing").copied().unwrap_or(true) {
        return false;
    }
    let rec_id = format!("{user_id}:{market}");
    if let Some(rec) = state.store.get(BRIEFINGS, &rec_id) {
        if rec.get("last_date").and_then(|v| v.as_str()) == Some(session_date) {
            return false;
        }
    }
    let Some(body) = build_daily_briefing_text(state, user_id, market, session_date).await else {
        return false;
    };
    state.store.put(
        BRIEFINGS,
        &rec_id,
        json!({
            "user_id": user_id,
            "market": market,
            "last_date": session_date,
            "sent_at": now_iso(),
        }),
    );
    let m_label = if market == "KR" { "국내장" } else { "미국장" };
    let res = dispatch(
        state,
        user_id,
        "briefing",
        &format!("{m_label} 마감 봇 성적표"),
        &body,
        "live",
        "info",
    )
    .await;
    res.get("sent")
        .and_then(|v| v.as_array())
        .is_some_and(|a| !a.is_empty())
}
