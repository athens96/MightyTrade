"""Manual trading API. Orders default to a dry run; sending needs the trading unlock (OTP, 30 min)."""
from __future__ import annotations

from flask import Blueprint, g, jsonify, request

from ..auth.decorators import login_required
from ..errors import ApiError
from ..users import repository as users
from . import orders

bp = Blueprint("trading", __name__)


def _fresh_user() -> dict:
    return users.get_user(g.user["id"]) or g.user


@bp.get("/status")
@login_required
def status():
    return jsonify(orders.trading_status(_fresh_user()))


@bp.post("/unlock")
@login_required
def unlock():
    code = str((request.get_json(silent=True) or {}).get("otp_code", ""))
    return jsonify(orders.unlock(_fresh_user(), code))


@bp.post("/lock")
@login_required
def lock():
    return jsonify(orders.lock(_fresh_user()))


@bp.post("/orders")
@login_required
def place_order():
    """Body: market, symbol, side, order_type, quantity, price?, time_in_force?, confirm_high_value?, dry_run?"""
    body = request.get_json(silent=True) or {}
    built = orders.build_order(body)
    if body.get("dry_run", True) is not False:
        return jsonify(dry_run=True, **built)
    orders.require_unlocked(_fresh_user())
    result = orders.submit(built, g.user["id"], source="manual")
    return jsonify(dry_run=False, result=result, **built)


@bp.get("/orders")
@login_required
def list_orders():
    status = request.args.get("status", "OPEN").upper()
    if status not in ("OPEN", "CLOSED"):
        raise ApiError(400, "invalid_status")
    symbol = request.args.get("symbol", "").strip().upper() or None
    return jsonify(items=orders.list_orders(status, symbol))


@bp.get("/orders/<path:order_id>")
@login_required
def get_order(order_id: str):
    return jsonify(order=orders.get_order(order_id))


@bp.post("/orders/<path:order_id>/cancel")
@login_required
def cancel_order(order_id: str):
    """Cancelling only reduces risk, so it doesn't need the trading unlock."""
    return jsonify(result=orders.cancel(order_id))


@bp.post("/orders/<path:order_id>/modify")
@login_required
def modify_order(order_id: str):
    orders.require_unlocked(_fresh_user())
    return jsonify(result=orders.modify(order_id, request.get_json(silent=True) or {}))
