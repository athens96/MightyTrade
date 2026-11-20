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


def test_limit_order_fills_at_limit_or_better_gap():
    # bar1 low=96 > limit 95 (no fill); bar2 low=94 <= 95 (fill at 95); bar3 sells at open=100; bar4 gaps down to 90 (fill at 90)
    b = make_bars([100, 100, 100, 100, 90], highs=[101, 101, 101, 101, 95], lows=[99, 96, 94, 99, 89])
    s = Scripted({0: [Order("buy", kind="limit", limit=95)], 1: [Order("buy", kind="limit", limit=95)],
                  2: [Order("sell")], 3: [Order("buy", kind="limit", limit=95)]})
    r = Backtester(b, s, 1000, NO_COST, 0).run()
    assert r["trades"][0]["entry_time"] == b.time[2] and r["trades"][0]["entry_price"] == 95
    assert r["trades"][0]["exit_price"] == 100
    assert r["trades"][1]["entry_time"] == b.time[4] and r["trades"][1]["entry_price"] == 90



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
    highs = [max(o, c) + 0.2 + abs(math.sin(i * 1.7)) * 1.5 for i, (o, c) in enumerate(zip(opens, closes))]
    lows = [min(o, c) - 0.2 - abs(math.cos(i * 1.7)) * 1.5 for i, (o, c) in enumerate(zip(opens, closes))]
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


def test_turtle_guards_nonpositive_entry_index_and_uses_entry_atr():
    from app.quant.engine import Context, Position
    # ATR grows drastically at the end; entry_index <= 0 must NEVER wrap around to self.atr[-1]
    opens = [100] * 30 + [100, 120, 140, 160, 180]
    highs = [101] * 30 + [105, 140, 160, 200, 220]
    lows = [99] * 30 + [95, 110, 130, 150, 160]
    closes = [100] * 30 + [104, 135, 155, 195, 210]
    b = make_bars(opens, highs, lows, closes)
    s = Turtle({"entry": 10, "exit": 5, "atr_n": 5, "stop_atr": 2, "risk_pct": 1})
    s.prepare(b)
    last = len(b) - 1
    # entry_index = 0 or -1 without entry_atr -> falls back to Donchian exit channel (no negative-index wrap)
    for bad_idx in (0, -1):
        orders = s.on_bar(last, Context(Position(10, 100.0, bad_idx), 0.0, 2100.0))
        assert len(orders) == 1 and orders[0].stop == s.ll[last] and "신저가" in orders[0].reason
    # explicit entry_atr overrides missing entry_index
    orders = s.on_bar(last, Context(Position(10, 200.0, -1, entry_atr=2.0), 0.0, 2100.0))
    assert len(orders) == 1 and orders[0].stop == pytest.approx(196.0) and "손절" in orders[0].reason


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
    assert all({"category", "timeframe", "theory", "guide"} <= set(i) for i in items)
    assert all({"background", "setup", "execution", "caution"} <= set(i["guide"]) for i in items)
    assert {i["category"] for i in items} == {"swing", "daytrade"}
    d = session.get("/api/quant/defaults?market=KR").json
    assert d["currency"] == "KRW" and d["commission_source"] == "default"

    r = session.post("/api/quant/backtest", json={"market": "KR", "symbol": "005930", "strategy": "ma_cross",
                                                  "params": {"fast": 5, "slow": 20}, "period": "3y"})
    assert r.status_code == 200, r.json
    body = r.json
    assert body["params"] == {"fast": 5, "slow": 20} and body["interval"] == "1d"
    assert body["chart_indicators"][0]["id"] == "ma"
    assert len(body["equity"]) == len(body["candles"]) - body["start_index"]
    assert {"cagr_pct", "mdd_pct", "sharpe", "trades"} <= body["metrics"].keys()

    # Intraday daytrade strategy backtest uses 15m / 5m bars
    r_orb = session.post("/api/quant/backtest", json={"market": "KR", "symbol": "005930", "strategy": "orb_breakout"})
    assert r_orb.status_code == 200 and r_orb.json["interval"] == "15m" and r_orb.json["candles"]
    r_vwap = session.post("/api/quant/backtest", json={"market": "US", "symbol": "AAPL", "strategy": "vwap_reversion"})
    assert r_vwap.status_code == 200 and r_vwap.json["interval"] == "5m" and r_vwap.json["candles"]


def test_api_rejects_bad_input(session):
    bad = {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "params": {"fast": 60, "slow": 20}}
    r = session.post("/api/quant/backtest", json=bad)
    assert r.status_code == 400 and r.json["error"] == "invalid_params"
    assert session.post("/api/quant/backtest", json={**bad, "strategy": "nope"}).json["error"] == "unknown_strategy"
    r = session.post("/api/quant/backtest", json={**bad, "params": {}, "commission_pct": 50})
    assert r.json["error"] == "out_of_range"

    # Walk-forward rejects intraday strategies with clear guidance
    r_wf = session.post("/api/quant/walkforward", json={
        "market": "US", "symbol": "AAPL", "strategy": "orb_breakout",
        "x": {"key": "open_bars", "start": 1, "stop": 3, "step": 1},
    })
    assert r_wf.status_code == 400 and r_wf.json["error"] == "daily_strategy_only"


def test_intraday_daytrade_strategies_flatten_same_session():
    """ORB (15m) and VWAP (5m) must close before session roll — never hold overnight."""
    # Build 4 realistic 15m sessions (26 bars per day, 18h overnight gap between days)
    times, opens, highs, lows, closes, vols = [], [], [], [], [], []
    base_t = 1_700_000_000
    for day in range(4):
        day_start = base_t + day * DAY
        for b in range(26):
            t = day_start + b * 900
            px = 100.0 + (b * 0.4 if b >= 2 else 0.1 * (b % 2))
            if b == 6:
                px = 96.0  # dip below VWAP
            times.append(t)
            opens.append(px)
            highs.append(px + 0.6)
            lows.append(px - 0.6)
            closes.append(px + 0.2)
            vols.append(1000.0)
    bars = Bars(times, opens, highs, lows, closes, vols)
    sess = dict(zip(times, ind.session_buckets(times)))
    for sid in ("orb_breakout", "vwap_reversion"):
        s = STRATEGIES[sid]()
        r = Backtester(bars, s, 100_000, NO_COST, s.warmup()).run()
        closed = [tr for tr in r["trades"] if not tr["open"]]
        assert closed, f"{sid} should trade on synthetic intraday sessions"
        for tr in closed:
            assert sess[tr["entry_time"]] == sess[tr["exit_time"]], f"{sid} held overnight: {tr}"
