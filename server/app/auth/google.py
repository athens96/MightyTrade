from __future__ import annotations

from typing import Any

from flask import current_app
from google.auth.transport import requests as google_requests
from google.oauth2 import id_token

from ..errors import ApiError


def verify_google_credential(credential: str) -> dict[str, Any]:
    """Verify a Google Identity Services ID token (the `credential` from the client)."""
    client_id = current_app.config["GOOGLE_CLIENT_ID"]
    if not client_id:
        raise ApiError(500, "google_not_configured", "GOOGLE_CLIENT_ID is not set")
    try:
        info = id_token.verify_oauth2_token(credential, google_requests.Request(), client_id)
    except ValueError as e:
        raise ApiError(401, "invalid_google_token", str(e)) from e
    if not info.get("email_verified"):
        raise ApiError(403, "email_not_verified")

    allowed = current_app.config["ALLOWED_EMAILS"]
    if allowed and info.get("email", "").lower() not in allowed:
        raise ApiError(403, "email_not_allowed")
    return info
