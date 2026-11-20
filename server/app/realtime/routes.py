from __future__ import annotations

from typing import Optional

from flask import Blueprint, Response, current_app, g, request

from ..account.toss_account import SYMBOL_RE
from ..auth.decorators import login_required
from ..errors import ApiError
from ..market.models import Market
from ..users import credentials
from .hub import Hub

bp = Blueprint("realtime", __name__)


def get_hub() -> Hub:
    ext = current_app.extensions
    if "realtime_hub" not in ext:
        app = current_app._get_current_object()  # type: ignore[attr-defined]

        def token_for(uid: str, rejected: Optional[str]) -> str:
            from ..integrations.toss import get_toss_client
            with app.app_context():
                return get_toss_client(uid).access_token(rejected)

        ext["realtime_hub"] = Hub(token_for)
    return ext["realtime_hub"]


@bp.get("/stream")
@login_required
def stream():
    """SSE: live trades + orderbook for ?market=KR&symbol=005930 (events: status, trade, orderbook)."""
    try:
        market = Market(request.args.get("market", "").upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    symbol = request.args.get("symbol", "").strip().upper()
    if not SYMBOL_RE[market.value].fullmatch(symbol):
        raise ApiError(400, "invalid_symbol")
    if current_app.config["MARKET_PROVIDERS"][market.value] != "toss":
        raise ApiError(501, "requires_toss", "실시간 시세는 토스증권 연동에서만 제공됩니다")
    uid = g.user["id"]
    if not credentials.get_toss(uid):
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키를 먼저 등록하세요 (설정)")

    m = market.value.lower()
    hub = get_hub()
    sub = hub.subscribe(uid, {f"trade:{m}:{symbol}", f"orderbook:{m}:{symbol}"})

    def generate():
        try:
            yield "retry: 3000\n\n"
            yield from sub.events()
        finally:  # browser closed the connection
            hub.unsubscribe(uid, sub)

    return Response(generate(), mimetype="text/event-stream",
                    headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"})


@bp.get("/orders")
@login_required
def order_stream():
    """SSE: the user's own order events (accepted / filled / cancelled / rejected) — event: order."""
    if current_app.config["MARKET_PROVIDERS"]["KR"] != "toss":
        raise ApiError(501, "requires_toss", "토스증권 연동에서만 제공됩니다")
    uid = g.user["id"]
    creds = credentials.get_toss(uid)
    if not creds or creds.get("account_seq") is None:
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키를 먼저 등록하세요 (설정)")
    hub = get_hub()
    sub = hub.subscribe(uid, {f"personal:order:{creds['account_seq']}"})

    def generate():
        try:
            yield "retry: 5000\n\n"
            yield from sub.events()
        finally:
            hub.unsubscribe(uid, sub)

    return Response(generate(), mimetype="text/event-stream",
                    headers={"Cache-Control": "no-cache", "X-Accel-Buffering": "no"})
