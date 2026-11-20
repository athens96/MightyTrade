"""Phase 2: optimisation grid, walk-forward, Monte Carlo."""
import math

import pyotp
import pytest

from fakes import approve_all
from app.auth import routes as auth_routes
from app.errors import ApiError
from app.quant import montecarlo, optimize
from app.quant.engine import Bars, Costs, Order
from app.quant.optimize import Axis, combos, parse_axis, run_grid
from app.quant.strategies import STRATEGIES, Param, Strategy
from app.quant.walkforward import make_windows, walk_forward

DAY = 86400
MA = STRATEGIES["ma_cross"]


def daily_bars(n=2600):
    closes = [100 + 20 * math.sin(i / 15) + 0.05 * i + 3 * math.sin(i / 2.3) for i in range(n)]
    opens = [closes[0]] + closes[:-1]
    return Bars([1_500_000_000 + i * DAY for i in range(n)], opens,
                [max(o, c) + 1.5 for o, c in zip(opens, closes)], [min(o, c) - 1.5 for o, c in zip(opens, closes)],
                closes, [1000] * n)


# ------------------------------------------------------------------ axes / grid
def test_parse_axis_values_and_limits():
    assert parse_axis(MA, {"key": "fast", "start": 5, "stop": 20, "step": 5}).values == [5, 10, 15, 20]
    assert parse_axis(MA, {"key": "fast", "start": 5, "stop": 6, "step": 0.4}).values == [5, 6]  # int params dedupe
    with pytest.raises(ApiError):
        parse_axis(MA, {"key": "fast", "start": 1, "stop": 5, "step": 1})  # below min 2
    with pytest.raises(ApiError):
        parse_axis(MA, {"key": "fast", "start": 2, "stop": 200, "step": 1})  # > 25 values
    with pytest.raises(ApiError):
        parse_axis(MA, {"key": "nope", "start": 1, "stop": 2, "step": 1})


def test_invalid_combinations_are_marked_not_run():
    x = Axis("fast", "단기", [10, 30])
    y = Axis("slow", "장기", [20, 40])
    items = combos(MA, MA.resolve({}), x, y)
    r = run_grid(daily_bars(800), items, x, y, 1e7, Costs(), 60, "sharpe", 0)
    assert "invalid" in r["cells"][0][1]  # fast 30 >= slow 20
    assert r["cells"][1][1]["params"] == {"fast": 30, "slow": 40}


def test_best_vs_robust_prefers_plateau(monkeypatch):
    """A lone spike wins 'best'; the cell inside a high plateau wins 'robust'."""
    def fake_eval(bars, strategy, capital, costs, start):
        f, s = strategy.p["fast"], strategy.p["slow"]
        score = 10 if (f, s) == (5, 50) else 6 if f >= 30 and s >= 70 else 0
        return {"sharpe": score, "trades": 50}
    monkeypatch.setattr(optimize, "evaluate", fake_eval)
    x = Axis("fast", "단기", [5, 10, 20, 30, 40, 50])
    y = Axis("slow", "장기", [50, 60, 70, 80, 90])
    r = run_grid(daily_bars(100), combos(MA, MA.resolve({}), x, y), x, y, 1e7, Costs(), 0, "sharpe", 5)
    assert r["best"]["params"] == {"fast": 5, "slow": 50}
    assert r["robust"]["params"]["fast"] >= 30 and r["robust"]["params"]["slow"] >= 70
    assert r["robust"]["neighbor_avg"] == 6


def test_min_trades_excludes_cells_from_best(monkeypatch):
    monkeypatch.setattr(optimize, "evaluate", lambda b, st, c, co, s: {
        "sharpe": 9 if st.p["fast"] == 5 else 1, "trades": 2 if st.p["fast"] == 5 else 20})
    x = Axis("fast", "단기", [5, 10, 15])
    r = run_grid(daily_bars(100), combos(MA, MA.resolve({}), x, None), x, None, 1e7, Costs(), 0, "sharpe", 5)
    assert r["best"]["params"]["fast"] != 5 and r["cells"][0][0]["eligible"] is False


# ------------------------------------------------------------------ walk-forward
def test_windows_rolling_and_anchored():
    times = [i * DAY for i in range(int(365.25 * 6))]
    rolling = make_windows(times, 0, 2, 12, "rolling")
    anchored = make_windows(times, 0, 2, 12, "anchored")
    assert len(rolling) == 4 and len(anchored) == 4
    for (a_is, a_end, a_oos), (b_is, b_end, b_oos) in zip(rolling, rolling[1:]):
        assert b_end == a_oos  # OOS segments are contiguous
        assert b_is > a_is  # rolling IS slides
    assert all(w[0] == 0 for w in anchored)  # anchored IS always starts at the beginning


class Spy(Strategy):
    """Records the data length it is given; trades every 10 bars."""
    id, name, summary, description = "spy", "spy", "spy", "spy"
    params = [Param("n", "n", 1, 1, 5)]
    seen: list = []

    def warmup(self):
        return 1

    def prepare(self, bars):
        Spy.seen.append(len(bars))

    def on_bar(self, i, ctx):
        if not ctx.position.is_open and i % 10 == 0:
            return [Order("buy")]
        if ctx.position.is_open and i % 10 == 5:
            return [Order("sell")]
        return []


def test_walk_forward_never_sees_future_data_and_is_contiguous():
    b = daily_bars()
    x = Axis("n", "n", [1, 2])
    items = combos(Spy, Spy.resolve({}), x, None)
    windows = make_windows(b.time, 10, 2, 12, "rolling")
    Spy.seen = []
    r = walk_forward(b, items, x, None, 1e7, Costs(), 10, "total_return_pct", 0, 2, 12, "rolling")
    allowed = sorted({w[1] for w in windows} | {w[2] for w in windows})  # IS ends and OOS ends
    assert set(Spy.seen) <= set(allowed)  # every run only had data up to its window's end
    assert len(r["windows"]) == len(windows)
    eq = r["equity"]
    assert eq[0]["time"] == b.time[windows[0][1]] and eq[0]["equity"] == 1e7
    assert len(eq) == windows[-1][2] - windows[0][1]  # one point per OOS bar, no gaps/overlaps
    assert [p["time"] for p in eq] == b.time[windows[0][1]:windows[-1][2]]


# ------------------------------------------------------------------ monte carlo
def test_shuffle_keeps_final_return_bootstrap_spreads():
    impacts = [0.05, -0.02, 0.03, -0.04, 0.06, 0.01, -0.01, 0.02]
    sh = montecarlo.simulate(impacts, runs=300, method="shuffle")
    final = (math.prod(1 + r for r in impacts) - 1) * 100
    assert sh["final_return_pct"]["p5"] == pytest.approx(final) == pytest.approx(sh["final_return_pct"]["p95"])
    assert sh["mdd_pct"]["p5"] < sh["mdd_pct"]["p95"] <= 0  # same trades, different order -> different drawdowns
    bs = montecarlo.simulate(impacts, runs=500, method="bootstrap")
    p = bs["final_return_pct"]
    assert p["p5"] < p["p50"] < p["p95"]
    assert all(v <= 0 for v in bs["mdd_pct"].values())
    assert len(bs["bands"]["p50"]) == len(bs["path_index"]) == len(impacts) + 1


def test_all_winning_trades_never_lose():
    r = montecarlo.simulate([0.01] * 10, runs=200)
    assert r["prob_loss_pct"] == 0 and r["mdd_pct"]["p5"] == 0


def test_monte_carlo_needs_trades():
    with pytest.raises(ApiError):
        montecarlo.simulate([0.01, 0.02], runs=100)


def test_trade_impact_uses_equity_before_entry():
    equity = [{"time": 1, "equity": 1000}, {"time": 2, "equity": 2000}, {"time": 3, "equity": 2100}]
    trades = [{"open": False, "pnl": 100, "entry_time": 2}, {"open": True, "pnl": 5, "entry_time": 3}]
    assert montecarlo.trade_impacts(trades, equity) == [0.1]  # 100 / equity at t=1


# ------------------------------------------------------------------ API
@pytest.fixture
def session(client, monkeypatch):
    monkeypatch.setattr(auth_routes, "verify_google_credential",
                        lambda c: {"sub": "v", "email": "v@example.com", "email_verified": True})
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    client.post("/api/auth/google", json={"credential": "x"})
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    client.post("/api/auth/otp/verify", json={"code": totp.now()})
    return client


BASE = {"market": "KR", "symbol": "005930", "strategy": "ma_cross", "period": "10y"}


def test_api_optimize(session):
    r = session.post("/api/quant/optimize", json={**BASE, "x": {"key": "fast", "start": 5, "stop": 20, "step": 5},
                                                  "y": {"key": "slow", "start": 30, "stop": 60, "step": 10}})
    assert r.status_code == 200, r.json
    b = r.json
    assert len(b["cells"]) == 4 and len(b["cells"][0]) == 4
    assert b["best"] is not None and b["objective"] == "sharpe"
    assert {"cagr_pct", "mdd_pct"} <= b["benchmark"].keys()


def test_api_optimize_limits(session):
    big = {"key": "fast", "start": 2, "stop": 26, "step": 1}
    r = session.post("/api/quant/optimize", json={**BASE, "x": big, "y": {"key": "slow", "start": 30, "stop": 54, "step": 1}})
    assert r.status_code == 400 and r.json["error"] == "range_too_large"
    r = session.post("/api/quant/optimize", json={**BASE, "x": {"key": "fast", "start": 5, "stop": 10, "step": 5},
                                                  "y": {"key": "fast", "start": 5, "stop": 10, "step": 5}})
    assert r.json["error"] == "invalid_range"


def test_api_walkforward_and_montecarlo(session):
    r = session.post("/api/quant/walkforward", json={**BASE, "x": {"key": "fast", "start": 5, "stop": 20, "step": 5},
                                                     "is_years": 2, "oos_months": 12})
    assert r.status_code == 200, r.json
    assert len(r.json["windows"]) >= 5 and r.json["equity"]
    r = session.post("/api/quant/montecarlo", json={**BASE, "strategy": "rsi2", "runs": 300})
    assert r.status_code == 200, r.json
    assert r.json["runs"] == 300 and set(r.json["final_return_pct"]) == {"p5", "p25", "p50", "p75", "p95"}
