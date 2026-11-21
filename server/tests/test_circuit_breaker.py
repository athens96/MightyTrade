"""Tests for MT-4: Bot circuit breakers (max drawdown, consecutive losses), cooldowns, and daily loss limits."""
from datetime import date, datetime, timedelta, timezone

from app.autotrade import bots, executor, notifier
from app.market.models import Candle
from app.quant.engine import Costs, Order
from app.quant.strategies import STRATEGIES, Strategy
from app.users import credentials
from fakes import login


class ScriptedCB(Strategy):
    id, name, summary, description, params = "scripted_cb", "CB테스트", "s", "s", []
    script: dict = {}

    def warmup(self):
        return 1

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        return list(ScriptedCB.script.get(executor.day_of(self.bars.time[i]), []))


def _candles(overrides=None):
    days = [d for d in (date(2026, 11, 2) + timedelta(days=i) for i in range(25)) if d.weekday() < 5]
    out = []
    for d in days:
        o, h, l, c = (overrides or {}).get(d.isoformat(), (100, 101, 99, 100))
        ts = int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())
        out.append(Candle(ts, o, h, l, c, 1000))
    return out


def test_consecutive_losses_cooldown_and_reenable_reset(client, monkeypatch):
    login(client, monkeypatch, sub="cb_u")
    monkeypatch.setattr("app.autotrade.routes.credentials.get_toss", lambda uid: {"account_seq": 7})
    monkeypatch.setitem(STRATEGIES, "scripted_cb", ScriptedCB)
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))

    alerts = []
    monkeypatch.setattr(notifier, "_send_http", lambda url, payload: alerts.append(payload))

    overrides = {
        "2026-11-10": (100, 101, 99, 100),  # buy 1 @ 100
        "2026-11-11": (96, 97, 95, 96),     # sell 1 @ 96 (loss #1 -> triggers 2-day cooldown until 2026-11-13!)
        "2026-11-12": (96, 97, 95, 96),     # buy attempted on 11-12 -> blocked by cooldown!
        "2026-11-16": (100, 101, 99, 100),  # buy 2 @ 100 (after cooldown expired)
        "2026-11-17": (95, 96, 94, 95),     # sell 2 @ 95 (loss #2 -> trips max_consecutive_losses=2!)
    }
    monkeypatch.setattr(executor, "get_provider", lambda m: type("P", (), {"get_history": lambda self, *a: _candles(overrides)})())

    ScriptedCB.script = {
        "2026-11-09": [Order("buy")],
        "2026-11-10": [Order("sell")],
        "2026-11-11": [Order("buy")],   # should be suppressed on 11-12 due to cooldown!
        "2026-11-13": [Order("buy")],   # executes on Monday 11-16!
        "2026-11-16": [Order("sell")],  # executes on 11-17 -> 2nd consecutive loss!
    }

    client.put("/api/users/me/notifications", json={
        "enabled": True,
        "paper_notify": True,
        "discord_webhook_url": "https://discord.com/api/webhooks/111/cb",
        "events": {"order": False, "fill": False, "error": False, "halt": True, "briefing": False},
    })

    r = client.post("/api/autotrade/bots", json={
        "market": "KR",
        "symbol": "005930",
        "strategy": "scripted_cb",
        "capital": 10_000,
        "guard": {"max_drawdown_pct": 15.0, "max_consecutive_losses": 2, "cooldown_days": 2, "daily_loss_limit_pct": 0},
    })
    assert r.status_code == 200
    bid = r.json["bot"]["id"]
    uid = r.json["bot"]["user_id"]
    client.post(f"/api/autotrade/bots/{bid}/enable")

    with client.application.app_context():
        executor.run(bots.get(uid, bid), "2026-11-10")  # buy #1
        executor.run(bots.get(uid, bid), "2026-11-11")  # sell #1 (loss) -> sets cooldown_until=2026-11-13
        b_after_loss1 = bots.get(uid, bid)
        assert b_after_loss1["state"]["paper"]["cooldown_until"] == "2026-11-13"
        assert b_after_loss1["enabled"] is True

        executor.run(bots.get(uid, bid), "2026-11-12")  # buy blocked by cooldown!
        assert bots.get(uid, bid)["state"]["paper"]["position"]["qty"] == 0

        executor.run(bots.get(uid, bid), "2026-11-16")  # cooldown over -> buy #2
        assert bots.get(uid, bid)["state"]["paper"]["position"]["qty"] > 0

        executor.run(bots.get(uid, bid), "2026-11-17")  # sell #2 (loss #2) -> trips circuit breaker!
        b_tripped = bots.get(uid, bid)
        assert b_tripped["enabled"] is False
        assert "2회 연속 손절" in b_tripped["state"]["paper"]["tripped"]["reason"]
        assert len(alerts) == 1  # urgent protection alert sent!

    # Re-enabling via API acknowledges & clears tripped state
    en = client.post(f"/api/autotrade/bots/{bid}/enable")
    assert en.status_code == 200
    assert en.json["bot"]["enabled"] is True
    assert en.json["bot"]["state"]["tripped"] is None


def test_max_drawdown_circuit_breaker_trips_and_halts_bot(app, monkeypatch):
    monkeypatch.setitem(STRATEGIES, "scripted_cb", ScriptedCB)
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))
    overrides = {
        "2026-11-10": (100, 101, 99, 100),
        "2026-11-11": (89, 90, 87, 88),  # -12% unrealized drop -> exceeds 10% MDD guard!
    }
    monkeypatch.setattr(executor, "get_provider", lambda m: type("P", (), {"get_history": lambda self, *a: _candles(overrides)})())
    ScriptedCB.script = {"2026-11-09": [Order("buy")]}

    with app.test_request_context():
        bot = bots.create("u_mdd", {
            "market": "KR", "symbol": "005930", "strategy": "scripted_cb", "capital": 10_000,
            "guard": {"max_drawdown_pct": 10.0, "max_consecutive_losses": 0, "cooldown_days": 0, "daily_loss_limit_pct": 0},
        })
        bot["enabled"] = True
        bots.save(bot)
        executor.run(bots.get("u_mdd", bot["id"]), "2026-11-10")
        assert bots.get("u_mdd", bot["id"])["enabled"] is True
        executor.run(bots.get("u_mdd", bot["id"]), "2026-11-11")
        b = bots.get("u_mdd", bot["id"])
        assert b["enabled"] is False
        assert "최대 허용 낙폭" in b["state"]["paper"]["tripped"]["reason"]
