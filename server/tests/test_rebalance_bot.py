"""Tests for multi-asset rebalancing bots (paper, live sell-before-buy, API lifecycle)."""
from datetime import date, datetime, timedelta, timezone

import pytest

from app.account import toss_account
from app.autotrade import bots, executor, rebalance
from app.market.models import Candle
from app.quant.engine import Costs
from app.trading import orders as trading
from fakes import FakeToss, login


def _day_ts(d: date) -> int:
    return int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())


def _series(days, base_px, step=0.0):
    return [Candle(_day_ts(d), base_px + i * step, base_px + i * step + 1, base_px + i * step - 1, base_px + i * step, 1000)
            for i, d in enumerate(days)]


@pytest.fixture
def rebal_env(app, monkeypatch):
    days = [d for d in (date(2026, 9, 1) + timedelta(days=i) for i in range(90))
            if d.weekday() < 5 and d <= date(2026, 11, 20)]
    data = {
        "SPY": _series(days, 100.0, 0.5),   # rising
        "AGG": _series(days, 100.0, 0.0),   # flat
    }

    class MultiProvider:
        def get_history(self, market, symbol, interval, start):
            return data[symbol]

    monkeypatch.setattr(rebalance, "get_provider", lambda m: MultiProvider())
    monkeypatch.setattr(rebalance, "_costs", lambda m: Costs(0, 0, 0))
    monkeypatch.setattr(rebalance, "SELL_SETTLE_INTERVAL", 0)
    with app.test_request_context():
        yield days, data


def test_rebalance_bot_paper_initial_build_and_monthly_rebalance(rebal_env):
    days, _ = rebal_env
    bot = bots.create("u1", {
        "kind": "rebalance",
        "market": "US",
        "model": "classic_60_40",
        "rebalance": "monthly",
        "capital": 10_000,
    })
    assert bot["kind"] == "rebalance" and bot["category"] == "rebalance"
    d1 = next(d.isoformat() for d in days if d.month == 10)  # first Oct trading day
    r1 = executor.run(bot, d1)
    assert r1 == {"ran": d1, "rebalanced": True}
    st = bots.get("u1", bot["id"])["state"]["paper"]
    assert st["positions"]["SPY"]["qty"] > 0 and st["positions"]["AGG"]["qty"] > 0
    assert st["last_rebalance_date"] == d1

    # Same month next day -> no rebalance triggered
    d2 = next(d.isoformat() for d in days if d.month == 10 and d.isoformat() > d1)
    r2 = executor.run(bots.get("u1", bot["id"]), d2)
    assert r2 == {"ran": d2, "rebalanced": False}

    # New month (Nov) -> periodic rebalance triggers again
    d3 = next(d.isoformat() for d in days if d.month == 11)
    r3 = executor.run(bots.get("u1", bot["id"]), d3)
    assert r3 == {"ran": d3, "rebalanced": True}
    assert len(bots.get("u1", bot["id"])["state"]["paper"]["rebalances"]) == 2


def test_rebalance_bot_live_sells_before_buys_and_reconciles(rebal_env, monkeypatch):
    days, _ = rebal_env
    filled = {"done": False}
    fake = FakeToss({
        ("GET", "/api/v1/prices"): [{"symbol": "SPY", "lastPrice": "200"}, {"symbol": "AGG", "lastPrice": "100"}],
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "100"},
        ("GET", "/api/v1/buying-power"): lambda p, j: {"currency": "USD", "cashBuyingPower": "5000" if filled["done"] else "0"},
        ("POST", "/api/v1/orders"): lambda p, j: {"orderId": f"ord-{j['symbol']}-{j['side']}"},
        ("GET", "/api/v1/orders/ord-SPY-SELL"): lambda p, j: (
            filled.__setitem__("done", True) or {"status": "FILLED", "execution": {"filledQuantity": "20", "averageFilledPrice": "200"}}
        ),
    })
    for mod in (rebalance, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))

    bot = bots.create("u1", {
        "kind": "rebalance", "market": "US", "model": "classic_60_40",
        "rebalance": "monthly", "capital": 10_000,
    })
    bot["mode"] = "live"
    # Overweight SPY (50 * 200 = 10,000 = 100% SPY, 0% AGG) -> must SELL SPY first, then BUY AGG
    bot["state"]["live"]["positions"] = {"SPY": {"qty": 50, "entry_price": 100.0}}
    bots.save(bot)

    today = days[-1].isoformat()
    executor.run(bots.get("u1", bot["id"]), today)
    posts = [c[3] for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/orders"]
    assert len(posts) == 2
    assert posts[0]["symbol"] == "SPY" and posts[0]["side"] == "SELL"
    assert posts[1]["symbol"] == "AGG" and posts[1]["side"] == "BUY"


def test_api_rebalance_bot_create_preview_and_delete(client, monkeypatch):
    login(client, monkeypatch)
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setattr("app.autotrade.routes.target_date", lambda m, now: date.today().isoformat())
    r = client.post("/api/autotrade/bots", json={
        "kind": "rebalance", "market": "US", "model": "all_weather", "rebalance": "quarterly", "capital": 50_000,
    })
    assert r.status_code == 200, r.json
    b = r.json["bot"]
    assert b["kind"] == "rebalance" and b["category"] == "rebalance" and len(b["assets"]) == 5
    ev = client.post(f"/api/autotrade/bots/{b['id']}/evaluate")
    assert ev.status_code == 200 and ev.json["due"] is True and len(ev.json["orders"]) == 5
    assert client.delete(f"/api/autotrade/bots/{b['id']}").status_code == 200
