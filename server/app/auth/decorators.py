from __future__ import annotations

from functools import wraps
from typing import Callable

from flask import g

from ..errors import ApiError
from ..users import repository as users
from . import tokens


def _require(stage: tokens.Stage) -> Callable:
    def decorator(fn: Callable) -> Callable:
        @wraps(fn)
        def wrapper(*args, **kwargs):
            user_id = tokens.read(stage)
            user = users.get_user(user_id) if user_id else None
            if user is None:
                raise ApiError(401, "unauthorized", "로그인이 만료되었습니다. 다시 로그인해 주세요")
            if users.status_of(user) != "approved":  # pending / rejected, or approval revoked
                raise ApiError(401, "not_approved", "관리자 승인 후 이용할 수 있습니다")
            if stage == "session" and not user.get("otp_enabled"):
                raise ApiError(401, "otp_required", "OTP 2단계 인증이 필요합니다")
            g.user = user
            return fn(*args, **kwargs)

        return wrapper

    return decorator


login_required = _require("session")
pending_required = _require("pending")


def admin_required(fn: Callable) -> Callable:
    """Logged in (OTP done) AND listed in ADMIN_EMAILS."""
    @wraps(fn)
    @login_required
    def wrapper(*args, **kwargs):
        if not users.is_admin(g.user):
            raise ApiError(403, "admin_only", "관리자만 사용할 수 있습니다")
        return fn(*args, **kwargs)

    return wrapper
