"""Tests for MT-9: Unified bot control tower overview & Paper-vs-Live execution slippage scorecard."""
import time

from app.account import toss_account
from app.autotrade import bots, executor
from fakes import FakeToss, login


def test_overview_separates_paper_and_live_detects_duplicates_and_tracks_slippage(client, monkeypatch):
    totp = login(client, monkeypatch, sub="ov_u")
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})

    # 1. Create two bots targeting overlapping symbol 005930 (one paper, one live)
    b1 = client.post("/api/autotrade/bots", json={
        "name": "모의 삼성 스윙",
        "market": "KR",
        "symbol": "005930",
        "strategy": "ma_cross",
        "capital": 1_000_000,
    }).json["bot"]
    b2 = client.post("/api/autotrade/bots", json={
        "name": "실전 삼성 스윙",
        "market": "KR",
        "symbol": "005930",
        "strategy": "rsi2",
        "capital": 2_000_000,
    }).json["bot"]
    client.post(f"/api/autotrade/bots/{b2['id']}/mode", json={"mode": "live", "otp_code": totp.at(time.time() + 30)})

    uid = client.get("/api/auth/me").json["user"]["id"]
    fake = FakeToss({
        ("GET", "/api/v1/orders/ord-slip-1"): {
            "orderId": "ord-slip-1",
            "status": "FILLED",
            "execution": {"filledQuantity": 10, "averageFilledPrice": "10020"},
        },
    })
    for mod in (executor, toss_account):
        monkeypatch.setattr(mod, "_account", lambda: (fake, 7))

    # Seed paper equity on b1 and reconcile a live order on b2 with signal_price=10000 -> fill=10020 (+20 bps)
    with client.application.app_context():
        rec1 = bots.get(uid, b1["id"])
        rec1["state"]["paper"]["cash"] = 1_050_000.0
        rec1["state"]["paper"]["last_run_date"] = "2026-11-20"
        rec1["state"]["paper"]["equity"] = [
            {"date": "2026-11-19", "value": 1_000_000.0},
            {"date": "2026-11-20", "value": 1_050_000.0},
        ]
        bots.save(rec1)

        rec2 = bots.get(uid, b2["id"])
        rec2["state"]["paper"]["cash"] = 2_100_000.0
        rec2["state"]["paper"]["last_run_date"] = "2026-11-18"
        rec2["state"]["paper"]["equity"] = [{"date": "2026-11-18", "value": 2_100_000.0}]
        rec2["state"]["live"]["pending"].append({
            "kind": "order",
            "id": "ord-slip-1",
            "symbol": "005930",
            "side": "BUY",
            "qty": 10,
            "date": "2026-11-20",
            "synced": 0.0,
            "signal_price": 10000.0,
        })
        executor.reconcile_live(rec2, "2026-11-20")
        rec2["state"]["live"]["last_run_date"] = "2026-11-20"
        rec2["state"]["live"]["equity"] = [
            {"date": "2026-11-19", "value": 2_000_000.0},
            {"date": "2026-11-20", "value": 2_040_000.0},
        ]
        bots.save(rec2)

    ov = client.get("/api/autotrade/overview")
    assert ov.status_code == 200
    data = ov.json
    assert data["total_bots"] == 2
    assert data["paper"]["bots"] == 1
    assert data["paper"]["equity_krw"] == 1_050_000
    assert data["paper"]["return_pct"] == 5.0

    assert data["live"]["bots"] == 1
    assert data["live"]["equity_krw"] == 2_040_000
    assert data["live"]["return_pct"] == 2.0

    # Duplicate symbol 005930 detected across both bots
    assert len(data["duplicate_symbols"]) == 1
    assert data["duplicate_symbols"][0]["symbol"] == "005930"
    assert len(data["duplicate_symbols"][0]["bots"]) == 2

    # Slippage recorded accurately: (10020 / 10000 - 1) * 10000 = +20.0 bps
    assert data["execution"]["total_fills"] == 1
    assert data["execution"]["avg_slippage_bps"] == 20.0
    row2 = next(x for x in data["execution"]["items"] if x["bot_id"] == b2["id"])
    assert row2["paper_return_pct"] == 5.0
    assert row2["live_return_pct"] == 2.0
    assert row2["return_gap_pct"] == -3.0
    assert row2["avg_slippage_bps"] == 20.0
