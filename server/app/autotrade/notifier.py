"""Real-time trade notifications (Telegram, Discord, Slack) and end-of-day briefing (MT-2).

All network calls use short timeouts and swallow transport errors so notification outages can NEVER
interrupt order placement, fills, or the scheduler loop.
"""
from __future__ import annotations

import logging
from typing import Any, Optional

import requests

from ..errors import ApiError
from ..storage import get_store
from ..users import credentials
from ..users.repository import now_iso

log = logging.getLogger(__name__)

NOTIFY_TIMEOUT_SEC = 4.0
BRIEFINGS = "bot_briefings"

ICON_BY_KIND = {
    "order": "📤",
    "approval": "✋",
    "fill": "✅",
    "error": "⚠️",
    "skip": "ℹ️",
    "halt": "🛑",
    "protection": "🛡️",
    "briefing": "📊",
}

EVENT_BUCKET = {
    "order": "order",
    "approval": "order",
    "fill": "fill",
    "error": "error",
    "skip": "error",
    "halt": "halt",
    "protection": "halt",
    "briefing": "briefing",
}


def _send_http(url: str, payload: dict[str, Any]) -> None:
    resp = requests.post(url, json=payload, timeout=NOTIFY_TIMEOUT_SEC)
    if resp.status_code >= 400:
        raise RuntimeError(f"HTTP {resp.status_code}: {resp.text[:120]}")


def _deliver(cfg: dict[str, Any], text: str) -> tuple[list[str], dict[str, str]]:
    sent: list[str] = []
    errors: dict[str, str] = {}
    if cfg.get("telegram_bot_token") and cfg.get("telegram_chat_id"):
        try:
            tg_url = f"https://api.telegram.org/bot{cfg['telegram_bot_token']}/sendMessage"
            _send_http(tg_url, {"chat_id": cfg["telegram_chat_id"], "text": text})
            sent.append("telegram")
        except Exception as e:  # noqa: BLE001
            errors["telegram"] = str(e)[:160]
    if cfg.get("discord_webhook_url"):
        try:
            _send_http(cfg["discord_webhook_url"], {"content": text[:1950]})
            sent.append("discord")
        except Exception as e:  # noqa: BLE001
            errors["discord"] = str(e)[:160]
    if cfg.get("slack_webhook_url"):
        try:
            _send_http(cfg["slack_webhook_url"], {"text": text})
            sent.append("slack")
        except Exception as e:  # noqa: BLE001
            errors["slack"] = str(e)[:160]
    return sent, errors


def dispatch(
    user_id: str,
    kind: str,
    title: str,
    body: str,
    *,
    mode: str = "live",
    level: str = "info",
) -> dict[str, Any]:
    cfg = credentials.get_notifications(user_id)
    if not cfg or not cfg.get("enabled", True):
        return {"sent": [], "skipped": "disabled"}
    bucket = EVENT_BUCKET.get(kind)
    if not bucket or not cfg["events"].get(bucket, True):
        return {"sent": [], "skipped": "event_off"}
    if kind == "skip" and level == "info":
        return {"sent": [], "skipped": "info_skip"}
    if mode == "paper" and kind not in ("halt", "briefing") and not cfg.get("paper_notify", False):
        return {"sent": [], "skipped": "paper_muted"}

    icon = ICON_BY_KIND.get(kind, "🔔")
    mode_tag = "모의" if mode == "paper" else ("반자동" if mode == "semi" else "실거래")
    prefix = f"{icon} [Mighty Trade · {mode_tag}] {title}" if kind != "briefing" else f"{icon} [Mighty Trade] {title}"
    text = f"{prefix}\n{body}".strip()
    sent, errors = _deliver(cfg, text)
    if errors:
        log.warning("notification delivery errors for %s: %s", user_id[:8], errors)
    return {"sent": sent, "errors": errors}


def notify_bot_log(bot: dict[str, Any], kind: str, message: str, level: str = "info") -> None:
    if kind not in EVENT_BUCKET:
        return
    try:
        title = f"{bot.get('name') or bot.get('symbol') or '자동매매 봇'} ({bot.get('market', '')})"
        dispatch(bot["user_id"], kind, title, message, mode=bot.get("mode", "paper"), level=level)
    except Exception:  # noqa: BLE001
        log.debug("notify_bot_log suppressed error", exc_info=True)


def send_test(user_id: str) -> dict[str, Any]:
    cfg = credentials.get_notifications(user_id)
    if not cfg:
        raise ApiError(400, "notifications_not_configured", "먼저 텔레그램 또는 디스코드·슬랙 웹훅 주소를 저장하세요")
    text = (
        "🔔 [Mighty Trade 알림 연결 테스트]\n"
        "실시간 매매·체결·안전장치 알림과 장 마감 일일 브리핑이 이 채널로 전송됩니다."
    )
    sent, errors = _deliver(cfg, text)
    if not sent:
        detail = ", ".join(f"{k}: {v}" for k, v in errors.items()) or "전송 가능한 채널 없음"
        raise ApiError(502, "notification_send_failed", f"테스트 알림 전송에 실패했습니다 ({detail})")
    return {"sent": sent, "errors": errors}


def build_daily_briefing_text(user_id: str, market: str, session_date: str) -> Optional[str]:
    from . import bots, executor

    user_bots = [b for b in bots.list_for(user_id) if b.get("market") == market]
    if not user_bots:
        return None
    enabled_bots = [b for b in user_bots if b.get("enabled")]
    if not enabled_bots:
        return None

    unit = "원" if market == "KR" else "USD"
    m_label = "국내(KRX)" if market == "KR" else "미국(US)"
    lines = [f"📅 {session_date} {m_label} 장 마감 일일 브리핑 (가동 봇 {len(enabled_bots)}개)"]
    total_cap = 0.0
    total_eq = 0.0

    for b in enabled_bots:
        pub = bots.public(b)
        st = pub["state"]
        cap = float(b["capital"])
        eq_list = st.get("equity") or []
        eq_val = float(st.get("equity_now") if b["mode"] == "paper" and st.get("equity_now") is not None else (eq_list[-1]["value"] if eq_list else cap))
        pnl_pct = ((eq_val / cap) - 1.0) * 100.0 if cap > 0 else 0.0
        total_cap += cap
        total_eq += eq_val

        held_desc = "미보유"
        if pub["kind"] in ("rebalance", "rotation"):
            cnt = sum(1 for p in (st.get("positions") or {}).values() if (p.get("qty") or 0) > 0)
            held_desc = f"{cnt}종목 보유" if cnt else "현금 대기"
        elif st["position"]["qty"] > 0:
            held_desc = f"{st['position']['qty']:g}주 보유"

        next_hint = ""
        try:
            pv = executor.preview(b, session_date)
            if pv.get("orders"):
                next_hint = f" · 다음 신호: {pv['orders'][0]['text'][:32]}"
        except Exception:  # noqa: BLE001
            pass

        mode_str = "모의" if b["mode"] == "paper" else ("반자동" if b["mode"] == "semi" else "실거래")
        lines.append(f"• [{mode_str}] {b['name']}: {pnl_pct:+.2f}% ({held_desc}){next_hint}")

    if total_cap > 0:
        tot_pct = ((total_eq / total_cap) - 1.0) * 100.0
        diff = total_eq - total_cap
        fmt_diff = f"{diff:+,.0f}{unit}" if market == "KR" else f"{diff:+,.2f} {unit}"
        lines.insert(1, f"합산 성과: {fmt_diff} ({tot_pct:+.2f}%)")
    return "\n".join(lines)


def maybe_send_daily_briefing(user_id: str, market: str, session_date: str) -> bool:
    cfg = credentials.get_notifications(user_id)
    if not cfg or not cfg.get("enabled", True) or not cfg["events"].get("briefing", True):
        return False
    store = get_store()
    rec_id = f"{user_id}:{market}"
    rec = store.get(BRIEFINGS, rec_id) or {}
    if rec.get("last_date") == session_date:
        return False
    body = build_daily_briefing_text(user_id, market, session_date)
    if not body:
        return False
    store.put(BRIEFINGS, rec_id, {"user_id": user_id, "market": market, "last_date": session_date, "sent_at": now_iso()})
    m_label = "국내장" if market == "KR" else "미국장"
    res = dispatch(user_id, "briefing", f"{m_label} 마감 봇 성적표", body, mode="live")
    return bool(res.get("sent"))
