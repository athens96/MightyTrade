from __future__ import annotations

import hashlib
import random
import time

from ..models import Candle, Instrument, Interval, Market, Quote
from .base import MarketDataProvider

_SAMPLE = [
    Instrument(Market.KR, "005930", "삼성전자", "KOSPI"),
    Instrument(Market.KR, "000660", "SK하이닉스", "KOSPI"),
    Instrument(Market.KR, "035420", "NAVER", "KOSPI"),
    Instrument(Market.US, "AAPL", "Apple Inc.", "NASDAQ"),
    Instrument(Market.US, "NVDA", "NVIDIA Corp.", "NASDAQ"),
    Instrument(Market.US, "TSLA", "Tesla Inc.", "NASDAQ"),
]


class MockProvider(MarketDataProvider):
    """Deterministic random-walk data so the UI works before real APIs are wired."""

    name = "mock"

    def _rng(self, market: Market, symbol: str, interval: Interval) -> random.Random:
        seed = hashlib.sha256(f"{market.value}:{symbol}:{interval.value}".encode()).digest()
        return random.Random(int.from_bytes(seed[:8], "big"))

    def get_candles(self, market: Market, symbol: str, interval: Interval, limit: int = 200) -> list[Candle]:
        rng = self._rng(market, symbol, interval)
        step = interval.seconds
        end = int(time.time()) // step * step
        price = 70000.0 if market == Market.KR else 150.0
        candles: list[Candle] = []
        for i in range(limit):
            t = end - (limit - 1 - i) * step
            o = price
            c = max(o * (1 + rng.gauss(0, 0.015)), 0.01)
            h = max(o, c) * (1 + abs(rng.gauss(0, 0.006)))
            l = min(o, c) * (1 - abs(rng.gauss(0, 0.006)))
            v = rng.randint(10_000, 1_000_000)
            if market == Market.KR:
                o, h, l, c = (round(x) for x in (o, h, l, c))
            else:
                o, h, l, c = (round(x, 2) for x in (o, h, l, c))
            candles.append(Candle(t, o, h, l, c, v))
            price = c
        return candles

    def get_quote(self, market: Market, symbol: str) -> Quote:
        last, prev = self.get_candles(market, symbol, Interval.D1, limit=2)[-2:][::-1]
        change = last.close - prev.close
        return Quote(
            market=market, symbol=symbol, price=last.close, change=round(change, 2),
            change_pct=round(change / prev.close * 100, 2),
            currency="KRW" if market == Market.KR else "USD", time=last.time,
        )

    def search(self, market: Market, query: str) -> list[Instrument]:
        q = query.strip().lower()
        return [i for i in _SAMPLE if i.market == market and (q in i.symbol.lower() or q in i.name.lower())]
