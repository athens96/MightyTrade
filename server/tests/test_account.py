"""Account (holdings, OCO brackets) and price-grid rules, against a fake Toss client."""
import pytest

from app.account import toss_account
from app.errors import ApiError
from app.trading.ticks import fmt_price, round_price
from fakes import FakeToss, login


@pytest.fixture
def session(client, monkeypatch):
    return client, login(client, monkeypatch)


@pytest.fixture
def toss(monkeypatch):
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "182300", "currency": "KRW"}],
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "10"},
        ("POST", "/api/v1/conditional-orders"): {"conditionalOrderId": "co-1", "clientOrderId": "x"},
        ("GET", "/api/v1/holdings"): {"profitLoss": {"rate": "0.05", "amount": {"krw": "1000", "usd": None}},
                                      "items": [{"symbol": "005930", "quantity": "10", "profitLoss": {"rate": "-0.02"}}]},
        ("GET", "/api/v1/buying-power"): lambda p, j: {"currency": p["currency"], "cashBuyingPower": "500"},
    })
    monkeypatch.setattr(toss_account, "_account", lambda: (fake, 7))
    return fake


OCO = {"market": "KR", "symbol": "005930", "quantity": 5,
       "take_profit": {"trigger": 200000, "price": 200000}, "stop_loss": {"trigger": 170000, "price": 168345}}


# ------------------------------------------------------------------ ticks / normalize
def test_tick_rounding():
    assert round_price("KR", 182345) == 182300 and round_price("KR", 182350) == 182400  # tick 100, half-up
    assert round_price("KR", 168345, "down") == 168300
    assert round_price("KR", 201234) == 201000  # >= 200,000: tick 500
    assert round_price("KR", 4998, "up") == 5000
    assert round_price("US", 12.345) == 12.35 and round_price("US", 0.12345, "down") == 0.1234
    assert fmt_price("KR", 182300.0) == "182300" and fmt_price("US", 12.5) == "12.5"


def test_normalize_keeps_ids_and_converts_rates():
    out = toss_account.normalize({"symbol": "005930", "quantity": "10", "profitLoss": {"rate": "0.05"}, "name": "삼성전자"})
    assert out == {"symbol": "005930", "quantity": 10.0, "profitLoss": {"ratePct": 5.0}, "name": "삼성전자"}


# ------------------------------------------------------------------ OCO
def test_build_oco_payload(app, toss):
    with app.test_request_context():
        built = toss_account.build_oco(OCO)
    p = built["payload"]
    assert p["type"] == "OCO" and p["orderType"] == "LIMIT" and p["quantity"] == "5"
    assert p["first"] == {"orderSide": "SELL", "triggerPrice": "200000", "orderPrice": "200000"}
    assert p["second"] == {"orderSide": "SELL", "triggerPrice": "170000", "orderPrice": "168300"}  # snapped down to tick
    assert len(p["clientOrderId"]) == 32 and "confirmHighValueOrder" not in p
    assert built["checks"]["current_price"] == 182300


@pytest.mark.parametrize("change,msg", [
    ({"take_profit": {"trigger": 180000, "price": 180000}}, "순서"),  # TP below current
    ({"quantity": 11}, "매도 가능 수량"),
    ({"stop_loss": {"trigger": 170000, "price": 171000}}, "손절 주문가"),
])
def test_build_oco_rejects_bad_brackets(app, toss, change, msg):
    with app.test_request_context(), pytest.raises(ApiError) as e:
        toss_account.build_oco({**OCO, **change})
    assert e.value.code == "oco_invalid" and msg in e.value.message


def test_high_value_requires_confirmation(app, toss):
    toss.routes[("GET", "/api/v1/sellable-quantity")] = {"sellableQuantity": "1000"}
    big = {**OCO, "quantity": 600}  # 600 x 182,300 > 1억
    with app.test_request_context(), pytest.raises(ApiError) as e:
        toss_account.build_oco(big)
    assert e.value.code == "confirm_high_value_required"
    with app.test_request_context():
        assert toss_account.build_oco({**big, "confirm_high_value": True})["payload"]["confirmHighValueOrder"] is True


def test_api_oco_dry_run_by_default_never_posts(session, toss):
    c, _ = session
    r = c.post("/api/account/conditional-orders/oco", json=OCO)
    assert r.status_code == 200 and r.json["dry_run"] is True
    assert not [x for x in toss.calls if x[0] == "POST"]


def test_api_oco_live_requires_fresh_otp_then_posts(session, toss, app):
    import time
    c, totp = session
    r = c.post("/api/account/conditional-orders/oco", json={**OCO, "dry_run": False, "otp_code": "000000"})
    assert r.status_code == 401 and not [x for x in toss.calls if x[0] == "POST"]
    r = c.post("/api/account/conditional-orders/oco", json={**OCO, "dry_run": False, "otp_code": totp.at(time.time() + 30)})
    assert r.status_code == 200 and r.json["result"]["conditionalOrderId"] == "co-1"
    posts = [x for x in toss.calls if x[0] == "POST"]
    assert len(posts) == 1 and posts[0][4] == 7  # account header
    with app.app_context():
        from app.storage import get_store
        rows = get_store().find("orders", lambda o: o["kind"] == "conditional_oco")
        assert rows and rows[0]["date"]  # audit trail with KST date for daily limit counting
    assert c.get("/api/trading/status").json["today_orders"] == 1


def test_api_oco_enforces_order_value_limit(session, toss, app):
    import time
    c, totp = session
    app.config["TRADING_MAX_ORDER_KRW"] = 500_000  # OCO is 5 * 182,300 = 911,500
    r = c.post("/api/account/conditional-orders/oco", json={**OCO, "dry_run": False, "otp_code": totp.at(time.time() + 30)})
    assert r.status_code == 400 and r.json["error"] == "order_value_limit"
    assert not [x for x in toss.calls if x[0] == "POST"]


def test_api_oco_enforces_daily_limit(session, toss, app):
    import time
    c, totp = session
    app.config["TRADING_MAX_ORDERS_PER_DAY"] = 1
    with app.app_context():
        from app.storage import get_store
        from app.trading.orders import kst_today
        uid = get_store().find("users")[0]["id"]
        get_store().put("orders", "existing", {"user_id": uid, "date": kst_today()})
    r = c.post("/api/account/conditional-orders/oco", json={**OCO, "dry_run": False, "otp_code": totp.at(time.time() + 30)})
    assert r.status_code == 429 and r.json["error"] == "daily_order_limit"
    assert not [x for x in toss.calls if x[0] == "POST"]


def test_api_summary(session, toss):
    c, _ = session
    body = c.get("/api/account/summary").json
    assert body["buying_power"] == {"KRW": 500.0, "USD": 500.0}
    assert body["holdings"]["profitLoss"]["ratePct"] == 5.0
    assert body["holdings"]["items"][0]["symbol"] == "005930"
