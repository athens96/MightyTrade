"""Per-account broker API credentials, encrypted at rest.

Record (collection `broker_credentials`, id = user id):
    toss: {client_id_enc, client_secret_enc, client_id_masked, account_seq, verified_at, updated_at}

Secrets are only ever decrypted server-side to call the broker. Nothing here is returned to the
client except `public_view()` (masked id + status).
"""
from __future__ import annotations

import ipaddress
import socket
from typing import Any, Optional
from urllib.parse import urlparse

from ..crypto import decrypt, encrypt
from ..errors import ApiError
from ..storage import get_store
from .repository import now_iso

CREDENTIALS = "broker_credentials"

DEFAULT_NOTIFY_EVENTS: dict[str, bool] = {
    "order": True,
    "fill": True,
    "error": True,
    "halt": True,
    "briefing": True,
}


def mask(value: str) -> str:
    if len(value) <= 12:
        return value[:2] + "…"
    return f"{value[:10]}…{value[-4:]}"


def save_toss(user_id: str, client_id: str, client_secret: str, account_seq: Optional[int]) -> dict[str, Any]:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    rec["toss"] = {
        "client_id_enc": encrypt(client_id),
        "client_secret_enc": encrypt(client_secret),
        "client_id_masked": mask(client_id),
        "account_seq": account_seq,
        "verified_at": now_iso(),
        "updated_at": now_iso(),
    }
    return store.put(CREDENTIALS, user_id, rec)["toss"]


def get_toss(user_id: str) -> Optional[dict[str, Any]]:
    """Decrypted credentials for server-side use only. Never serialize this to a response."""
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("toss")
    if not rec:
        return None
    return {
        "client_id": decrypt(rec["client_id_enc"]),
        "client_secret": decrypt(rec["client_secret_enc"]),
        "account_seq": rec.get("account_seq"),
        "version": rec.get("updated_at"),
    }


def delete_toss(user_id: str) -> bool:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    if "toss" not in rec:
        return False
    rec.pop("toss")
    store.put(CREDENTIALS, user_id, rec)
    return True


def public_view(user_id: str) -> dict[str, Any]:
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("toss")
    if not rec:
        return {"configured": False}
    return {
        "configured": True,
        "client_id_masked": rec["client_id_masked"],
        "account_seq": rec.get("account_seq"),
        "verified_at": rec.get("verified_at"),
    }


# ------------------------------------------------------------------ trade notifications (MT-2)
def _validate_webhook_url(url: str, label: str) -> str:
    parsed = urlparse(url)
    if parsed.scheme != "https" or not parsed.hostname:
        raise ApiError(400, "invalid_webhook_url", f"{label} 주소는 https:// 로 시작해야 합니다")
    host = parsed.hostname.lower()
    if host in ("localhost", "localhost.localdomain") or host.endswith(".local") or host.endswith(".internal"):
        raise ApiError(400, "invalid_webhook_url", f"{label} 주소에 내부 네트워크 호스트를 사용할 수 없습니다")
    try:
        ip = ipaddress.ip_address(host)
        if ip.is_private or ip.is_loopback or ip.is_link_local or ip.is_reserved or ip.is_multicast:
            raise ApiError(400, "invalid_webhook_url", f"{label} 주소에 사설 IP를 사용할 수 없습니다")
    except ValueError:
        pass
    return url


def _mask_url(url: str) -> str:
    if len(url) <= 28:
        return url[:14] + "…"
    return f"{url[:24]}…{url[-6:]}"


def save_notifications(user_id: str, body: dict[str, Any]) -> dict[str, Any]:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    prev = rec.get("notifications") or {}

    def resolve_secret(field_in: str, enc_key: str, mask_fn, validator=None) -> tuple[Optional[str], Optional[str]]:
        if field_in not in body or body[field_in] is None or body[field_in] == "__KEEP__":
            return prev.get(enc_key), prev.get(f"{field_in}_masked")
        raw = str(body[field_in]).strip()
        if not raw:
            return None, None
        if validator:
            raw = validator(raw)
        return encrypt(raw), mask_fn(raw)

    tg_enc, tg_masked = resolve_secret("telegram_bot_token", "telegram_bot_token_enc", mask)
    dc_enc, dc_masked = resolve_secret(
        "discord_webhook_url",
        "discord_webhook_enc",
        _mask_url,
        lambda u: _validate_webhook_url(u, "디스코드 웹훅"),
    )
    sl_enc, sl_masked = resolve_secret(
        "slack_webhook_url",
        "slack_webhook_enc",
        _mask_url,
        lambda u: _validate_webhook_url(u, "슬랙 웹훅"),
    )

    chat_id = str(body.get("telegram_chat_id", prev.get("telegram_chat_id") or "")).strip()
    if tg_enc and not chat_id:
        raise ApiError(400, "telegram_chat_id_required", "텔레그램 알림을 받으려면 Chat ID도 함께 입력하세요")

    raw_events = body.get("events") if isinstance(body.get("events"), dict) else (prev.get("events") or {})
    events = {k: bool(raw_events.get(k, d)) for k, d in DEFAULT_NOTIFY_EVENTS.items()}

    rec["notifications"] = {
        "enabled": bool(body.get("enabled", prev.get("enabled", True))),
        "paper_notify": bool(body.get("paper_notify", prev.get("paper_notify", False))),
        "telegram_bot_token_enc": tg_enc,
        "telegram_bot_token_masked": tg_masked,
        "telegram_chat_id": chat_id,
        "discord_webhook_enc": dc_enc,
        "discord_webhook_url_masked": dc_masked,
        "slack_webhook_enc": sl_enc,
        "slack_webhook_url_masked": sl_masked,
        "events": events,
        "updated_at": now_iso(),
    }
    store.put(CREDENTIALS, user_id, rec)
    return public_notifications_view(user_id)


def get_notifications(user_id: str) -> Optional[dict[str, Any]]:
    """Decrypted notification targets for server-side dispatch only."""
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("notifications")
    if not rec:
        return None
    tg = decrypt(rec["telegram_bot_token_enc"]) if rec.get("telegram_bot_token_enc") else None
    dc = decrypt(rec["discord_webhook_enc"]) if rec.get("discord_webhook_enc") else None
    sl = decrypt(rec["slack_webhook_enc"]) if rec.get("slack_webhook_enc") else None
    if not (tg and rec.get("telegram_chat_id")) and not dc and not sl:
        return None
    return {
        "enabled": bool(rec.get("enabled", True)),
        "paper_notify": bool(rec.get("paper_notify", False)),
        "telegram_bot_token": tg,
        "telegram_chat_id": rec.get("telegram_chat_id") or "",
        "discord_webhook_url": dc,
        "slack_webhook_url": sl,
        "events": {**DEFAULT_NOTIFY_EVENTS, **(rec.get("events") or {})},
    }


def delete_notifications(user_id: str) -> dict[str, Any]:
    store = get_store()
    rec = store.get(CREDENTIALS, user_id) or {}
    if "notifications" in rec:
        rec.pop("notifications")
        store.put(CREDENTIALS, user_id, rec)
    return public_notifications_view(user_id)


def public_notifications_view(user_id: str) -> dict[str, Any]:
    rec = (get_store().get(CREDENTIALS, user_id) or {}).get("notifications")
    if not rec:
        return {
            "configured": False,
            "enabled": True,
            "paper_notify": False,
            "telegram_configured": False,
            "telegram_bot_token_masked": None,
            "telegram_chat_id": "",
            "discord_configured": False,
            "discord_webhook_masked": None,
            "slack_configured": False,
            "slack_webhook_masked": None,
            "events": dict(DEFAULT_NOTIFY_EVENTS),
            "updated_at": None,
        }
    tg_ok = bool(rec.get("telegram_bot_token_enc") and rec.get("telegram_chat_id"))
    dc_ok = bool(rec.get("discord_webhook_enc"))
    sl_ok = bool(rec.get("slack_webhook_enc"))
    return {
        "configured": tg_ok or dc_ok or sl_ok,
        "enabled": bool(rec.get("enabled", True)),
        "paper_notify": bool(rec.get("paper_notify", False)),
        "telegram_configured": tg_ok,
        "telegram_bot_token_masked": rec.get("telegram_bot_token_masked"),
        "telegram_chat_id": rec.get("telegram_chat_id") or "",
        "discord_configured": dc_ok,
        "discord_webhook_masked": rec.get("discord_webhook_url_masked"),
        "slack_configured": sl_ok,
        "slack_webhook_masked": rec.get("slack_webhook_url_masked"),
        "events": {**DEFAULT_NOTIFY_EVENTS, **(rec.get("events") or {})},
        "updated_at": rec.get("updated_at"),
    }
