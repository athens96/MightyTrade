"""Tests for MT-6: Screener-linked automatic stock rotation bot ('자동 순환매 랭킹 봇') & backtest."""
from app.autotrade import bots, executor, rotation
from app.quant.engine import Costs
from fakes import login


def test_select_target_picks_respects_hold_buffer():
    ranked = [
        {"symbol": "A", "name": "A", "rank": 1, "score": 95},
        {"symbol": "B", "name": "B", "rank": 2, "score": 92},
        {"symbol": "C", "name": "C", "rank": 3, "score": 88},
        {"symbol": "D", "name": "D", "rank": 4, "score": 84},
        {"symbol": "E", "name": "E", "rank": 5, "score": 80},
        {"symbol": "F", "name": "F", "rank": 8, "score": 70},
        {"symbol": "G", "name": "G", "rank": 12, "score": 55},
    ]
    # Currently holding F (rank 8 <= buffer 10 -> KEEP!) and G (rank 12 > buffer 10 -> DROP!)
    selected, dropped = rotation.select_target_picks(ranked, ["F", "G"], top_n=3, hold_buffer=10)
    syms = [r["symbol"] for r in selected]
    assert "F" in syms
    assert "G" not in syms and dropped == ["G"]
    assert syms == ["F", "A", "B"]


def test_rotation_bot_paper_lifecycle_and_rotation_backtest_api(client, monkeypatch):
    login(client, monkeypatch, sub="rot_u")
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setattr("app.autotrade.routes.target_date", lambda m, n: "2026-11-20")
    monkeypatch.setattr(rotation, "_costs", lambda m: Costs(0, 0, 0))

    # 1. Create rotation bot via API
    r = client.post("/api/autotrade/bots", json={
        "kind": "rotation",
        "market": "KR",
        "preset": "momentum_quality",
        "universe": "custom",
        "custom_symbols": ["005930", "000660", "005380", "105560"],
        "top_n": 2,
        "hold_buffer": 3,
        "rebalance": "weekly",
        "capital": 10_000_000,
    })
    assert r.status_code == 200
    bot = r.json["bot"]
    assert bot["kind"] == "rotation" and bot["category"] == "rotation"
    bid = bot["id"]
    uid = bot["user_id"]

    # 2. Evaluate preview
    ev = client.post(f"/api/autotrade/bots/{bid}/evaluate")
    assert ev.status_code == 200
    assert ev.json["due"] is True
    assert len(ev.json["selected"]) == 2

    # 3. Run paper rotation on mock provider's latest trading day
    with client.application.app_context():
        from datetime import datetime, timezone
        from app.market.models import Interval, Market
        from app.market.registry import get_provider
        cs = get_provider(Market.KR).get_history(Market.KR, "005930", Interval.D1, 0)
        today = datetime.fromtimestamp(cs[-1].time, timezone.utc).date().isoformat()
        res = executor.run(bots.get(uid, bid), today)
        assert res["rotated"] is True
        updated = bots.public(bots.get(uid, bid))
        assert len(updated["state"]["positions"]) == 2
        assert updated["state"]["last_rebalance_date"] == today

    # 4. Historical rotation backtest endpoint
    bt = client.post("/api/quant/screener/rotation-backtest", json={
        "market": "KR",
        "preset": "momentum_quality",
        "universe": "custom",
        "symbols": "005930, 000660, 005380, 105560",
        "top_n": 2,
        "hold_buffer": 3,
        "cadence": "monthly",
        "period": "1y",
    })
    assert bt.status_code == 200
    assert bt.json["metrics"]["rebalances"] >= 1
    assert len(bt.json["equity"]) > 10
