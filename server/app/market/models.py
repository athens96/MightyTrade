from __future__ import annotations

from dataclasses import asdict, dataclass
from enum import Enum


class Market(str, Enum):
    KR = "KR"  # KOSPI/KOSDAQ, symbol = 6-digit code (e.g. 005930)
    US = "US"  # NYSE/NASDAQ, symbol = ticker (e.g. AAPL)


class Interval(str, Enum):
    M1 = "1m"
    M5 = "5m"
    M15 = "15m"
    H1 = "1h"
    D1 = "1d"
    W1 = "1w"

    @property
    def seconds(self) -> int:
        return {"1m": 60, "5m": 300, "15m": 900, "1h": 3600, "1d": 86400, "1w": 604800}[self.value]


@dataclass
class Candle:
    time: int  # unix seconds (UTC), bar open time
    open: float
    high: float
    low: float
    close: float
    volume: float

    def to_dict(self) -> dict:
        return asdict(self)


@dataclass
class Quote:
    market: Market
    symbol: str
    price: float
    change: float
    change_pct: float
    currency: str
    time: int

    def to_dict(self) -> dict:
        d = asdict(self)
        d["market"] = self.market.value
        return d


@dataclass
class Instrument:
    market: Market
    symbol: str
    name: str
    exchange: str = ""
    english_name: str = ""

    def to_dict(self) -> dict:
        d = asdict(self)
        d["market"] = self.market.value
        return d
