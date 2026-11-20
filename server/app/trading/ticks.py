"""Price grid rules. Toss rejects limit/trigger prices that are not on the exchange tick grid."""
from __future__ import annotations

import math

# KRX (KOSPI/KOSDAQ stocks, unified table since 2023): (upper bound exclusive, tick)
KRX_TICKS = [(2_000, 1), (5_000, 5), (20_000, 10), (50_000, 50), (200_000, 100), (500_000, 500), (math.inf, 1_000)]


def krx_tick(price: float) -> int:
    return next(t for bound, t in KRX_TICKS if price < bound)


def round_price(market: str, price: float, mode: str = "nearest") -> float:
    """Snap to the market's price grid. mode: nearest | down | up."""
    if market == "KR":
        tick = krx_tick(price)
        q = price / tick
        n = math.floor(q + 1e-9) if mode == "down" else math.ceil(q - 1e-9) if mode == "up" else math.floor(q + 0.5)
        return float(int(n * tick))
    digits = 2 if price >= 1 else 4  # US: cents above $1, 4 decimals below
    f = 10 ** digits
    # half-up (Python's round() is half-to-even); small epsilon absorbs float noise like 12.345 -> 1234.4999
    n = math.floor(price * f + 1e-9) if mode == "down" else math.ceil(price * f - 1e-9) if mode == "up" else math.floor(price * f + 0.5 + 1e-9)
    return n / f


def fmt_price(market: str, price: float) -> str:
    """Toss expects decimals as strings."""
    return str(int(price)) if market == "KR" else f"{price:.4f}".rstrip("0").rstrip(".")
