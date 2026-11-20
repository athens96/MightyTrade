from __future__ import annotations

from flask import Blueprint, g, jsonify, request

from ..auth import otp
from ..auth.decorators import login_required
from ..errors import ApiError
from ..market.models import Market
from ..storage import get_store
from ..trading import orders as trading
from ..users.repository import now_iso
from . import toss_account

bp = Blueprint("account", __name__)


@bp.get("/summary")
@login_required
def summary():
    """Holdings with P&L (Toss computes it, incl. after-cost figures) and cash buying power (KRW/USD)."""
    return jsonify(toss_account.summary())


@bp.get("/conditional-orders")
@login_required
def list_conditional():
    status = request.args.get("status", "OPEN").upper()
    if status not in ("OPEN", "CLOSED"):
        raise ApiError(400, "invalid_status")
    return jsonify(items=toss_account.conditional_orders(status))


@bp.post("/conditional-orders/oco")
@login_required
def create_oco():
    """Take-profit / stop-loss bracket on a holding.

    Default is a dry run: validate + return the exact payload, send nothing. Sending a real order needs
    {"dry_run": false, "otp_code": "..."} — a fresh OTP code (codes are single-use).
    """
    body = request.get_json(silent=True) or {}
    built = toss_account.build_oco(body)
    if body.get("dry_run", True) is not False:
        return jsonify(dry_run=True, payload=built["payload"], checks=built["checks"])

    otp.verify(g.user, str(body.get("otp_code", "")))
    trading.check_limits(g.user["id"], Market(built["market"]), built["checks"]["order_value"])
    result = toss_account.submit_oco(built)
    p = built["payload"]
    get_store().put("orders", p["clientOrderId"], {
        "user_id": g.user["id"], "kind": "conditional_oco", "source": "manual_oco",
        "market": built["market"], "symbol": p["symbol"], "side": "SELL",
        "payload": p, "result": result, "date": trading.kst_today(), "created_at": now_iso(),
    })
    return jsonify(dry_run=False, payload=p, checks=built["checks"], result=result)


@bp.delete("/conditional-orders/<order_id>")
@login_required
def cancel_conditional(order_id: str):
    toss_account.cancel_conditional(order_id)
    return jsonify(ok=True)
