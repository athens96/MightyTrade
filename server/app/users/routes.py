"""Per-account data (watchlist, settings). Everything is keyed by the logged-in user id."""
from __future__ import annotations

from flask import Blueprint, g, jsonify, request

from .. import transport_crypto
from ..auth import otp
from ..auth.decorators import login_required
from ..errors import ApiError
from ..integrations.toss import forget_client, verify_credentials
from ..market.models import Market
from ..storage import get_store
from . import credentials
from .repository import now_iso

bp = Blueprint("users", __name__)

WATCHLISTS = "watchlists"
SETTINGS = "settings"
DEFAULT_WATCHLIST = [
    {"market": "KR", "symbol": "005930", "name": "삼성전자"},
    {"market": "KR", "symbol": "000660", "name": "SK하이닉스"},
    {"market": "US", "symbol": "AAPL", "name": "Apple"},
    {"market": "US", "symbol": "NVDA", "name": "NVIDIA"},
]


@bp.get("/me/watchlist")
@login_required
def get_watchlist():
    rec = get_store().get(WATCHLISTS, g.user["id"])
    return jsonify(items=rec["items"] if rec else DEFAULT_WATCHLIST)


@bp.put("/me/watchlist")
@login_required
def put_watchlist():
    items = (request.get_json(silent=True) or {}).get("items")
    if not isinstance(items, list):
        raise ApiError(400, "items_required")
    clean = []
    for it in items:
        try:
            clean.append(
                {"market": Market(it["market"]).value, "symbol": str(it["symbol"]).strip().upper(),
                 "name": str(it.get("name") or "")}
            )
        except (KeyError, ValueError, TypeError):
            raise ApiError(400, "invalid_item", f"invalid watchlist item: {it!r}")
    rec = get_store().put(WATCHLISTS, g.user["id"], {"items": clean, "updated_at": now_iso()})
    return jsonify(items=rec["items"])


@bp.get("/me/settings")
@login_required
def get_settings():
    rec = get_store().get(SETTINGS, g.user["id"]) or {}
    return jsonify(settings=rec.get("values", {}))


@bp.put("/me/settings")
@login_required
def put_settings():
    values = (request.get_json(silent=True) or {}).get("settings")
    if not isinstance(values, dict):
        raise ApiError(400, "settings_required")
    rec = get_store().put(SETTINGS, g.user["id"], {"values": values, "updated_at": now_iso()})
    return jsonify(settings=rec["values"])


# ------------------------------------------------------------------ broker API keys
@bp.get("/transport-key")
@login_required
def transport_key():
    """RSA public key the client uses to encrypt secrets before sending them (see transport_crypto)."""
    return jsonify(transport_crypto.public_key_info())


@bp.get("/me/broker/toss")
@login_required
def get_toss_credentials():
    return jsonify(credentials.public_view(g.user["id"]))


@bp.put("/me/broker/toss")
@login_required
def put_toss_credentials():
    """Body: {"envelope": {...}} encrypting {"client_id", "client_secret", "otp_code", "iat"}."""
    uid = g.user["id"]
    payload = transport_crypto.open_envelope((request.get_json(silent=True) or {}).get("envelope"), aad=uid)

    # Step-up auth: these keys can place real orders, so require a fresh OTP code.
    otp.verify(g.user, str(payload.get("otp_code", "")))

    client_id = str(payload.get("client_id") or "").strip()
    client_secret = str(payload.get("client_secret") or "").strip()
    if not (0 < len(client_id) <= 200 and 0 < len(client_secret) <= 500):
        raise ApiError(400, "invalid_credentials", "client_id / client_secret을 입력하세요")

    previous = credentials.get_toss(uid)
    account_seq = verify_credentials(client_id, client_secret)  # real token issue + account lookup
    credentials.save_toss(uid, client_id, client_secret, account_seq)
    forget_client(uid, previous["client_id"] if previous and previous["client_id"] != client_id else None)
    return jsonify(credentials.public_view(uid))


@bp.delete("/me/broker/toss")
@login_required
def delete_toss_credentials():
    uid = g.user["id"]
    previous = credentials.get_toss(uid)
    credentials.delete_toss(uid)
    forget_client(uid, previous["client_id"] if previous else None)
    return jsonify(credentials.public_view(uid))
