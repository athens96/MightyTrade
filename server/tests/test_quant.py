import json
import math

import pyotp
import pytest

from fakes import approve_all
from app.auth import routes as auth_routes
from app.quant import fixtures
from app.quant import indicators as ind
from app.quant.engine import Backtester, Bars, Costs, Order
from app.quant.metrics import performance
from app.quant.strategies import STRATEGIES, Strategy
from app.quant.strategies.turtle import Turtle

DAY = 86400
NO_COST = Costs(commission_pct=0, tax_pct=0, slippage_pct=0)


def make_bars(opens, highs=None, lows=None, closes=None):
    n = len(opens)
    return Bars(
        time=[1_700_000_000 + i * DAY for i in range(n)],
        open=list(opens),
        high=list(highs or [o + 1 for o in opens]),
        low=list(lows or [o - 1 for o in opens]),
        close=list(closes or opens),
        volume=[1000] * n,
    )


class Scripted(Strategy):
    """Emits fixed orders at the close of given bars."""
    id, name, summary, description, params = "scripted", "s", "s", "s", []

    def __init__(self, script):
        self.script = script
        super().__init__({})

    def warmup(self):
        return 0

    def prepare(self, bars):
        pass

    def on_bar(self, i, ctx):
        return self.script.get(i, [])


# ------------------------------------------------------------------ engine
def test_market_order_fills_next_open_not_same_close():
    b = make_bars([100, 110, 120, 130], closes=[105, 115, 125, 135])
    r = Backtester(b, Scripted({0: [Order("buy")], 2: [Order("sell")]}), 1000, NO_COST, 0).run()
    t = r["trades"][0]
    assert t["entry_price"] == 110 and t["entry_time"] == b.time[1]  # decided at bar 0 close -> bar 1 open
    assert t["exit_price"] == 130 and t["bars"] == 2
    assert t["qty"] == 9  # floor(1000 / 110) whole shares
    assert r["equity"][-1]["equity"] == pytest.approx(1000 - 9 * 110 + 9 * 130)


def test_stop_order_trigger_and_gap():
    # bar1: high 104 < level 105 -> no fill; bar2 opens at 108 above level -> fill at open
    b = make_bars([100, 100, 108, 110], highs=[101, 104, 112, 111])
    s = Scripted({0: [Order("buy", kind="stop", stop=105)], 1: [Order("buy", kind="stop", stop=105)]})
    r = Backtester(b, s, 1000, NO_COST, 0).run()
    assert r["trades"][0]["entry_time"] == b.time[2] and r["trades"][0]["entry_price"] == 108


def test_stop_from_open_breakout_level():
    b = make_bars([100, 100, 100], highs=[110, 110, 110], lows=[90, 99, 99])
    r = Backtester(b, Scripted({0: [Order("buy", kind="stop", stop_from_open=5)]}), 1000, NO_COST, 0).run()
    assert r["trades"][0]["entry_price"] == 105  # next open 100 + 5


def test_costs_are_applied():
    b = make_bars([100, 100, 100, 100])
    costs = Costs(commission_pct=1, tax_pct=0.5, slippage_pct=1)
    r = Backtester(b, Scripted({0: [Order("buy")], 1: [Order("sell")]}), 10_000, costs, 0).run()
    t = r["trades"][0]
    assert t["entry_price"] == pytest.approx(101) and t["exit_price"] == pytest.approx(99)
    qty = math.floor(10_000 / (101 * 1.01))
    expected_pnl = qty * 99 * (1 - 0.01 - 0.005) - qty * 101 * 1.01
    assert t["pnl"] == pytest.approx(expected_pnl)
    assert r["equity"][-1]["equity"] == pytest.approx(10_000 + expected_pnl)


def test_metrics_drawdown_and_cagr():
    times = [0, 100 * DAY, 200 * DAY, 365.25 * DAY]
    m = performance(times, [100, 120, 90, 200])
    assert m["mdd_pct"] == pytest.approx(-25)
    assert m["cagr_pct"] == pytest.approx(100)
    assert m["max_dd_days"] == 265  # from the peak (day 100) until recovery (day 365)


# -------------------------------------------------------------- strategies
def wave(n=800):
    closes = [100 + 20 * math.sin(i / 15) + 0.05 * i + 3 * math.sin(i / 2.3) for i in range(n)]
    opens = [closes[0]] + closes[:-1]
    highs = [max(o, c) + 1.5 for o, c in zip(opens, closes)]
    lows = [min(o, c) - 1.5 for o, c in zip(opens, closes)]
    return make_bars(opens, highs, lows, closes)


@pytest.mark.parametrize("sid", list(STRATEGIES))
def test_every_strategy_runs_and_is_json_safe(sid):
    s = STRATEGIES[sid]()
    r = Backtester(wave(), s, 10_000_000, Costs(), s.warmup()).run()
    assert r["metrics"]["trades"] > 0, sid
    text = json.dumps(r)
    assert "NaN" not in text and "Infinity" not in text


def test_vol_breakout_holds_exactly_one_bar():
    s = STRATEGIES["vol_breakout"]({"k": 0.3})
    r = Backtester(wave(), s, 10_000_000, NO_COST, 2).run()
    assert {t["bars"] for t in r["trades"] if not t["open"]} == {1}


def test_turtle_sizes_by_atr_risk():
    b = wave()
    s = Turtle({"risk_pct": 1})
    r = Backtester(b, s, 1e12, NO_COST, s.warmup()).run()  # huge capital: size limited by risk, not cash
    t = r["trades"][0]
    j = b.time.index(t["entry_time"])
    n = ind.atr(b.high, b.low, b.close, 20)[j - 1]
    assert t["qty"] == math.floor(1e12 * 0.01 / n)


def test_param_validation():
    from app.quant.strategies import StrategyParamError
    with pytest.raises(StrategyParamError):
        STRATEGIES["ma_cross"]({"fast": 60, "slow": 20})
    with pytest.raises(StrategyParamError):
        STRATEGIES["bollinger"]({"n": 1})


def test_indicator_fixture_is_current():
    """shared/fixtures/indicators.json must match indicators.py (the TS tests assert parity against it)."""
    assert json.loads(fixtures.FIXTURE.read_text()) == json.loads(json.dumps(fixtures.build()))


# --------------------------------------------------------------------- API
@pytest.fixture
def session(client, monkeypatch):
    monkeypatch.setattr(auth_routes, "verify_google_credential",
                        lambda c: {"sub": "q", "email": "q@example.com", "email_verified": True})
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    client.post("/api/auth/google", json={"credential": "x"})
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    client.post("/api/auth/otp/verify", json={"code": totp.now()})
    return client


def test_api_requires_login(client):
    assert client.get("/api/quant/strategies").status_code == 401


def test_api_backtest_with_mock_data(session):
    items = session.get("/api/quant/strategies").json["items"]
    assert [i["id"] for i in items] == list(STRATEGIES)
    d = session.get("/api/quant/defaults?market=KR").json
    assert d["currency"] == "KRW" and d["commission_source"] == "default"

    r = session.post("/api/quant/backtest", json={"market": "KR", "symbol": "005930", "strategy": "ma_cross",
                                                  "params": {"fast": 5, "slow": 20}, "period": "3y"})
    assert r.status_code == 200, r.json
    body = r.json
    assert body["params"] == {"fast": 5, "slow": 20}
    assert body["chart_indicators"][0]["id"] == "ma"
    assert len(body["equity"]) == len(body["candles"]) - body["start_index"]
    assert {"cagr_pct", "mdd_pct", "sharpe", "trades"} <= body["metrics"].keys()


def test_api_rejects_bad_input(session):
    bad = {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "params": {"fast": 60, "slow": 20}}
    r = session.post("/api/quant/backtest", json=bad)
    assert r.status_code == 400 and r.json["error"] == "invalid_params"
    assert session.post("/api/quant/backtest", json={**bad, "strategy": "nope"}).json["error"] == "unknown_strategy"
    r = session.post("/api/quant/backtest", json={**bad, "params": {}, "commission_pct": 50})
    assert r.json["error"] == "out_of_range"
