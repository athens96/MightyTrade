"""Tests for Quant Stock Screener and fundamental metric parsers."""
import json

import pyotp
import pytest

from fakes import approve_all
from app.auth import routes as auth_routes
from app.quant import fundamentals, screener, screener_data


@pytest.fixture
def session(client, monkeypatch):
    monkeypatch.setattr(
        auth_routes,
        "verify_google_credential",
        lambda c: {"sub": "sc", "email": "sc@example.com", "email_verified": True},
    )
    client.post("/api/auth/google", json={"credential": "x"})
    approve_all(client)
    client.post("/api/auth/google", json={"credential": "x"})
    totp = pyotp.TOTP(client.post("/api/auth/otp/setup").json["secret"])
    client.post("/api/auth/otp/verify", json={"code": totp.now()})
    return client


def test_fundamental_parsers_and_mock():
    assert fundamentals._parse_float("5.69배") == pytest.approx(5.69)
    assert fundamentals._parse_float("0.92%") == pytest.approx(0.92)
    assert fundamentals._parse_float("31,809원") == pytest.approx(31809.0)
    assert fundamentals._parse_float("적자") is None
    assert fundamentals._parse_kr_market_cap("1,057조 5,918억") == pytest.approx(1057.5918e12)

    m1 = fundamentals.get_fundamentals("US", ["AAPL", "MSFT"], use_mock=True)
    m2 = fundamentals.get_fundamentals("US", ["AAPL", "MSFT"], use_mock=True)
    assert m1 == m2
    assert m1["AAPL"]["per"] > 0 and m1["AAPL"]["roe"] > 0 and m1["AAPL"]["source"] == "mock"

    # Ordinary KR stock filter excludes ETFs, preferred shares, and SPACs
    assert screener_data._is_ordinary_kr_stock({"itemCode": "005930", "stockName": "삼성전자", "stockEndType": "stock"})
    assert not screener_data._is_ordinary_kr_stock({"itemCode": "005935", "stockName": "삼성전자우", "stockEndType": "stock"})
    assert not screener_data._is_ordinary_kr_stock({"itemCode": "069500", "stockName": "KODEX 200", "stockEndType": "etf"})
    assert not screener_data._is_ordinary_kr_stock({"itemCode": "450000", "stockName": "미래에셋스팩10호", "stockEndType": "stock"})
    rows = screener_data._FCHART_ROW_RE.findall('<item data="20261120|180000|185000|179000|181000|123456" />')
    assert rows == [("20261120", "180000|185000|179000|181000|123456")]


def test_screener_presets_endpoint_and_guides(session):
    r = session.get("/api/quant/screener/presets")
    assert r.status_code == 200
    body = r.json
    assert {p["id"] for p in body["presets"]} == set(screener.SCREENER_PRESETS)
    assert all({"background", "setup", "execution", "caution"} <= set(p["guide"]) for p in body["presets"])
    assert "KR" in body["universes"] and "US" in body["universes"]
    assert len(body["daily_strategies"]) >= 7


@pytest.mark.parametrize("preset_id", list(screener.SCREENER_PRESETS))
def test_screener_all_presets_on_kr(session, preset_id):
    r = session.post("/api/quant/screener/run", json={
        "market": "KR",
        "preset": preset_id,
        "universe": "custom",
        "symbols": "005930, 000660, 005380, 105560, 035420",
        "strategy": "rsi2",
        "top_n": 5,
    })
    assert r.status_code == 200, r.json
    body = r.json
    assert body["scanned"] == 5 and body["passed"] == 5
    assert len(body["items"]) == 5
    rank_keys = [(1 if it["matched"] else 0, it["score"]) for it in body["items"]]
    assert rank_keys == sorted(rank_keys, reverse=True)
    assert all(0 <= it["score"] <= 100 for it in body["items"])
    assert all(it["reasons"] for it in body["items"])
    text = json.dumps(body)
    assert "NaN" not in text and "Infinity" not in text


@pytest.mark.parametrize("preset_id", [p for p, m in screener.SCREENER_PRESETS.items() if not m["kr_only"]])
def test_screener_all_us_presets(session, preset_id):
    r = session.post("/api/quant/screener/run", json={
        "market": "US",
        "preset": preset_id,
        "universe": "custom",
        "symbols": ["AAPL", "MSFT", "NVDA", "JNJ", "KO"],
        "strategy": "ma_cross",
        "top_n": 4,
    })
    assert r.status_code == 200, r.json
    assert len(r.json["items"]) == 4


def test_screener_validations_and_watchlist(session):
    # KR-only smart_money rejects US
    r1 = session.post("/api/quant/screener/run", json={"market": "US", "preset": "smart_money", "universe": "us_sp100"})
    assert r1.status_code == 400 and r1.json["error"] == "kr_only_preset"

    # Legacy alias universe IDs still work (saved localStorage compatibility)
    r_alias = session.post("/api/quant/screener/run", json={"market": "KR", "preset": "momentum_quality", "universe": "kr_bluechip", "top_n": 10})
    assert r_alias.status_code == 200 and r_alias.json["universe"] == "kr_all_150" and len(r_alias.json["items"]) == 10

    # Strict-only filter returns only matched rows
    r_strict = session.post("/api/quant/screener/run", json={"market": "US", "preset": "magic_formula", "universe": "us_sp100", "strict_only": True, "top_n": 20})
    assert r_strict.status_code == 200
    assert all(it["matched"] for it in r_strict.json["items"])

    # Intraday strategy rejected for strategy_fit
    r2 = session.post("/api/quant/screener/run", json={
        "market": "US", "preset": "strategy_fit", "universe": "custom",
        "symbols": ["AAPL", "MSFT"], "strategy": "orb_breakout",
    })
    assert r2.status_code == 400 and r2.json["error"] == "daily_strategy_only"

    # Watchlist universe works with default watchlist
    r3 = session.post("/api/quant/screener/run", json={"market": "KR", "preset": "momentum_quality", "universe": "watchlist"})
    assert r3.status_code == 200 and r3.json["passed"] >= 2


def test_rebalance_bots_support_new_canary_and_core_models(session):
    from app.autotrade import bots as bot_store, rebalance

    for model_id in ("daa", "baa", "laa", "permanent", "hrp", "sector_rotation"):
        r = session.post("/api/autotrade/bots", json={
            "kind": "rebalance",
            "market": "US",
            "model": model_id,
            "capital": 10000,
        })
        assert r.status_code == 200, (model_id, r.json)
        bot_id = r.json["bot"]["id"]
        with session.application.app_context():
            bot = bot_store.get_store().get(bot_store.BOTS, bot_id)
            pr = rebalance.preview(bot, "2026-11-20")
        assert sum(pr["target_weights"].values()) == pytest.approx(100.0, abs=1.0)
