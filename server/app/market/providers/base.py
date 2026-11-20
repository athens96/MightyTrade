from __future__ import annotations

import time
from abc import ABC, abstractmethod

from ..models import Candle, Instrument, Interval, Market, Quote


class MarketDataProvider(ABC):
    """Common interface for every market data source (Toss, Investing.com, mock...)."""

    name: str = "base"

    def __init__(self, config: dict):
        self.config = config

    @abstractmethod
    def get_candles(self, market: Market, symbol: str, interval: Interval, limit: int = 200) -> list[Candle]:
        """Return candles sorted ascending by time."""

    @abstractmethod
    def get_quote(self, market: Market, symbol: str) -> Quote: ...

    def get_history(self, market: Market, symbol: str, interval: Interval, start: int) -> list[Candle]:
        """Candles from `start` (unix seconds, 0 = as far back as available) to now, ascending."""
        limit = 6000 if not start else min(max(int((time.time() - start) / interval.seconds) + 10, 50), 6000)
        return self.get_candles(market, symbol, interval, limit)

    def search(self, market: Market, query: str) -> list[Instrument]:
        raise NotImplementedError(f"{self.name}: search not implemented")
