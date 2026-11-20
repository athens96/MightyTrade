from __future__ import annotations

from flask import Blueprint, jsonify, request

from ..auth.decorators import login_required
from ..errors import ApiError
from .models import Interval, Market
from .registry import get_provider

bp = Blueprint("market", __name__)


def _market() -> Market:
    try:
        return Market(request.args.get("market", "").upper())
    except ValueError:
        raise ApiError(400, "invalid_market", "market must be KR or US")


def _symbol() -> str:
    symbol = request.args.get("symbol", "").strip().upper()
    if not symbol or len(symbol) > 20:
        raise ApiError(400, "invalid_symbol")
    return symbol


@bp.get("/candles")
@login_required
def candles():
    market, symbol = _market(), _symbol()
    try:
        interval = Interval(request.args.get("interval", "1d"))
    except ValueError:
        raise ApiError(400, "invalid_interval", f"one of {[i.value for i in Interval]}")
    limit = min(max(request.args.get("limit", 200, type=int), 1), 1000)
    provider = get_provider(market)
    data = provider.get_candles(market, symbol, interval, limit)
    return jsonify(
        market=market.value, symbol=symbol, interval=interval.value, provider=provider.name,
        candles=[c.to_dict() for c in data],
    )


@bp.get("/quote")
@login_required
def quote():
    market, symbol = _market(), _symbol()
    return jsonify(quote=get_provider(market).get_quote(market, symbol).to_dict())


@bp.get("/quotes")
@login_required
def quotes():
    """Batch quotes for the watchlist: ?market=KR&symbols=005930,000660 (max 50)."""
    market = _market()
    symbols = list(dict.fromkeys(s.strip().upper() for s in request.args.get("symbols", "").split(",") if s.strip()))
    if not symbols or len(symbols) > 50 or any(len(s) > 20 for s in symbols):
        raise ApiError(400, "invalid_symbols", "symbols: 1~50 comma-separated codes")
    result = get_provider(market).get_quotes(market, symbols)
    items = []
    for sym in symbols:
        r = result.get(sym)
        if r is None:
            items.append({"symbol": sym, "error": "not_found"})
        elif isinstance(r, ApiError):
            items.append({"symbol": sym, "error": r.code})
        else:
            items.append({"symbol": sym, **r.to_dict()})
    return jsonify(items=items)


@bp.get("/search")
@login_required
def search():
    market = _market()
    q = request.args.get("q", "").strip()
    if not q:
        return jsonify(items=[])
    return jsonify(items=[i.to_dict() for i in get_provider(market).search(market, q)])

