"""Two-stage session tokens stored in httpOnly cookies.

- pending: issued after Google login, only allows OTP setup/verify.
- session: issued after OTP success, required by every protected API.
"""
from __future__ import annotations

from datetime import datetime, timedelta, timezone
from typing import Literal, Optional

import jwt
from flask import Response, current_app, request

Stage = Literal["pending", "session"]
COOKIE = {"pending": "mt_pending", "session": "mt_session"}
COOKIE_PATH = {"pending": "/api/auth", "session": "/api"}


def _ttl(stage: Stage) -> timedelta:
    key = "PENDING_TOKEN_TTL_MIN" if stage == "pending" else "ACCESS_TOKEN_TTL_MIN"
    return timedelta(minutes=current_app.config[key])


def issue(resp: Response, user_id: str, stage: Stage) -> None:
    now = datetime.now(timezone.utc)
    ttl = _ttl(stage)
    token = jwt.encode(
        {"sub": user_id, "stage": stage, "iat": now, "exp": now + ttl},
        current_app.config["SECRET_KEY"],
        algorithm="HS256",
    )
    resp.set_cookie(
        COOKIE[stage],
        token,
        max_age=int(ttl.total_seconds()),
        httponly=True,
        secure=current_app.config["COOKIE_SECURE"],
        samesite="Strict",
        path=COOKIE_PATH[stage],
    )


def clear(resp: Response, stage: Stage) -> None:
    resp.delete_cookie(COOKIE[stage], path=COOKIE_PATH[stage])


def read(stage: Stage) -> Optional[str]:
    """Return the user id from the cookie for `stage`, or None if missing/invalid."""
    token = request.cookies.get(COOKIE[stage])
    if not token:
        return None
    try:
        payload = jwt.decode(token, current_app.config["SECRET_KEY"], algorithms=["HS256"])
    except jwt.PyJWTError:
        return None
    if payload.get("stage") != stage:
        return None
    return payload.get("sub")
