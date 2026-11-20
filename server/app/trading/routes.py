"""Orders / positions / auto-trading API. Endpoints exist so the client can be built
against them; they return 501 until the broker integration is done."""
from __future__ import annotations

from flask import Blueprint, g, jsonify

from ..auth.decorators import login_required
from ..storage import get_store

bp = Blueprint("trading", __name__)


@bp.get("/orders")
@login_required
def list_orders():
    uid = g.user["id"]
    return jsonify(items=get_store().find("orders", lambda o: o.get("user_id") == uid))


@bp.post("/orders")
@login_required
def place_order():
    # TODO: validate OrderRequest -> risk checks -> broker.place_order -> persist
    raise NotImplementedError("Order placement is not implemented yet")


@bp.delete("/orders/<order_id>")
@login_required
def cancel_order(order_id: str):
    raise NotImplementedError("Order cancel is not implemented yet")


@bp.get("/positions")
@login_required
def positions():
    raise NotImplementedError("Positions are not implemented yet")


@bp.get("/strategies")
@login_required
def list_strategies():
    uid = g.user["id"]
    return jsonify(items=get_store().find("strategies", lambda s: s.get("user_id") == uid))
