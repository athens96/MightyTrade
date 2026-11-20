"""Manual trading: preview by default, OTP unlock, limits, pre-checks, audit — against a fake Toss client."""
import time

import pytest

from app.account import toss_account
from app.errors import ApiError
from app.market import extras
from app.trading import orders
from fakes import FakeToss, login

OPEN = {"open": True, "label": "정규장", "session": "regularMarket"}


@pytest.fixture
def session(client, monkeypatch):
    return client, login(client, monkeypatch)


@pytest.fixture
def toss(monkeypatch):
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "182300", "currency": "KRW"}],
        ("GET", "/api/v1/price-limits"): {"upperLimitPrice": "236900", "lowerLimitPrice": "127700", "currency": "KRW"},
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "5000000"},
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "10"},
        ("POST", "/api/v1/orders"): {"orderId": "o-1", "clientOrderId": "x"},
        ("POST", "/api/v1/orders/o-1/cancel"): {"orderId": "o-2"},
        ("GET", "/api/v1/orders/o-1"): {"orderId": "o-1", "currency": "USD", "orderType": "LIMIT", "quantity": "3"},
        ("POST", "/api/v1/orders/o-1/modify"): {"orderId": "o-3"},
    })
    monkeypatch.setattr(toss_account, "_account", lambda: (fake, 7))
    monkeypatch.setattr(orders, "_account", lambda: (fake, 7))
    monkeypatch.setattr(extras, "market_status", lambda m, now=None: OPEN)
    return fake


BUY = {"market": "KR", "symbol": "005930", "side": "BUY", "order_type": "LIMIT", "quantity": 10, "price": 182345}
posts = lambda fake: [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]  # noqa: E731


def test_build_limit_buy_rounds_down_to_tick(app, toss):
    with app.test_request_context():
        b = orders.build_order(BUY)
    p = b["payload"]
    assert p["price"] == "182300" and p["side"] == "BUY" and p["orderType"] == "LIMIT" and p["timeInForce"] == "DAY"
    assert p["quantity"] == "10" and len(p["clientOrderId"]) == 32
    assert b["checks"]["estimated_value"] == 1_823_000


@pytest.mark.parametrize("change,needle", [
    ({"price": 300000}, "상·하한가"),
    ({"quantity": 100}, "매수 가능 금액"),
    ({"side": "SELL", "quantity": 11}, "매도 가능 수량"),
])
def test_build_rejects(app, toss, change, needle):
    with app.test_request_context(), pytest.raises(ApiError) as e:
        orders.build_order({**BUY, **change})
    assert e.value.code == "order_invalid" and needle in e.value.message


def test_time_in_force_rules(app, toss):
    with app.test_request_context():
        assert orders.build_order({**BUY, "time_in_force": "OPG"})["payload"]["timeInForce"] == "OPG"
        with pytest.raises(ApiError):
            orders.build_order({**BUY, "time_in_force": "CLS"})  # CLS is US limit only


def test_preview_is_default_and_sends_nothing(session, toss):
    c, _ = session
    r = c.post("/api/trading/orders", json=BUY)
    assert r.status_code == 200 and r.json["dry_run"] is True and not posts(toss)


def test_live_order_requires_unlock_then_sends_once(session, toss, app):
    c, totp = session
    r = c.post("/api/trading/orders", json={**BUY, "dry_run": False})
    assert r.status_code == 403 and r.json["error"] == "trading_locked" and not posts(toss)
    assert c.post("/api/trading/unlock", json={"otp_code": "000000"}).status_code == 401
    s = c.post("/api/trading/unlock", json={"otp_code": totp.at(time.time() + 30)}).json
    assert s["unlocked"] is True and s["unlocked_until"]
    r = c.post("/api/trading/orders", json={**BUY, "dry_run": False})
    assert r.status_code == 200 and r.json["result"]["orderId"] == "o-1"
    assert len(posts(toss)) == 1 and posts(toss)[0][4] == 7
    assert c.get("/api/trading/status").json["today_orders"] == 1
    c.post("/api/trading/lock")
    assert c.post("/api/trading/orders", json={**BUY, "dry_run": False}).status_code == 403


def test_limits(session, toss, app):
    c, totp = session
    c.post("/api/trading/unlock", json={"otp_code": totp.at(time.time() + 30)})
    app.config["TRADING_MAX_ORDER_KRW"] = 1_000_000
    r = c.post("/api/trading/orders", json={**BUY, "dry_run": False})
    assert r.status_code == 400 and r.json["error"] == "order_value_limit" and not posts(toss)
    app.config["TRADING_MAX_ORDER_KRW"] = 50_000_000
    app.config["TRADING_MAX_ORDERS_PER_DAY"] = 1
    assert c.post("/api/trading/orders", json={**BUY, "dry_run": False}).status_code == 200
    r = c.post("/api/trading/orders", json={**BUY, "dry_run": False})
    assert r.status_code == 429 and r.json["error"] == "daily_order_limit" and len(posts(toss)) == 1


def test_cancel_without_unlock_modify_needs_unlock(session, toss):
    c, totp = session
    assert c.post("/api/trading/orders/o-1/cancel").json["result"]["orderId"] == "o-2"
    assert c.post("/api/trading/orders/o-1/modify", json={"price": 10}).status_code == 403
    c.post("/api/trading/unlock", json={"otp_code": totp.at(time.time() + 30)})
    assert c.post("/api/trading/orders/o-1/modify", json={"price": 12.345}).status_code == 200
    payload = [x for x in toss.calls if x[1] == "/api/v1/orders/o-1/modify"][0][3]
    assert payload == {"orderType": "LIMIT", "price": "12.35"}  # US: no quantity on modify
