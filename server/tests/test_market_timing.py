"""Tests for MT-5: Market Timing ('시장 온도 신호등') filter across Quant Backtester & AutoTrade bots."""
from datetime import date, datetime, timedelta, timezone

from app.autotrade import bots, executor
from app.market.models import Candle
from app.quant import market_timing
from app.quant.engine import Backtester, Bars, Costs, Order
from app.quant.strategies import STRATEGIES, Strategy
from fakes import login


class AlwaysBuy(Strategy):
    id, name, summary, description, params = "always_buy_mt", "항상매수", "s", "s", []

    def warmup(self):
        return 1

    def prepare(self, bars):
        self.bars = bars

    def on_bar(self, i, ctx):
        if not ctx.position.is_open:
            return [Order("buy", reason="매수시도")]
        return []


def _series(vals: list[float]) -> list[Candle]:
    d = date(2026, 8, 3)
    out = []
    idx = 0
    while idx < len(vals):
        if d.weekday() < 5:
            v = vals[idx]
            ts = int(datetime(d.year, d.month, d.day, tzinfo=timezone.utc).timestamp())
            out.append(Candle(ts, v, v + 1, v - 1, v, 1000))
            idx += 1
        d += timedelta(days=1)
    return out


def test_compute_regime_and_backtester_market_timing(monkeypatch):
    monkeypatch.setitem(STRATEGIES, "always_buy_mt", AlwaysBuy)
    # 30 bars trending down -> below both 5-day and 15-day MAs -> 'bear' regime!
    bench = Bars.from_candles(_series([200 - i * 2 for i in range(30)]))
    stock = Bars.from_candles(_series([100.0] * 30))
    regimes = market_timing.align_regimes_to_bars(stock, bench, ma_short=5, ma_long=15)
    assert regimes[-1] == "bear"

    # Without market timing: buys immediately
    r_off = Backtester(stock, AlwaysBuy({}), 10_000, Costs(0, 0, 0), start_index=20).run()
    assert len(r_off["trades"]) == 1

    # With block_buy in bear regime: blocks all buys -> 0 trades!
    r_block = Backtester(
        stock, AlwaysBuy({}), 10_000, Costs(0, 0, 0), start_index=20,
        regimes=regimes, market_timing_mode="block_buy",
    ).run()
    assert len(r_block["trades"]) == 0

    # With half_size in bear regime: buys half quantity (50 shares instead of 100)!
    r_half = Backtester(
        stock, AlwaysBuy({}), 10_000, Costs(0, 0, 0), start_index=20,
        regimes=regimes, market_timing_mode="half_size",
    ).run()
    assert len(r_half["trades"]) == 1
    assert r_half["trades"][0]["qty"] == 50


def test_bot_market_timing_blocks_and_exits_in_bear_market_and_regimes_endpoint(client, monkeypatch):
    login(client, monkeypatch, sub="mt_u")
    monkeypatch.setitem(STRATEGIES, "always_buy_mt", AlwaysBuy)
    monkeypatch.setattr(executor, "_costs", lambda m: Costs(0, 0, 0))

    bear_candles = _series([250 - i * 2 for i in range(75)])
    stock_candles = _series([100.0] * 75)

    class Prov:
        def get_history(self, m, sym, iv, st):
            return bear_candles if sym == "069500" else stock_candles

    monkeypatch.setattr(executor, "get_provider", lambda m: Prov())
    monkeypatch.setattr(market_timing, "get_provider", lambda m: Prov())

    reg_res = client.get("/api/market/regimes")
    assert reg_res.status_code == 200
    assert reg_res.json["KR"]["regime"] == "bear"

    today = executor.day_of(stock_candles[-1].time)
    with client.application.app_context():
        # 1) block_buy suppresses entry
        b1 = bots.create("u_mt", {
            "market": "KR", "symbol": "005930", "strategy": "always_buy_mt", "capital": 10_000,
            "market_timing": {"mode": "block_buy", "benchmark": "069500", "ma_short": 5, "ma_long": 15},
        })
        executor.run(b1, today)
        assert bots.get("u_mt", b1["id"])["state"]["paper"]["position"]["qty"] == 0

        # 2) exit_all liquidates existing open position in bear regime!
        b2 = bots.create("u_mt", {
            "market": "KR", "symbol": "005930", "strategy": "always_buy_mt", "capital": 10_000,
            "market_timing": {"mode": "exit_all", "benchmark": "069500", "ma_short": 5, "ma_long": 15},
        })
        b2["state"]["paper"]["position"] = {"qty": 40, "entry_price": 95.0, "entry_date": "2026-08-10"}
        b2["state"]["paper"]["trades"].append({
            "entry_date": "2026-08-10", "entry_price": 95.0, "qty": 40, "entry_reason": "init",
            "cost_basis": 3800.0, "exit_date": None, "exit_price": None, "exit_reason": "", "pnl": None, "return_pct": None,
        })
        bots.save(b2)
        executor.run(bots.get("u_mt", b2["id"]), today)
        assert bots.get("u_mt", b2["id"])["state"]["paper"]["position"]["qty"] == 0
        assert "마켓 타이밍" in bots.get("u_mt", b2["id"])["state"]["paper"]["trades"][-1]["exit_reason"]
