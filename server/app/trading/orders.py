"""Stock orders through the Toss Open API with safety rails.

Rails (all enforced server-side):
- preview first: build_order() validates and returns the exact payload without sending anything
- trading unlock: a fresh OTP unlocks manual trading for TRADING_UNLOCK_MIN minutes
- limits: max value per order, max orders per day (manual + bots)
- pre-checks: tick grid, KRX price limits, buying power / sellable quantity, session status
- idempotency: every order carries a unique clientOrderId (Toss dedupes it for 10 minutes)
- audit: every sent order is stored in the `orders` collection
"""
from __future__ import annotations

import re
import uuid
from datetime import datetime, timedelta, timezone
from typing import Any, Optional

from flask import current_app, g

from ..account.toss_account import HIGH_VALUE_KRW, SYMBOL_RE, _account, normalize
from ..auth import otp
from ..errors import ApiError
from ..market import extras
from ..market.models import Market
from ..storage import get_store
from ..users import repository as users
from ..users.repository import now_iso
from .ticks import fmt_price, round_price

KST = timezone(timedelta(hours=9))
ORDER_ID_RE = re.compile(r"[A-Za-z0-9\-_=+/]{1,128}")


def kst_today() -> str:
    return datetime.now(KST).date().isoformat()


# ------------------------------------------------------------------ unlock / limits
def trading_status(user: dict) -> dict:
    until = user.get("trading_unlocked_until")
    unlocked = bool(until) and datetime.fromisoformat(until) > datetime.now(timezone.utc)
    cfg = current_app.config
    return {
        "unlocked": unlocked,
        "unlocked_until": until if unlocked else None,
        "today_orders": today_order_count(user["id"]),
        "limits": {
            "max_orders_per_day": cfg["TRADING_MAX_ORDERS_PER_DAY"],
            "max_order_krw": cfg["TRADING_MAX_ORDER_KRW"],
            "max_order_usd": cfg["TRADING_MAX_ORDER_USD"],
            "unlock_minutes": cfg["TRADING_UNLOCK_MIN"],
        },
    }


def unlock(user: dict, code: str) -> dict:
    user = otp.verify(user, code)  # single-use code, lockout on repeated failures
    until = datetime.now(timezone.utc) + timedelta(minutes=current_app.config["TRADING_UNLOCK_MIN"])
    user = users.update_user(user["id"], trading_unlocked_until=until.isoformat())
    return trading_status(user)


def lock(user: dict) -> dict:
    return trading_status(users.update_user(user["id"], trading_unlocked_until=None))


def require_unlocked(user: dict) -> None:
    if not trading_status(user)["unlocked"]:
        raise ApiError(403, "trading_locked", "거래가 잠겨 있습니다. OTP로 잠금을 해제하세요")


def today_order_count(uid: str) -> int:
    today = kst_today()
    return len(get_store().find("orders", lambda o: o.get("user_id") == uid and o.get("date") == today))


def check_limits(uid: str, market: Market, value: float) -> None:
    cfg = current_app.config
    if today_order_count(uid) >= cfg["TRADING_MAX_ORDERS_PER_DAY"]:
        raise ApiError(429, "daily_order_limit", f"오늘 주문 한도({cfg['TRADING_MAX_ORDERS_PER_DAY']}건)에 도달했습니다")
    cap = cfg["TRADING_MAX_ORDER_KRW"] if market == Market.KR else cfg["TRADING_MAX_ORDER_USD"]
    if value > cap:
        unit = "원" if market == Market.KR else "달러"
        raise ApiError(400, "order_value_limit", f"주문 1건 한도({cap:,.0f}{unit})를 넘습니다")


# ------------------------------------------------------------------ build / send
def _num(v: Any, name: str) -> float:
    try:
        f = float(v)
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_number", f"{name}: 숫자를 입력하세요")
    if f <= 0:
        raise ApiError(400, "invalid_number", f"{name}: 0보다 커야 합니다")
    return f


def build_order(body: dict) -> dict:
    """Validate an order and build the Toss payload. Read-only calls only; nothing is ordered here."""
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    m = market.value
    symbol = str(body.get("symbol", "")).strip().upper()
    if not SYMBOL_RE[m].fullmatch(symbol):
        raise ApiError(400, "invalid_symbol")
    side = str(body.get("side", "")).upper()
    if side not in ("BUY", "SELL"):
        raise ApiError(400, "invalid_side")
    order_type = str(body.get("order_type", "LIMIT")).upper()
    if order_type not in ("LIMIT", "MARKET"):
        raise ApiError(400, "invalid_order_type")
    tif = str(body.get("time_in_force", "DAY")).upper()
    if tif not in ("DAY", "OPG", "CLS") or (tif == "OPG" and m != "KR") or (tif == "CLS" and (m != "US" or order_type != "LIMIT")):
        raise ApiError(400, "invalid_time_in_force", "OPG(시가 단일가)는 국내, CLS(종가)는 미국 지정가만 가능합니다")
    qty = _num(body.get("quantity"), "수량")
    if qty != int(qty):
        raise ApiError(400, "invalid_quantity", "수량은 정수여야 합니다")
    qty = int(qty)
    price: Optional[float] = None
    if order_type == "LIMIT":
        raw = _num(body.get("price"), "가격")
        price = round_price(m, raw, "down" if side == "BUY" else "up")  # never worse than what the user typed

    client, seq = _account()
    prices = client.request("GET", "/api/v1/prices", params={"symbols": symbol}) or []
    if not prices:
        raise ApiError(404, "stock_not_found", symbol)
    current = float(prices[0]["lastPrice"])
    est_price = price if price is not None else current
    value = qty * est_price
    problems: list = []
    warnings: list = []

    if market == Market.KR and price is not None:
        lim = normalize(client.request("GET", "/api/v1/price-limits", params={"symbol": symbol}) or {})
        up, lo = lim.get("upperLimitPrice"), lim.get("lowerLimitPrice")
        if (up and price > up) or (lo and price < lo):
            problems.append(f"가격이 오늘 상·하한가({lo or 0:,.0f} ~ {up or 0:,.0f}) 밖입니다")

    currency = "KRW" if market == Market.KR else "USD"
    if side == "BUY":
        bp = normalize(client.request("GET", "/api/v1/buying-power", params={"currency": currency}, account_seq=seq) or {})
        cash = bp.get("cashBuyingPower") or 0.0
        if value > cash:
            problems.append(f"매수 가능 금액({cash:,.2f} {currency})이 부족합니다 (필요 약 {value:,.2f})")
        if order_type == "MARKET":
            warnings.append("시장가 매수는 체결 가격이 현재가와 다를 수 있습니다")
    else:
        sellable = float((client.request("GET", "/api/v1/sellable-quantity", params={"symbol": symbol}, account_seq=seq) or {})
                         .get("sellableQuantity") or 0)
        if qty > sellable:
            problems.append(f"매도 가능 수량({sellable:g}주)보다 많습니다")

    if price is not None and abs(price / current - 1) > 0.1:
        warnings.append(f"주문가가 현재가와 {abs(price / current - 1) * 100:.1f}% 차이 납니다")
    try:
        status = extras.market_status(m)
        if not status["open"]:
            warnings.append(f"현재 {status['label']} — 주문이 거부되거나 다음 거래 시간에 처리될 수 있습니다")
    except ApiError:
        status = None
    if problems:
        raise ApiError(400, "order_invalid", " / ".join(problems))

    high_value = market == Market.KR and value >= HIGH_VALUE_KRW
    if high_value and not body.get("confirm_high_value"):
        raise ApiError(400, "confirm_high_value_required", "1억 원 이상 주문입니다. 고액 주문 확인에 체크하세요")

    payload = {
        "clientOrderId": uuid.uuid4().hex,
        "symbol": symbol,
        "side": side,
        "orderType": order_type,
        "timeInForce": tif,
        "quantity": str(qty),
        **({"price": fmt_price(m, price)} if price is not None else {}),
        **({"confirmHighValueOrder": True} if high_value else {}),
    }
    return {
        "market": m,
        "payload": payload,
        "checks": {"current_price": current, "price": price, "estimated_value": value, "currency": currency,
                   "session": status},
        "warnings": warnings,
    }


def submit(built: dict, uid: str, source: str = "manual") -> dict:
    """Send a built order. Caller is responsible for unlock/OTP checks; limits are enforced here."""
    market = Market(built["market"])
    check_limits(uid, market, built["checks"]["estimated_value"])
    client, seq = _account()
    result = normalize(client.request("POST", "/api/v1/orders", json=built["payload"], account_seq=seq) or {})
    p = built["payload"]
    get_store().put("orders", p["clientOrderId"], {
        "user_id": uid, "kind": "order", "source": source, "market": built["market"], "symbol": p["symbol"],
        "side": p["side"], "payload": p, "result": result, "date": kst_today(), "created_at": now_iso(),
    })
    return result


# ------------------------------------------------------------------ queries / changes
def list_orders(status: str, symbol: Optional[str] = None) -> list:
    client, seq = _account()
    params: dict = {"status": status, "limit": 100}
    if symbol:
        params["symbol"] = symbol
    if status == "CLOSED":
        params["from"] = (datetime.now(KST).date() - timedelta(days=30)).isoformat()
    out, cursor = [], None
    for _ in range(5):
        if cursor:
            params["cursor"] = cursor
        r = client.request("GET", "/api/v1/orders", params=params, account_seq=seq) or {}
        out += normalize(r.get("orders", []))
        cursor = r.get("nextCursor")
        if status == "OPEN" or not r.get("hasNext") or not cursor:
            break
    return out


def _check_id(order_id: str) -> None:
    if not ORDER_ID_RE.fullmatch(order_id):
        raise ApiError(400, "invalid_order_id")


def get_order(order_id: str) -> dict:
    _check_id(order_id)
    client, seq = _account()
    return normalize(client.request("GET", f"/api/v1/orders/{order_id}", account_seq=seq) or {})


def cancel(order_id: str) -> dict:
    _check_id(order_id)
    client, seq = _account()
    return normalize(client.request("POST", f"/api/v1/orders/{order_id}/cancel", json={}, account_seq=seq) or {})


def modify(order_id: str, body: dict) -> dict:
    """Change price/quantity of an open order. US orders can't change quantity (Toss rule)."""
    _check_id(order_id)
    current = get_order(order_id)
    m = "KR" if current.get("currency") == "KRW" else "US"
    order_type = str(body.get("order_type", current.get("orderType", "LIMIT"))).upper()
    payload: dict = {"orderType": order_type}
    if order_type == "LIMIT":
        payload["price"] = fmt_price(m, round_price(m, _num(body.get("price"), "가격")))
    if m == "KR":
        q = _num(body.get("quantity", current.get("quantity")), "수량")
        if q != int(q):
            raise ApiError(400, "invalid_quantity", "수량은 정수여야 합니다")
        payload["quantity"] = str(int(q))
    client, seq = _account()
    return normalize(client.request("POST", f"/api/v1/orders/{order_id}/modify", json=payload, account_seq=seq) or {})
