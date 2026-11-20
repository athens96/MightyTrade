from __future__ import annotations

from flask import Blueprint, g, jsonify, request

from ..errors import ApiError
from ..users import repository as users
from . import otp, tokens
from .decorators import login_required, pending_required
from .google import verify_google_credential

bp = Blueprint("auth", __name__)


@bp.post("/google")
def google_login():
    """Step 1: Google ID token -> pending session. Client must then do OTP."""
    credential = (request.get_json(silent=True) or {}).get("credential")
    if not credential:
        raise ApiError(400, "credential_required")
    info = verify_google_credential(credential)
    user = users.upsert_from_google(info)

    status = users.status_of(user)
    if status != "approved":
        # Not usable yet: no session or OTP step until an admin approves the sign-up.
        resp = jsonify(next="approval_pending" if status == "pending" else "rejected", user=users.public_view(user))
        tokens.clear(resp, "pending")
        tokens.clear(resp, "session")
        return resp

    resp = jsonify(
        next="otp_verify" if user.get("otp_enabled") else "otp_setup",
        user=users.public_view(user),
    )
    tokens.clear(resp, "session")
    tokens.issue(resp, user["id"], "pending")
    return resp


@bp.post("/otp/setup")
@pending_required
def otp_setup():
    """Step 2a (first login only): returns otpauth URL to render as QR."""
    return jsonify(otp.begin_setup(g.user))


@bp.post("/otp/verify")
@pending_required
def otp_verify():
    """Step 2b: valid TOTP code -> full session."""
    code = (request.get_json(silent=True) or {}).get("code", "")
    user = otp.verify(g.user, code)
    resp = jsonify(user=users.public_view(user))
    tokens.clear(resp, "pending")
    tokens.issue(resp, user["id"], "session")
    return resp


@bp.get("/me")
@login_required
def me():
    return jsonify(user=users.public_view(g.user))


@bp.post("/logout")
def logout():
    resp = jsonify(ok=True)
    tokens.clear(resp, "pending")
    tokens.clear(resp, "session")
    return resp
