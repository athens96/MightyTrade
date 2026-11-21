"""Tests for MT-3: Strategy-agnostic Stop-Loss / Take-Profit / Trailing-Stop & Live Toss OCO sync."""
from datetime import date, datetime, timedelta, timezone

import pytest

from app.account import toss_account
from app.autotrade import bots, executor
from app.market.models import Candle
from app.quant.engine import Backtester, Bars, Costs, Order, RiskRules
from app.quant.strategies import STRATEGIES, Strategy
from app.trading import orders as trading
from fakes import FakeToss


class HoldForever(Strategy):
    id, name, summary, description, params = "hold_forever", "장기보유", "s", "s", []

    def warmup(self):
        return 1

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        if not ctx.position.is_open and i == 1:
            return [Order("buy", reason="초기진입")]
        return []


def _mk_bars(prices: list[tuple[float, float, float, float]]) -> list[Candle]:
    d0 = date(2026, 11, 2)
    out = []
    idx = 0
    cur = d0
    while idx < len(prices):
        if cur.weekday() < 5:
            o, h, l, c = prices[idx]
            ts = int(datetime(cur.year, cur.month, cur.day, tzinfo=timezone.utc).timestamp())
            out.append(Candle(ts, o, h, l, c, 1000))
            idx += 1
        cur += timedelta(days=1)
    return out


def test_backtester_common_stop_take_profit_and_trailing_stop(monkeypatch):
    monkeypatch.setitem(STRATEGIES, "hold_forever", HoldForever)
    # Bar 0..1 warmup, Bar 2 buy @ 100, Bar 3 rallies to 112, Bar 4 drops to 105 (trailing -5% from 112 = 106.4 -> triggers!)
    cs = _mk_bars([
        (100, 101, 99, 100),
        (100, 101, 99, 100),
        (100, 102, 99, 101),   # entry @ 100
        (102, 112, 101, 110),  # peak becomes 112
        (109, 109, 105, 106),  # low 105 <= 112 * 0.95 (106.4) -> trailing stop exit @ 106.4!
        (106, 107, 104, 105),
    ])
    bars = Bars.from_candles(cs)
    res = Backtester(
        bars,
        HoldForever({}),
        capital=10_000,
        costs=Costs(0, 0, 0),
        start_index=1,
        risk=RiskRules(stop_loss_pct=8.0, take_profit_pct=20.0, trailing_stop_pct=5.0),
    ).run()
    assert len(res["trades"]) == 1
    t = res["trades"][0]
    assert t["open"] is False
    assert t["exit_price"] == pytest.approx(106.4)
    assert "트레일링 스톱" in t["exit_reason"]


def test_live_bot_attaches_and_ratchets_toss_oco_protection(app, monkeypatch):
    monkeypatch.setitem(STRATEGIES, "hold_forever", HoldForever)
    cs = _mk_bars([(10000, 10100, 9900, 10000)] * 8)

    class Prov:
        def get_history(self, m, s, iv, st):
            return cs

    cur_price = {"v": "10000"}
    cond_seq = {"n": 0}

    def create_cond(p, j):
        cond_seq["n"] += 1
        return {"conditionalOrderId": f"oco-{cond_seq['n']}"}

    fake = FakeToss({
        ("GET", "/api/v1/prices"): lambda p, j: [{"symbol": "005930", "lastPrice": cur_price["v"]}],
        ("GET", "/api/v1/buying-power"): {"currency": "KRW", "cashBuyingPower": "1000000"},
        ("GET", "/api/v1/sellable-quantity"): {"sellableQuantity": "10"},
        ("GET", "/api/v1/conditional-orders/oco-1"): {"status": "WATCHING", "first": {"status": "WATCHING"}, "second": {"status": "WATCHING"}},
        ("DELETE", "/api/v1/conditional-orders/oco-1"): {},
        ("POST", "/api/v1/conditional-orders"): create_cond,
    })
    monkeypatch.setattr(executor, "get_provider", lambda m: Prov())
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))
    for mod in (executor, trading, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))

    with app.test_request_context():
        bot = bots.create("u1", {
            "market": "KR",
            "symbol": "005930",
            "strategy": "hold_forever",
            "risk": {"stop_loss_pct": 5.0, "take_profit_pct": 12.0, "trailing_stop_pct": 4.0},
            "capital": 200_000,
        })
        bot["mode"] = "live"
        bot["state"]["live"]["position"] = {"qty": 10, "entry_price": 10000.0, "entry_date": "2026-11-05"}
        bots.save(bot)

        today = executor.day_of(cs[-1].time)
        executor.sync_live_protection(bot, today, current=10000.0)
        bots.save(bot)

        cond_posts = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/conditional-orders"]
        assert len(cond_posts) == 1
        oco = cond_posts[0][3]
        assert oco["type"] == "OCO"
        assert oco["first"]["triggerPrice"] == "11200"   # +12% TP
        assert oco["second"]["triggerPrice"] == "9600"   # max(-5%=9500, -4% trail=9600) -> 9600!

        # Price rallies to 10800 -> trailing stop (-4%) moves up to 10368 (KRX tick -> 10360), ratcheting OCO!
        cur_price["v"] = "10800"
        executor.reconcile_live(bot, today)
        cond_posts2 = [c for c in fake.calls if c[0] == "POST" and c[1] == "/api/v1/conditional-orders"]
        assert len(cond_posts2) == 2
        assert any(c[0] == "DELETE" and c[1] == "/api/v1/conditional-orders/oco-1" for c in fake.calls)
        assert int(cond_posts2[1][3]["second"]["triggerPrice"]) > 9600
