from __future__ import annotations

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

    def search(self, market: Market, query: str) -> list[Instrument]:
        raise NotImplementedError(f"{self.name}: search not implemented")
