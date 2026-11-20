from __future__ import annotations

import hashlib
import random
import time

from ..models import Candle, Instrument, Interval, Market, Quote
from .base import MarketDataProvider

_SAMPLE = [
    Instrument(Market.KR, "005930", "삼성전자", "KOSPI", "Samsung Electronics"),
    Instrument(Market.KR, "000660", "SK하이닉스", "KOSPI", "SK hynix"),
    Instrument(Market.KR, "035420", "NAVER", "KOSPI", "NAVER"),
    Instrument(Market.KR, "069500", "KODEX 200", "KOSPI", "KODEX 200 ETF"),
    Instrument(Market.KR, "229200", "KODEX 코스닥150", "KOSPI", "KODEX KOSDAQ150 ETF"),
    Instrument(Market.KR, "360750", "TIGER 미국S&P500", "KOSPI", "TIGER US S&P500 ETF"),
    Instrument(Market.KR, "148070", "KOSEF 국고채10년", "KOSPI", "KOSEF KTB 10Y ETF"),
    Instrument(Market.KR, "114260", "KODEX 국고채3년", "KOSPI", "KODEX KTB 3Y ETF"),
    Instrument(Market.KR, "153130", "KODEX 단기채권", "KOSPI", "KODEX Short-Term Bond ETF"),
    Instrument(Market.KR, "132030", "KODEX 골드선물(H)", "KOSPI", "KODEX Gold Futures ETF"),
    Instrument(Market.KR, "261220", "KODEX WTI원유선물(H)", "KOSPI", "KODEX WTI Crude Oil ETF"),
    Instrument(Market.US, "AAPL", "애플", "NASDAQ", "Apple Inc."),
    Instrument(Market.US, "NVDA", "엔비디아", "NASDAQ", "NVIDIA Corp."),
    Instrument(Market.US, "TSLA", "테슬라", "NASDAQ", "Tesla Inc."),
    Instrument(Market.US, "SPY", "SPDR S&P 500 ETF", "NYSE", "SPDR S&P 500 ETF Trust"),
    Instrument(Market.US, "QQQ", "Invesco QQQ", "NASDAQ", "Invesco QQQ Trust"),
    Instrument(Market.US, "TLT", "iShares 미국 장기국채 20년+", "NASDAQ", "iShares 20+ Year Treasury Bond ETF"),
    Instrument(Market.US, "IEF", "iShares 미국 중기국채 7-10년", "NASDAQ", "iShares 7-10 Year Treasury Bond ETF"),
    Instrument(Market.US, "SHY", "iShares 미국 단기국채 1-3년", "NASDAQ", "iShares 1-3 Year Treasury Bond ETF"),
    Instrument(Market.US, "AGG", "iShares 미국 종합채권", "NYSE", "iShares Core U.S. Aggregate Bond ETF"),
    Instrument(Market.US, "LQD", "iShares 투자등급 회사채", "NYSE", "iShares iBoxx Investment Grade Corporate Bond ETF"),
    Instrument(Market.US, "GLD", "SPDR 골드 트러스트", "NYSE", "SPDR Gold Shares"),
    Instrument(Market.US, "DBC", "Invesco 원자재 지수", "NYSE", "Invesco DB Commodity Index Tracking Fund"),
    Instrument(Market.US, "VEA", "Vanguard 선진국 주식", "NYSE", "Vanguard FTSE Developed Markets ETF"),
    Instrument(Market.US, "VWO", "Vanguard 신흥국 주식", "NYSE", "Vanguard FTSE Emerging Markets ETF"),
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
        return [
            i for i in _SAMPLE
            if i.market == market and (q in i.symbol.lower() or q in i.name.lower() or q in i.english_name.lower())
        ]
