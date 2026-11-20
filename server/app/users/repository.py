from __future__ import annotations

import uuid
from datetime import datetime, timezone
from typing import Any, Optional

from flask import current_app

from ..storage import get_store

USERS = "users"


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def get_user(user_id: str) -> Optional[dict[str, Any]]:
    return get_store().get(USERS, user_id)


def get_by_google_sub(sub: str) -> Optional[dict[str, Any]]:
    return get_store().find_one(USERS, lambda u: u.get("google_sub") == sub)


STATUSES = ("pending", "approved", "rejected")


def is_admin(user: Optional[dict[str, Any]]) -> bool:
    return bool(user) and (user.get("email") or "").lower() in current_app.config["ADMIN_EMAILS"]


def status_of(user: dict[str, Any]) -> str:
    """Accounts created before sign-up approval existed have no status: treat them as approved."""
    return user.get("status") or "approved"


def upsert_from_google(info: dict[str, Any]) -> dict[str, Any]:
    """Create on first Google login (status pending until an admin approves; admins are approved)."""
    admin = (info.get("email") or "").lower() in current_app.config["ADMIN_EMAILS"]
    user = get_by_google_sub(info["sub"])
    if user is None:
        user = {
            "google_sub": info["sub"],
            "status": "approved" if admin else "pending",
            "approved_at": now_iso() if admin else None,
            "approved_by": "admin-email" if admin else None,
            "otp_enabled": False,
            "otp_secret_enc": None,
            "otp_pending_secret_enc": None,
            "otp_fail_count": 0,
            "otp_locked_until": None,
            "created_at": now_iso(),
        }
        user_id = str(uuid.uuid4())
    else:
        user_id = user["id"]
        if admin and status_of(user) != "approved":
            user.update(status="approved", approved_at=now_iso(), approved_by="admin-email")
    user.update(
        email=info.get("email"),
        name=info.get("name"),
        picture=info.get("picture"),
        last_login_at=now_iso(),
    )
    return get_store().put(USERS, user_id, user)


def update_user(user_id: str, **fields: Any) -> dict[str, Any]:
    user = get_user(user_id)
    if user is None:
        raise KeyError(user_id)
    user.update(fields)
    return get_store().put(USERS, user_id, user)


def public_view(user: dict[str, Any]) -> dict[str, Any]:
    return {
        "id": user["id"],
        "email": user.get("email"),
        "name": user.get("name"),
        "picture": user.get("picture"),
        "otp_enabled": bool(user.get("otp_enabled")),
        "status": status_of(user),
        "role": "admin" if is_admin(user) else "user",
    }
