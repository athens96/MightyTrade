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
    assert all({"background", "setup", "execution", "caution"} <= set(m["guide"]) for m in r.json["items"])
    rb = session.post("/api/quant/portfolio/backtest", json={
        "market": "US", "model": "all_weather", "period": "3y", "rebalance": "quarterly",
    })
    assert rb.status_code == 200, rb.json
    body = rb.json
    assert body["model"] == "all_weather" and body["equity"] and body["rebalances"]
    assert set(body["current_weights"]) == {"SPY", "TLT", "IEF", "GLD", "DBC"}
