"""Auto-trading: paper fills (backtest semantics), live order safety, scheduler — no network."""
import time
from datetime import date, datetime, timedelta, timezone

import pytest

from app.account import toss_account
from app.autotrade import bots, executor, scheduler
from app.market.models import Candle
from app.quant.engine import Costs, Order
from app.quant.strategies import STRATEGIES, Strategy
from app.trading import orders as trading
from fakes import FakeToss, login

DAYS = [d for d in (date(2026, 11, 9) + timedelta(days=i) for i in range(12)) if d.weekday() < 5]  # 11-09 .. 11-20


def day_ts(d: date) -> int:
    return int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())


def candles(overrides=None):
    """Flat 100 bars; overrides: {"2026-11-18": (open, high, low, close)}."""
    out = []
    for d in DAYS:
        o, h, l, c = (overrides or {}).get(d.isoformat(), (100, 101, 99, 100))
        out.append(Candle(day_ts(d), o, h, l, c, 1000))
    return out


class Scripted(Strategy):
    id, name, summary, description, params = "scripted", "테스트", "s", "s", []
    script: dict = {}

    def warmup(self):
        return 1

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        return list(Scripted.script.get(executor.day_of(self.bars.time[i]), []))


class FakeProvider:
    def __init__(self, data):
        self.data = data

    def get_history(self, market, symbol, interval, start):
        return self.data


@pytest.fixture
def env(app, monkeypatch):
    """App context + scripted strategy + fake price history + zero costs."""
    monkeypatch.setitem(STRATEGIES, "scripted", Scripted)
    Scripted.script = {}
    holder = {"provider": FakeProvider(candles())}
    monkeypatch.setattr(executor, "get_provider", lambda m: holder["provider"])
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))
    with app.test_request_context():
        yield holder


def make_bot(mode="paper", capital=10_000):
    bot = bots.create("u1", {"market": "KR", "symbol": "005930", "strategy": "scripted", "capital": capital})
    bot["mode"] = mode
    return bots.save(bot)


def reload(bot):
    return bots.get("u1", bot["id"])


# ------------------------------------------------------------------ paper
def test_paper_market_buy_fills_at_todays_open(env):
    env["provider"] = FakeProvider(candles({"2026-11-18": (110, 112, 108, 111)}))
    Scripted.script = {"2026-11-17": [Order("buy", reason="test")]}  # decided on 11-17 close
    bot = make_bot()
    executor.run(bot, "2026-11-18")
    st = reload(bot)["state"]["paper"]
    assert st["position"] == {"qty": 90, "entry_price": 110, "entry_date": "2026-11-18"}  # floor(10000/110)
    assert st["cash"] == 10_000 - 90 * 110 and st["last_run_date"] == "2026-11-18"
    assert executor.run(reload(bot), "2026-11-18") == {"skipped": "already_ran"}


def test_paper_stop_is_settled_next_run_against_that_days_range(env):
    env["provider"] = FakeProvider(candles({"2026-11-18": (100, 106, 99, 104)}))
    Scripted.script = {"2026-11-17": [Order("buy", kind="stop", stop=105)]}
    bot = make_bot()
    executor.run(bot, "2026-11-18")
    st = reload(bot)["state"]["paper"]
    assert st["position"]["qty"] == 0 and st["pending"][0]["level"] == 105
    executor.run(reload(bot), "2026-11-19")
    st = reload(bot)["state"]["paper"]
    assert st["position"]["entry_price"] == 105 and st["position"]["entry_date"] == "2026-11-18" and not st["pending"]


def test_paper_gap_over_stop_fills_at_open(env):
    env["provider"] = FakeProvider(candles({"2026-11-18": (108, 110, 107, 109)}))
    Scripted.script = {"2026-11-17": [Order("buy", kind="stop", stop=105)]}
    bot = make_bot()
    executor.run(bot, "2026-11-18")
    assert reload(bot)["state"]["paper"]["position"]["entry_price"] == 108


def test_paper_sell_then_rebuy_same_day(env):
    """Volatility-breakout shape: exit at the open, then a breakout entry later the same day."""
    env["provider"] = FakeProvider(candles({"2026-11-18": (100, 106, 99, 104), "2026-11-19": (104, 104, 103, 104)}))
    bot = make_bot()
    bot["state"]["paper"].update(position={"qty": 50, "entry_price": 90, "entry_date": "2026-11-16"}, cash=5_500)
    bot["state"]["paper"]["trades"].append({"entry_date": "2026-11-16", "entry_price": 90, "qty": 50, "entry_reason": "",
                                            "cost_basis": 4500, "exit_date": None, "exit_price": None, "exit_reason": "",
                                            "pnl": None, "return_pct": None})
    bots.save(bot)
    Scripted.script = {"2026-11-17": [Order("sell"), Order("buy", kind="stop", stop_from_open=3)]}
    executor.run(reload(bot), "2026-11-18")
    st = reload(bot)["state"]["paper"]
    assert st["position"]["qty"] == 0 and st["cash"] == 10_500 and st["pending"][0]["level"] == 103
    assert st["trades"][0]["pnl"] == 500  # (100 - 90) * 50
    executor.run(reload(bot), "2026-11-19")
    assert reload(bot)["state"]["paper"]["position"]["entry_price"] == 103


def test_paper_needs_todays_open(env):
    from app.errors import ApiError
    with pytest.raises(ApiError) as e:
        executor.run(make_bot(), "2026-11-23")  # no bar for that day yet
    assert e.value.code == "market_not_open"


def test_turtle_long_held_position_loads_from_entry_date_and_stores_entry_atr(env):
    """Holding past the default warmup window fetches back to entry_date and avoids negative-index ATR wrap."""
    seen_starts = []
    # 90 business days up to 2026-11-20
    all_days = [d for d in (date(2026, 7, 1) + timedelta(days=i) for i in range(150))
                if d.weekday() < 5 and d <= date(2026, 11, 20)]
    long_candles = [Candle(day_ts(d), 100 + i, 102 + i, 98 + i, 101 + i, 1000) for i, d in enumerate(all_days)]

    class RecordingProvider:
        def get_history(self, market, symbol, interval, start):
            seen_starts.append(start)
            return [c for c in long_candles if c.time >= start]

    env["provider"] = RecordingProvider()
    bot = bots.create("u1", {"market": "KR", "symbol": "005930", "strategy": "turtle",
                             "params": {"entry": 10, "exit": 5, "atr_n": 5, "stop_atr": 2, "risk_pct": 1}, "capital": 100_000})
    # Pretend the bot entered 3+ months ago without entry_atr stored (legacy position)
    entry_d = all_days[20].isoformat()
    bot["state"]["paper"]["position"] = {"qty": 10, "entry_price": 120.0, "entry_date": entry_d}
    bots.save(bot)
    d = executor.decide(reload(bot), all_days[-1].isoformat())
    # History start was pulled back before entry_date + warmup
    assert seen_starts[-1] < day_ts(all_days[20]) - 20 * 86400
    assert len(d["orders"]) == 1 and d["orders"][0].side == "sell"
    # Even if provider truncates history before entry_date, entry_atr is respected without wrapping to atr[-1]
    env["provider"] = FakeProvider(long_candles[-25:])
    bot = reload(bot)
    bot["state"]["paper"]["entry_atr"] = 1.5
    bot["state"]["paper"]["position"]["entry_price"] = 250.0
    bots.save(bot)
    d2 = executor.decide(reload(bot), all_days[-1].isoformat())
    assert d2["orders"][0].stop == pytest.approx(247.0)  # 250 - 2 * 1.5


# ------------------------------------------------------------------ live
@pytest.fixture
def toss(env, monkeypatch):
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "005930", "lastPrice": "100"}],
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "1000000"},
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "50"},
        ("POST", "/api/v1/orders"): lambda p, j: {"orderId": f"o-{j['clientOrderId'][:6]}"},
        ("POST", "/api/v1/conditional-orders"): {"conditionalOrderId": "c-1"},
    })
    monkeypatch.setattr(executor, "SELL_SETTLE_INTERVAL", 0)
    for mod in (executor, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))
    return fake


order_posts = lambda f: [c for c in f.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]  # noqa: E731


def test_live_market_order_is_sent_once_per_day(toss):
    Scripted.script = {"2026-11-17": [Order("buy")]}
    bot = make_bot("live")
    executor.run(bot, "2026-11-18")
    executor.run(reload(bot), "2026-11-18")
    posts = order_posts(toss)
    assert len(posts) == 1
    p = posts[0][3]
    assert p["side"] == "BUY" and p["orderType"] == "MARKET" and p["quantity"] == "99"  # floor(10000 / (100 * 1.005)): 0.5% headroom for market fills
    assert p["clientOrderId"] == executor._client_order_id(bot, "2026-11-18", 0, "mkt")  # deterministic -> Toss dedupes retries
    assert reload(bot)["state"]["live"]["pending"][0]["kind"] == "order"


def test_crash_while_sending_never_double_orders(toss):
    """The day is marked done before any order is sent: a retry after a crash skips instead of re-ordering."""
    def boom(p, j):
        raise RuntimeError("network died mid-request")
    toss.routes[("POST", "/api/v1/orders")] = boom
    Scripted.script = {"2026-11-17": [Order("buy")]}
    bot = make_bot("live")
    with pytest.raises(RuntimeError):
        executor.run(bot, "2026-11-18")
    assert reload(bot)["state"]["live"]["last_run_date"] == "2026-11-18"
    assert executor.run(reload(bot), "2026-11-18") == {"skipped": "already_ran"}
    assert len(order_posts(toss)) == 1


def test_live_stop_becomes_conditional_or_market_if_crossed(toss):
    Scripted.script = {"2026-11-17": [Order("buy", kind="stop", stop=104.3)]}
    bot = make_bot("live")
    executor.run(bot, "2026-11-18")
    cond = [c for c in toss.calls if c[1] == "/api/v1/conditional-orders"][0][3]
    assert cond["type"] == "SINGLE" and cond["orderType"] == "MARKET" and cond["expireDate"] == "2026-11-18"
    assert cond["first"] == {"orderSide": "BUY", "triggerPrice": "105"}  # KRX tick 1 below 2,000 -> rounded up
    # price already above the level -> buy now at market instead of waiting for a trigger
    toss.routes[("GET", "/api/v1/prices")] = [{"symbol": "005930", "lastPrice": "106"}]
    bot2 = make_bot("live")
    executor.run(bot2, "2026-11-18")
    assert len(order_posts(toss)) == 1


def test_live_respects_order_limits(toss, app):
    app.config["TRADING_MAX_ORDER_KRW"] = 1_000
    Scripted.script = {"2026-11-17": [Order("buy")]}
    bot = make_bot("live")
    executor.run(bot, "2026-11-18")
    assert not order_posts(toss)
    assert any(e["level"] == "error" and "한도" in e["message"] for e in bots.events(reload(bot)))


def test_live_same_day_sell_and_rebuy_are_both_sent(toss):
    bot = make_bot("live")
    bot["state"]["live"]["position"] = {"qty": 50, "entry_price": 90, "entry_date": "2026-11-16"}
    bots.save(bot)
    Scripted.script = {"2026-11-17": [Order("sell"), Order("buy", kind="stop", stop_from_open=3)]}
    executor.run(reload(bot), "2026-11-18")
    assert order_posts(toss)[0][3]["side"] == "SELL" and order_posts(toss)[0][3]["quantity"] == "50"
    conds = [c for c in toss.calls if c[1] == "/api/v1/conditional-orders"]
    assert conds and conds[0][3]["quantity"] == "96"  # full 10,000 budget (floor(10000 / (103 * 1.005)))


def test_live_same_day_sell_rebuy_full_capital_when_filled_or_pending(toss):
    """Fully invested bot (100% of capital) must still size its same-day conditional re-entry properly."""
    # Case 1: sell fills during settle poll -> cashBuyingPower updates -> full rebuy
    settled = {"done": False}
    toss.routes[("GET", "/api/v1/sellable-quantity")] = {"sellableQuantity": "100"}
    toss.routes[("GET", "/api/v1/buying-power")] = lambda p, j: {"currency": "KRW", "cashBuyingPower": "10000" if settled["done"] else "0"}

    bot = make_bot("live", capital=10_000)
    bot["state"]["live"]["position"] = {"qty": 100, "entry_price": 100, "entry_date": "2026-11-16"}
    bots.save(bot)
    sell_id = f"o-{executor._client_order_id(bot, '2026-11-18', 0, 'mkt')[:6]}"

    def get_order(p, j):
        settled["done"] = True
        return {"status": "FILLED", "execution": {"filledQuantity": "100", "averageFilledPrice": "100"}}

    toss.routes[("GET", f"/api/v1/orders/{sell_id}")] = get_order
    Scripted.script = {"2026-11-17": [Order("sell"), Order("buy", kind="stop", stop_from_open=3)]}
    executor.run(reload(bot), "2026-11-18")
    conds = [c for c in toss.calls if c[1] == "/api/v1/conditional-orders"]
    assert len(conds) == 1 and conds[0][3]["quantity"] == "96"
    assert reload(bot)["state"]["live"]["position"]["qty"] == 0

    # Case 2: sell still PENDING (not yet reflected in buying power) -> conditional rebuy uses estimated proceeds
    bot2 = make_bot("live", capital=10_000)
    bot2["state"]["live"]["position"] = {"qty": 100, "entry_price": 100, "entry_date": "2026-11-16"}
    bots.save(bot2)
    sell_id2 = f"o-{executor._client_order_id(bot2, '2026-11-18', 0, 'mkt')[:6]}"
    toss.routes[("GET", "/api/v1/buying-power")] = {"currency": "KRW", "cashBuyingPower": "0"}
    toss.routes[("GET", f"/api/v1/orders/{sell_id2}")] = {"status": "PENDING", "execution": {"filledQuantity": "0"}}
    executor.run(reload(bot2), "2026-11-18")
    conds = [c for c in toss.calls if c[1] == "/api/v1/conditional-orders"]
    assert len(conds) == 2 and conds[1][3]["quantity"] == "96"


def test_live_rejected_sell_blocks_followup_rebuy(toss):
    """If the open sell is rejected during settle polling, do not place a follow-up buy."""
    toss.routes[("GET", "/api/v1/sellable-quantity")] = {"sellableQuantity": "100"}
    bot = make_bot("live", capital=10_000)
    bot["state"]["live"]["position"] = {"qty": 100, "entry_price": 100, "entry_date": "2026-11-16"}
    bots.save(bot)
    sell_id = f"o-{executor._client_order_id(bot, '2026-11-18', 0, 'mkt')[:6]}"
    toss.routes[("GET", f"/api/v1/orders/{sell_id}")] = {"status": "REJECTED", "execution": {"filledQuantity": "0"}}
    Scripted.script = {"2026-11-17": [Order("sell"), Order("buy", kind="stop", stop_from_open=3)]}
    executor.run(reload(bot), "2026-11-18")
    assert not [c for c in toss.calls if c[1] == "/api/v1/conditional-orders"]
    assert reload(bot)["state"]["live"]["position"]["qty"] == 100


def test_reconcile_applies_fills_and_follows_triggered_conditionals(toss):
    bot = make_bot("live")
    bot["state"]["live"]["pending"] = [
        {"kind": "order", "id": "o-1", "side": "BUY", "date": "2026-11-18", "synced": 0.0},
        {"kind": "conditional", "id": "c-1", "side": "BUY", "date": "2026-11-18", "synced": 0.0},
    ]
    bots.save(bot)
    toss.routes[("GET", "/api/v1/orders/o-1")] = {"status": "FILLED", "execution": {"filledQuantity": "10", "averageFilledPrice": "100"}}
    toss.routes[("GET", "/api/v1/conditional-orders/c-1")] = {"status": "ORDERED", "first": {"status": "ORDERED", "triggeredOrderId": "o-9"}}
    toss.routes[("GET", "/api/v1/orders/o-9")] = {"status": "PARTIAL_FILLED", "execution": {"filledQuantity": "5", "averageFilledPrice": "106"}}
    b = reload(bot)
    executor.reconcile_live(b, "2026-11-18")
    st = b["state"]["live"]
    assert st["position"]["qty"] == 15 and st["position"]["entry_price"] == pytest.approx((10 * 100 + 5 * 106) / 15)
    assert [p["id"] for p in st["pending"]] == ["o-9"]  # filled order done; partially filled one still tracked


# ------------------------------------------------------------------ scheduler
@pytest.fixture
def sched_env(app, monkeypatch):
    start = datetime(2026, 11, 20, 0, 0, tzinfo=timezone.utc)  # 09:00 KST
    monkeypatch.setattr(scheduler, "sessions", lambda m: [("2026-11-20", start, start + timedelta(hours=6, minutes=30))])
    monkeypatch.setattr(scheduler.credentials, "get_toss", lambda uid: {"account_seq": 7})
    ran = []

    def fake_run(bot, d):
        ran.append((bot["id"], d))
        bots.state(bot)["last_run_date"] = d
        bots.save(bot)
    monkeypatch.setattr(executor, "run", fake_run)
    with app.app_context():
        from app.storage import get_store
        get_store().put("users", "u1", {"email": "u@example.com", "otp_enabled": True})
        bot = bots.create("u1", {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6})
        bot["enabled"] = True
        bots.save(bot)
    return start, ran


def test_scheduler_runs_once_after_open(app, sched_env):
    start, ran = sched_env
    scheduler.tick(app, start + timedelta(seconds=30))
    assert ran == []  # waits one minute after the open
    scheduler.tick(app, start + timedelta(minutes=2))
    scheduler.tick(app, start + timedelta(minutes=3))
    assert len(ran) == 1 and ran[0][1] == "2026-11-20"


def test_scheduler_respects_kill_switch_and_late_start(app, sched_env):
    start, ran = sched_env
    with app.app_context():
        from app.users import repository as users
        users.update_user("u1", autotrade_halted=True)
    scheduler.tick(app, start + timedelta(minutes=2))
    assert ran == []
    with app.app_context():
        users.update_user("u1", autotrade_halted=False)
    scheduler.tick(app, start + timedelta(hours=6))  # 15:00 KST, inside the last hour before the close
    assert ran == []
    with app.app_context():
        assert bots.list_for("u1")[0]["state"]["paper"]["last_run_date"] == "2026-11-20"  # day skipped, not retried


def test_scheduler_skips_unapproved_users(app, sched_env):
    start, ran = sched_env
    with app.app_context():
        from app.users import repository as users
        users.update_user("u1", status="rejected")
    scheduler.tick(app, start + timedelta(minutes=2))
    assert ran == []


def test_only_one_scheduler_per_server(app):
    a, b = scheduler.Scheduler(app), scheduler.Scheduler(app)
    try:
        assert a.start() is True
        assert b.start() is False  # file lock held by the first one
    finally:
        a.stop()


# ------------------------------------------------------------------ API
@pytest.fixture
def session(client, monkeypatch):
    return client, login(client, monkeypatch)


def test_api_bot_lifecycle_and_live_needs_otp(session, monkeypatch):
    c, totp = session
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    r = c.post("/api/autotrade/bots", json={"market": "KR", "symbol": "005930", "strategy": "ma_cross",
                                            "params": {"fast": 5, "slow": 20}, "capital": 1_000_000})
    bot = r.json["bot"]
    assert bot["mode"] == "paper" and bot["enabled"] is False and bot["state"]["cash"] == 1_000_000
    assert c.post("/api/autotrade/bots", json={"market": "KR", "symbol": "AAPL", "strategy": "ma_cross", "capital": 1}).status_code == 400
    bid = bot["id"]
    assert c.post(f"/api/autotrade/bots/{bid}/enable").json["bot"]["enabled"] is True
    assert c.post(f"/api/autotrade/bots/{bid}/mode", json={"mode": "live", "otp_code": "000000"}).status_code == 401
    r = c.post(f"/api/autotrade/bots/{bid}/mode", json={"mode": "live", "otp_code": totp.at(time.time() + 30)})
    assert r.json["bot"]["mode"] == "live" and r.json["bot"]["enabled"] is False  # switching mode always turns it off
    assert c.post(f"/api/autotrade/bots/{bid}/run").json["error"] == "paper_only"


def test_api_kill_switch(session, monkeypatch):
    c, totp = session
    cancelled = []
    monkeypatch.setattr(executor, "cancel_live_pending", lambda b: cancelled.append(b["id"]) or 1)
    r = c.post("/api/autotrade/bots", json={"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6})
    bid = r.json["bot"]["id"]
    with c.application.app_context():
        b = bots.get(bots.list_for(r.json["bot"]["user_id"])[0]["user_id"], bid)
        b["state"]["live"]["pending"] = [{"kind": "order", "id": "o-1", "side": "BUY", "date": "x", "synced": 0}]
        bots.save(b)
    assert c.post("/api/autotrade/halt").json == {"halted": True, "cancelled": 1} and cancelled == [bid]
    assert c.get("/api/autotrade/status").json["halted"] is True
    assert c.post("/api/autotrade/resume", json={"otp_code": "000000"}).status_code == 401
    assert c.post("/api/autotrade/resume", json={"otp_code": totp.at(time.time() + 30)}).json == {"halted": False}


def test_api_delete_bot_also_removes_its_events(session, monkeypatch):
    c, _ = session
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    b1 = c.post("/api/autotrade/bots", json={"market": "KR", "symbol": "005930", "strategy": "ma_cross", "capital": 1e6}).json["bot"]["id"]
    b2 = c.post("/api/autotrade/bots", json={"market": "KR", "symbol": "000660", "strategy": "ma_cross", "capital": 1e6}).json["bot"]["id"]
    c.post(f"/api/autotrade/bots/{b1}/enable")
    c.post(f"/api/autotrade/bots/{b2}/enable")
    assert c.delete(f"/api/autotrade/bots/{b1}").status_code == 200
    with c.application.app_context():
        from app.storage import get_store
        assert not get_store().find(bots.EVENTS, lambda e: e.get("bot_id") == b1)
        assert get_store().find(bots.EVENTS, lambda e: e.get("bot_id") == b2)
