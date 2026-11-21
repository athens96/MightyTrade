"""Tests for MT-7: Smart DCA ('스마트 적립식') bot & Cash-inflow Buy-Only ('무매도') rebalancing."""
from datetime import date, datetime, timedelta, timezone

import pytest

from app.autotrade import bots, dca, executor, rebalance
from app.market.models import Candle
from app.quant.engine import Bars, Costs


def _mk_candles(closes: list[float]) -> list[Candle]:
    d = date(2026, 9, 1)
    out = []
    idx = 0
    while idx < len(closes):
        if d.weekday() < 5:
            v = closes[idx]
            ts = int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())
            out.append(Candle(ts, v, v + 1, v - 1, v, 1000))
            idx += 1
        d += timedelta(days=1)
    return out


def test_smart_dca_scales_up_on_dip_and_tracks_cost_savings(app, monkeypatch):
    # Sharp drop at the end -> oversold RSI -> dip_mult=2.0x!
    dip_bars = _mk_candles([100.0] * 20 + [92.0, 86.0, 80.0])
    mult, note = dca.compute_dca_multiplier(Bars.from_candles(dip_bars[:-1]), smart_scale=True, dip_mult=2.0, peak_mult=0.5)
    assert mult == 2.0 and "세일 구간" in note

    # Sharp rally -> overheated RSI -> peak_mult=0.5x!
    hot_bars = _mk_candles([100.0] * 20 + [108.0, 115.0, 122.0])
    mult_hot, note_hot = dca.compute_dca_multiplier(Bars.from_candles(hot_bars[:-1]), smart_scale=True, dip_mult=2.0, peak_mult=0.5)
    assert mult_hot == 0.5 and "과열 구간" in note_hot

    # End-to-end paper DCA across an overheated day (120) then a dip day (80) -> beats fixed DCA average cost!
    seq_candles = _mk_candles([100.0] * 20 + [110.0, 120.0, 120.0, 95.0, 80.0, 80.0])
    monkeypatch.setattr(dca, "get_provider", lambda m: type("P", (), {"get_history": lambda self, *a: seq_candles})())
    monkeypatch.setattr(dca, "_costs", lambda m: Costs(0, 0, 0))

    with app.test_request_context():
        bot = bots.create("u_dca", {
            "kind": "dca",
            "market": "KR",
            "symbol": "005930",
            "rebalance": "daily",
            "base_amount": 1_200,
            "smart_scale": True,
            "dip_mult": 2.0,
            "peak_mult": 0.5,
            "capital": 100_000,
        })
        day_hot = dca._day_of(seq_candles[22].time)  # price 120 after surge -> buys 0.5x (600 / 120 = 5 shares)
        day_dip = dca._day_of(seq_candles[25].time)  # price 80 after plunge -> buys 2.0x (2400 / 80 = 30 shares)
        executor.run(bots.get("u_dca", bot["id"]), day_hot)
        executor.run(bots.get("u_dca", bot["id"]), day_dip)
        pub = bots.public(bots.get("u_dca", bot["id"]))
        stats = pub["state"]["dca_stats"]
        assert stats["steps"] == 2
        assert stats["smart_avg_price"] < stats["regular_avg_price"]
        assert stats["saved_pct"] > 5.0  # significantly lower avg cost than plain fixed DCA!


def test_rebalance_buy_only_mode_never_sells_overweight_assets(app, monkeypatch):
    # Asset A surges to 200 (overweight), Asset B stays at 100 (underweight), new cash deposited -> buys ONLY B, 0 sells of A!
    c_a = _mk_candles([100.0] * 25 + [200.0, 200.0])
    c_b = _mk_candles([100.0] * 25 + [100.0, 100.0])

    class Prov:
        def get_history(self, m, sym, iv, st):
            return c_a if sym == "069500" else c_b

    monkeypatch.setattr(rebalance, "get_provider", lambda m: Prov())
    monkeypatch.setattr(rebalance, "_costs", lambda m: Costs(0, 0, 0))

    with app.test_request_context():
        bot = bots.create("u_bo", {
            "kind": "rebalance",
            "market": "KR",
            "model": "equal_weight",
            "assets": [
                {"symbol": "069500", "role": "offensive", "weight": 50},
                {"symbol": "148070", "role": "defensive", "weight": 50},
            ],
            "rebalance": "monthly",
            "buy_only": True,
            "capital": 30_000,
        })
        st = bot["state"]["paper"]
        # Already hold 50 shares of 069500 (@200 = 10,000) and 20 shares of 148070 (@100 = 2,000), plus 4,000 fresh cash!
        st["positions"] = {
            "069500": {"qty": 50, "entry_price": 100.0},
            "148070": {"qty": 20, "entry_price": 100.0},
        }
        st["cash"] = 4_000.0
        st["last_rebalance_date"] = "2026-09-01"
        bots.save(bot)

        today = dca._day_of(c_a[-1].time)
        executor.run(bots.get("u_bo", bot["id"]), today)
        after = bots.get("u_bo", bot["id"])["state"]["paper"]["positions"]
        assert after["069500"]["qty"] == 50   # NEVER sold overweight winner!
        assert after["148070"]["qty"] == 60   # All 4,000 cash deployed into underweight 148070 (+40 shares)!
