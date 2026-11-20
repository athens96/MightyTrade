"""Tests for Phase 3 multi-asset portfolio allocation theories and rebalancing backtests."""
import json
import math

import pyotp
import pytest

from fakes import approve_all
from app.auth import routes as auth_routes
from app.quant import portfolio
from app.quant.engine import Bars, Costs

DAY = 86400


def trend_bars(n, start_px, daily_drift, amp=0.0):
    closes = [start_px * ((1 + daily_drift) ** i) * (1 + amp * math.sin(i / 10)) for i in range(n)]
    opens = [closes[0]] + closes[:-1]
    return Bars([1_500_000_000 + i * DAY for i in range(n)], opens, [max(o, c) * 1.005 for o, c in zip(opens, closes)],
                [min(o, c) * 0.995 for o, c in zip(opens, closes)], closes, [1000] * n)


@pytest.mark.parametrize("model_id", list(portfolio.PORTFOLIO_MODELS))
def test_every_portfolio_model_runs_and_is_json_safe(model_id):
    model = portfolio.PORTFOLIO_MODELS[model_id]
    assets = model.presets["US"]
    bars = {
        a["symbol"]: trend_bars(600, 100.0 + idx * 10, 0.0004 if a["role"] == "offensive" else 0.0001, amp=0.05 + idx * 0.01)
        for idx, a in enumerate(assets)
    }
    res = portfolio.run_portfolio_backtest(
        bars, model_id, assets, model.resolve({}), model.default_rebalance, 5.0, 100_000, Costs(), 0
    )
    assert res["equity"] and res["rebalances"]
    assert res["metrics"]["rebalances"] == len(res["rebalances"])
    assert sum(res["current_weights"].values()) == pytest.approx(100.0, abs=1.0)
    text = json.dumps(res)
    assert "NaN" not in text and "Infinity" not in text


def test_dual_momentum_switches_to_defensive_in_bear_market():
    model = portfolio.PORTFOLIO_MODELS["dual_momentum"]
    assets = model.presets["US"]
    # Both offensive assets falling, defensive rising -> 100% defensive
    bars = {
        "SPY": trend_bars(320, 100, -0.002),
        "VEA": trend_bars(320, 100, -0.003),
        "AGG": trend_bars(320, 100, +0.0005),
    }
    res = portfolio.run_portfolio_backtest(
        bars, "dual_momentum", assets, {"lookback": 60, "top_n": 1}, "monthly", 5.0, 10_000, Costs(0, 0, 0), 0
    )
    assert res["current_weights"] == {"SPY": 0.0, "VEA": 0.0, "AGG": 100.0}


def test_vaa_canary_switches_to_defensive_when_one_offensive_is_negative():
    model = portfolio.PORTFOLIO_MODELS["vaa"]
    assets = model.presets["US"]
    # SPY/VEA/AGG positive, VWO negative -> canary triggered -> 100% best defensive (IEF)
    bars = {
        "SPY": trend_bars(300, 100, +0.001),
        "VEA": trend_bars(300, 100, +0.001),
        "VWO": trend_bars(300, 100, -0.001),  # canary!
        "AGG": trend_bars(300, 100, +0.0005),
        "SHY": trend_bars(300, 100, +0.0001),
        "IEF": trend_bars(300, 100, +0.0008),
        "LQD": trend_bars(300, 100, +0.0003),
    }
    res = portfolio.run_portfolio_backtest(
        bars, "vaa", assets, {"top_offensive": 1}, "monthly", 5.0, 10_000, Costs(0, 0, 0), 0
    )
    assert res["current_weights"]["IEF"] == 100.0
    assert res["current_weights"]["SPY"] == 0.0


def test_risk_parity_allocates_more_to_lower_volatility_asset():
    assets = [
        {"symbol": "SPY", "name": "HighVol", "role": "offensive", "weight": 50},
        {"symbol": "TLT", "name": "LowVol", "role": "defensive", "weight": 50},
    ]
    bars = {
        "SPY": trend_bars(150, 100, 0.0005, amp=0.15),  # high volatility
        "TLT": trend_bars(150, 100, 0.0002, amp=0.02),  # low volatility
    }
    res = portfolio.run_portfolio_backtest(
        bars, "risk_parity", assets, {"vol_window": 40}, "monthly", 5.0, 10_000, Costs(0, 0, 0), 0
    )
    assert res["current_weights"]["TLT"] > res["current_weights"]["SPY"]


def test_drift_rebalance_triggers_on_weight_deviation():
    assets = [
        {"symbol": "SPY", "name": "Fast", "role": "offensive", "weight": 50},
        {"symbol": "AGG", "name": "Flat", "role": "defensive", "weight": 50},
    ]
    bars = {"SPY": trend_bars(120, 100, +0.008), "AGG": trend_bars(120, 100, 0.0)}
    res = portfolio.run_portfolio_backtest(
        bars, "classic_60_40", assets, {}, "drift", 5.0, 100_000, Costs(0, 0, 0), 0
    )
    assert len(res["rebalances"]) >= 2
    assert any("편차" in r["reason"] for r in res["rebalances"][1:])


@pytest.fixture
def session(client, monkeypatch):
    monkeypatch.setattr(auth_routes, "verify_google_credential",
                        lambda c: {"sub": "p", "email": "p@example.com", "email_verified": True})
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    client.post("/api/auth/google", json={"credential": "x"})
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    client.post("/api/auth/otp/verify", json={"code": totp.now()})
    return client


def test_api_portfolio_models_and_backtest(session):
    r = session.get("/api/quant/portfolio/models")
    assert r.status_code == 200
    assert {m["id"] for m in r.json["items"]} == set(portfolio.PORTFOLIO_MODELS)
    assert len(r.json["items"]) == 12
    assert all({"background", "setup", "execution", "caution"} <= set(m["guide"]) for m in r.json["items"])
    rb = session.post("/api/quant/portfolio/backtest", json={
        "market": "US", "model": "all_weather", "period": "3y", "rebalance": "quarterly",
    })
    assert rb.status_code == 200, rb.json
    body = rb.json
    assert body["model"] == "all_weather" and body["equity"] and body["rebalances"]
    assert set(body["current_weights"]) == {"SPY", "TLT", "IEF", "GLD", "DBC"}


def test_daa_scales_protection_by_canary_count():
    assets = [
        {"symbol": "C1", "name": "Canary1", "role": "canary", "weight": 0},
        {"symbol": "C2", "name": "Canary2", "role": "canary", "weight": 0},
        {"symbol": "O1", "name": "Off1", "role": "offensive", "weight": 0},
        {"symbol": "O2", "name": "Off2", "role": "offensive", "weight": 0},
        {"symbol": "D1", "name": "Def1", "role": "defensive", "weight": 0},
    ]
    # 1 bad canary out of 2 -> 50% defensive, 50% split across top 2 offensive
    closes = {
        "C1": [100 * (1.001 ** i) for i in range(260)],
        "C2": [100 * (0.998 ** i) for i in range(260)],
        "O1": [100 * (1.002 ** i) for i in range(260)],
        "O2": [100 * (1.0015 ** i) for i in range(260)],
        "D1": [100 * (1.0003 ** i) for i in range(260)],
    }
    w_half = portfolio._daa_weights(closes, 259, assets, {"top_offensive": 2, "canary_threshold": 2})
    assert w_half["D1"] == pytest.approx(0.5)
    assert w_half["O1"] == pytest.approx(0.25)
    assert w_half["O2"] == pytest.approx(0.25)

    # Both canaries bad -> 100% defensive
    closes["C1"] = [100 * (0.998 ** i) for i in range(260)]
    w_full = portfolio._daa_weights(closes, 259, assets, {"top_offensive": 2, "canary_threshold": 2})
    assert w_full["D1"] == pytest.approx(1.0)
    assert w_full["O1"] == 0.0


def test_baa_switches_to_defensive_and_avoids_falling_bonds():
    assets = [
        {"symbol": "C1", "name": "Canary", "role": "canary", "weight": 0},
        {"symbol": "O1", "name": "Off", "role": "offensive", "weight": 0},
        {"symbol": "BIL", "name": "Cash", "role": "defensive", "weight": 0},
        {"symbol": "DBC", "name": "Cmdty", "role": "defensive", "weight": 0},
        {"symbol": "TLT", "name": "FallBond", "role": "defensive", "weight": 0},
    ]
    closes = {
        "C1": [100 * (0.999 ** i) for i in range(260)],
        "O1": [100 * (1.001 ** i) for i in range(260)],
        "BIL": [100 * (1.0002 ** i) for i in range(260)],
        "DBC": [100 * (1.0005 ** i) for i in range(260)],
        "TLT": [100 * (0.998 ** i) for i in range(260)],  # negative SMA12 -> replaced by top defensive (DBC)
    }
    w = portfolio._baa_weights(closes, 259, assets, {"top_offensive": 1, "top_defensive": 3})
    assert w["O1"] == 0.0
    assert w["TLT"] == 0.0
    assert w["DBC"] == pytest.approx(2 / 3)
    assert w["BIL"] == pytest.approx(1 / 3)


def test_laa_keeps_core_75_and_switches_25_sleeve_by_trend():
    assets = portfolio.PORTFOLIO_MODELS["laa"].presets["US"]
    # Bull trend on SPY -> 25% QQQ, 0% SHY
    bull = {
        "SPY": [100 * (1.001 ** i) for i in range(220)],
        "IEF": [100.0] * 220,
        "GLD": [100.0] * 220,
        "QQQ": [100.0] * 220,
        "SHY": [100.0] * 220,
    }
    wb = portfolio._laa_weights(bull, 219, assets, {"trend_days": 200})
    assert wb == {"SPY": 0.25, "IEF": 0.25, "GLD": 0.25, "QQQ": 0.25, "SHY": 0.0}

    # Bear trend on SPY -> 0% QQQ, 25% SHY
    bear = {**bull, "SPY": [100 * (0.998 ** i) for i in range(220)]}
    wr = portfolio._laa_weights(bear, 219, assets, {"trend_days": 200})
    assert wr == {"SPY": 0.25, "IEF": 0.25, "GLD": 0.25, "QQQ": 0.0, "SHY": 0.25}


def test_hrp_avoids_double_counting_correlated_assets():
    assets = [
        {"symbol": "SPY", "name": "S&P", "role": "offensive", "weight": 0},
        {"symbol": "QQQ", "name": "NDX", "role": "offensive", "weight": 0},
        {"symbol": "TLT", "name": "Bond", "role": "defensive", "weight": 0},
    ]
    # SPY & QQQ move nearly identically; TLT is orthogonal with same amplitude
    n = 120
    spy = [100.0]
    qqq = [100.0]
    tlt = [100.0]
    for i in range(1, n):
        r_eq = 0.015 * math.sin(i * 0.7)
        r_bd = 0.015 * math.cos(i * 1.9)
        spy.append(spy[-1] * (1 + r_eq))
        qqq.append(qqq[-1] * (1 + r_eq * 0.99 + 0.0005 * math.cos(i)))
        tlt.append(tlt[-1] * (1 + r_bd))
    w = portfolio._hrp_weights({"SPY": spy, "QQQ": qqq, "TLT": tlt}, n - 1, assets, {"vol_window": 80})
    assert sum(w.values()) == pytest.approx(1.0)
    # TLT should receive ~50% while SPY + QQQ share the other ~50% (~25% each)
    assert w["TLT"] > 0.42
    assert w["SPY"] < 0.32 and w["QQQ"] < 0.32


def test_sector_rotation_picks_uptrend_and_parks_downtrend_in_defensive():
    assets = [
        {"symbol": "XLK", "name": "Tech", "role": "offensive", "weight": 0},
        {"symbol": "XLE", "name": "Energy", "role": "offensive", "weight": 0},
        {"symbol": "XLF", "name": "Fin", "role": "offensive", "weight": 0},
        {"symbol": "SHY", "name": "Cash", "role": "defensive", "weight": 0},
    ]
    closes = {
        "XLK": [100 * (1.002 ** i) for i in range(220)],   # strong uptrend
        "XLE": [100 * (0.998 ** i) for i in range(220)],   # downtrend
        "XLF": [100 * (0.999 ** i) for i in range(220)],   # downtrend
        "SHY": [100 * (1.0001 ** i) for i in range(220)],  # cash
    }
    w = portfolio._sector_rotation_weights(closes, 219, assets, {"lookback": 126, "ma_days": 200, "top_n": 2})
    assert w["XLK"] == pytest.approx(0.5)
    assert w["SHY"] == pytest.approx(0.5)
    assert w["XLE"] == 0.0 and w["XLF"] == 0.0


def test_validate_assets_enforces_canary_and_core_roles():
    with pytest.raises(Exception) as e1:
        portfolio.validate_assets("US", [
            {"symbol": "SPY", "role": "offensive", "weight": 0},
            {"symbol": "SHY", "role": "defensive", "weight": 0},
        ], portfolio.PORTFOLIO_MODELS["daa"])
    assert "감시병" in str(e1.value)

    with pytest.raises(Exception) as e2:
        portfolio.validate_assets("US", [
            {"symbol": "QQQ", "role": "offensive", "weight": 0},
            {"symbol": "SHY", "role": "defensive", "weight": 0},
        ], portfolio.PORTFOLIO_MODELS["laa"])
    assert "고정" in str(e2.value)


def test_align_bars_matches_different_time_of_day_timestamps():
    # Yahoo US open timestamp (14:30 UTC) vs Toss midnight timestamp (00:00 UTC) on identical calendar dates
    day0 = 1_700_006_400  # 2023-11-15 00:00:00 UTC
    b_toss = Bars([day0 + i * DAY for i in range(10)], [100] * 10, [101] * 10, [99] * 10, [100] * 10, [1000] * 10)
    b_yahoo = Bars([day0 + i * DAY + 14 * 3600 + 1800 for i in range(10)], [50] * 10, [51] * 10, [49] * 10, [50] * 10, [1000] * 10)
    times, opens, closes = portfolio.align_bars({"A": b_toss, "B": b_yahoo})
    assert len(times) == 10
    assert len(opens["A"]) == 10 and len(closes["B"]) == 10
