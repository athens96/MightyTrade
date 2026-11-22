from __future__ import annotations

import hmac
from datetime import datetime, timedelta, timezone
from typing import Any, Optional

import pyotp
from flask import current_app

from ..crypto import decrypt, encrypt
from ..errors import ApiError
from ..users import repository as users


def begin_setup(user: dict[str, Any]) -> dict[str, str]:
    """Create a new pending TOTP secret. Becomes active only after the first valid code."""
    if user.get("otp_enabled"):
        raise ApiError(409, "otp_already_enabled")
    secret = pyotp.random_base32()
    users.update_user(user["id"], otp_pending_secret_enc=encrypt(secret))
    uri = pyotp.TOTP(secret).provisioning_uri(
        name=user.get("email") or user["id"], issuer_name=current_app.config["OTP_ISSUER"]
    )
    return {"otpauth_url": uri, "secret": secret}


def _matched_step(totp: pyotp.TOTP, code: str, now: datetime) -> Optional[int]:
    """Time step (30s counter) the code belongs to, allowing ±1 step of clock drift."""
    current = totp.timecode(now)
    for step in (current, current - 1, current + 1):
        if hmac.compare_digest(totp.generate_otp(step), code):
            return step
    return None


def verify(user: dict[str, Any], code: str) -> dict[str, Any]:
    """Verify a TOTP code with lockout. Promotes the pending secret on first success."""
    now = datetime.now(timezone.utc)
    locked_until = user.get("otp_locked_until")
    if locked_until and datetime.fromisoformat(locked_until) > now:
        raise ApiError(429, "otp_locked", f"Too many attempts. Try again after {locked_until}")

    if user.get("otp_enabled"):
        enc = user.get("otp_secret_enc")
    else:
        enc = user.get("otp_pending_secret_enc")
    if not enc:
        raise ApiError(400, "otp_not_setup", "OTP가 아직 등록되지 않았습니다")

    code = (code or "").strip().replace(" ", "")
    step = _matched_step(pyotp.TOTP(decrypt(enc)), code, now) if code.isdigit() else None
    # Replay protection: each 30s code is single-use (reject steps at or before the last accepted one).
    if step is not None and step <= int(user.get("otp_last_step") or -1):
        step = None
    if step is None:
        fails = int(user.get("otp_fail_count") or 0) + 1
        fields: dict[str, Any] = {"otp_fail_count": fails}
        if fails >= current_app.config["OTP_MAX_FAILS"]:
            fields["otp_fail_count"] = 0
            fields["otp_locked_until"] = (
                now + timedelta(minutes=current_app.config["OTP_LOCK_MIN"])
            ).isoformat()
        users.update_user(user["id"], **fields)
        raise ApiError(401, "invalid_otp", "OTP 코드가 올바르지 않거나 이미 사용된 번호입니다")

    fields = {"otp_fail_count": 0, "otp_locked_until": None, "otp_last_step": step}
    if not user.get("otp_enabled"):
        fields.update(otp_enabled=True, otp_secret_enc=enc, otp_pending_secret_enc=None)
    return users.update_user(user["id"], **fields)
