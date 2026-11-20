from __future__ import annotations

from ..models import Candle, Interval, Market, Quote
from .base import MarketDataProvider


class InvestingProvider(MarketDataProvider):
    """investing.com data source.

    TODO:
      - investing.com has no official public API. Decide the access method
        (licensed data feed / page scraping) and check its Terms of Use first.
      - Map symbol -> investing.com instrument id (pair id), cache the mapping.
      - Add response caching + rate limiting to avoid being blocked.
    """

    name = "investing"

    def get_candles(self, market: Market, symbol: str, interval: Interval, limit: int = 200) -> list[Candle]:
        raise NotImplementedError("investing.com market data is not implemented yet")

    def get_quote(self, market: Market, symbol: str) -> Quote:
        raise NotImplementedError("investing.com quote is not implemented yet")
