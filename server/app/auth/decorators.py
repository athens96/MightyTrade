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
                raise ApiError(401, "unauthorized")
            if stage == "session" and not user.get("otp_enabled"):
                raise ApiError(401, "otp_required")
            g.user = user
            return fn(*args, **kwargs)

        return wrapper

    return decorator


login_required = _require("session")
pending_required = _require("pending")
