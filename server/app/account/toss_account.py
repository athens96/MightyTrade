"""Account data and conditional (OCO) orders via the Toss Open API, using the user's own key."""
from __future__ import annotations

import re
import uuid
from datetime import date, timedelta
from typing import Any, Optional

from flask import g

from ..errors import ApiError
from ..integrations.toss import get_toss_client
from ..market.models import Market
from ..trading.ticks import fmt_price, round_price
from ..users import credentials

# Toss returns decimals as strings; convert them to numbers except for identifier-like fields.
STRING_KEYS = {
    "symbol", "name", "marketCountry", "currency", "market", "conditionalOrderId", "clientOrderId", "orderId",
    "triggeredOrderId", "status", "type", "orderSide", "orderType", "expireDate", "createdAt", "date",
    "timestamp", "accountNo", "accountType", "nextCursor",
}
RATE_KEYS = {"rate": "ratePct", "rateAfterCost": "rateAfterCostPct"}  # Toss rates are fractions
SYMBOL_RE = {"KR": re.compile(r"[0-9A-Z]{6}"), "US": re.compile(r"[A-Z][A-Z0-9.\-]{0,11}")}
HIGH_VALUE_KRW = 100_000_000


def normalize(v: Any, key: str = "") -> Any:
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in RATE_KEYS and isinstance(x, (str, int, float)):
                out[RATE_KEYS[k]] = float(x) * 100
            else:
                out[k] = normalize(x, k)
        return out
    if isinstance(v, list):
        return [normalize(x, key) for x in v]
    if isinstance(v, str) and key not in STRING_KEYS:
        try:
            return float(v)
        except ValueError:
            return v
    return v


def _account() -> tuple:
    creds = credentials.get_toss(g.user["id"])
    if not creds:
        raise ApiError(409, "toss_credentials_required", "토스증권 API 키를 먼저 등록하세요 (설정)")
    if creds.get("account_seq") is None:
        raise ApiError(409, "toss_account_missing", "토스증권 위탁 계좌를 찾지 못했습니다. 설정에서 키를 다시 등록하세요")
    return get_toss_client(), creds["account_seq"]


def summary() -> dict:
    client, seq = _account()
    holdings = normalize(client.request("GET", "/api/v1/holdings", account_seq=seq) or {})
    bp = {}
    for cur in ("KRW", "USD"):
        r = client.request("GET", "/api/v1/buying-power", params={"currency": cur}, account_seq=seq) or {}
        bp[cur] = normalize(r).get("cashBuyingPower")
    return {"holdings": holdings, "buying_power": bp}


def conditional_orders(status: str) -> list:
    client, seq = _account()
    out, cursor = [], None
    for _ in range(10):  # up to 10 pages
        params = {"status": status, "limit": 100}
        if cursor:
            params["cursor"] = cursor
        r = client.request("GET", "/api/v1/conditional-orders", params=params, account_seq=seq) or {}
        out += normalize(r.get("conditionalOrders", []))
        cursor = r.get("nextCursor")
        if not r.get("hasNext") or not cursor:
            break
    return out


def cancel_conditional(order_id: str) -> None:
    if not re.fullmatch(r"[A-Za-z0-9\-_]{1,64}", order_id):
        raise ApiError(400, "invalid_order_id")
    client, seq = _account()
    client.request("DELETE", f"/api/v1/conditional-orders/{order_id}", account_seq=seq)


def _num(body: dict, path: str) -> float:
    cur: Any = body
    for k in path.split("."):
        cur = cur.get(k) if isinstance(cur, dict) else None
    try:
        v = float(cur)
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_number", f"{path}: 숫자를 입력하세요")
    if v <= 0:
        raise ApiError(400, "invalid_number", f"{path}: 0보다 커야 합니다")
    return v


def build_oco(body: dict) -> dict:
    """Validate an OCO bracket (take-profit above / stop-loss below the current price) and build the Toss payload.

    Read-only checks against Toss: current price and sellable quantity. Nothing is ordered here.
    """
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    m = market.value
    symbol = str(body.get("symbol", "")).strip().upper()
    if not SYMBOL_RE[m].fullmatch(symbol):
        raise ApiError(400, "invalid_symbol")
    qty = _num(body, "quantity")
    if qty != int(qty):
        raise ApiError(400, "invalid_quantity", "수량은 정수여야 합니다")
    qty = int(qty)

    tp_trigger = round_price(m, _num(body, "take_profit.trigger"))
    tp_price = round_price(m, _num(body, "take_profit.price"))
    sl_trigger = round_price(m, _num(body, "stop_loss.trigger"))
    sl_price = round_price(m, _num(body, "stop_loss.price"), "down")

    expire = body.get("expire_date") or (date.today() + timedelta(days=30)).isoformat()
    try:
        exp = date.fromisoformat(expire)
    except ValueError:
        raise ApiError(400, "invalid_expire_date")
    if not date.today() <= exp <= date.today() + timedelta(days=365):
        raise ApiError(400, "invalid_expire_date", "만료일은 오늘부터 1년 이내여야 합니다")

    client, seq = _account()
    prices = client.request("GET", "/api/v1/prices", params={"symbols": symbol}) or []
    if not prices:
        raise ApiError(404, "stock_not_found", symbol)
    current = float(prices[0]["lastPrice"])
    sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": symbol}, account_seq=seq) or {})
                     .get("sellableQuantity") or 0)

    problems = []
    if not tp_trigger > current > sl_trigger:
        problems.append(f"익절 감시가({tp_trigger:g}) > 현재가({current:g}) > 손절 감시가({sl_trigger:g}) 순서여야 합니다")
    if qty > sellable:
        problems.append(f"매도 가능 수량({sellable:g}주)보다 많습니다")
    if sl_price > sl_trigger:
        problems.append("손절 주문가는 손절 감시가 이하여야 체결 가능성이 높습니다")
    if problems:
        raise ApiError(400, "oco_invalid", " / ".join(problems))

    value = qty * current
    high_value = market == Market.KR and value >= HIGH_VALUE_KRW
    if high_value and not body.get("confirm_high_value"):
        raise ApiError(400, "confirm_high_value_required", "1억 원 이상 주문입니다. 고액 주문 확인에 체크하세요")

    payload = {
        "symbol": symbol,
        "type": "OCO",
        "quantity": str(qty),
        "orderType": "LIMIT",  # OCO supports LIMIT only
        "expireDate": exp.isoformat(),
        "clientOrderId": uuid.uuid4().hex,  # idempotency key (10 min) — a retried request can't double-order
        "first": {"orderSide": "SELL", "triggerPrice": fmt_price(m, tp_trigger), "orderPrice": fmt_price(m, tp_price)},
        "second": {"orderSide": "SELL", "triggerPrice": fmt_price(m, sl_trigger), "orderPrice": fmt_price(m, sl_price)},
        **({"confirmHighValueOrder": True} if high_value else {}),
    }
    checks = {
        "current_price": current,
        "sellable_quantity": sellable,
        "order_value": value,
        "take_profit_pct": (tp_trigger / current - 1) * 100,
        "stop_loss_pct": (sl_trigger / current - 1) * 100,
        "rounded": {"take_profit": [tp_trigger, tp_price], "stop_loss": [sl_trigger, sl_price]},
    }
    return {"market": m, "payload": payload, "checks": checks, "account_seq": seq}


def submit_oco(built: dict) -> dict:
    client, seq = _account()
    return normalize(client.request("POST", "/api/v1/conditional-orders", json=built["payload"], account_seq=seq) or {})
