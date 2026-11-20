"""Technical indicators (server side). Line-for-line port of client/src/indicators/{math,calc}.ts.

Series are index-aligned with the bars; None = not enough data yet. Parity with the TypeScript
implementation is enforced by shared/fixtures/indicators.json (see fixtures.py and tests).
"""
from __future__ import annotations

import math
from typing import Callable, Optional, Sequence

Series = list  # list[Optional[float]]


def sma(values: Sequence[Optional[float]], n: int) -> Series:
    out: Series = [None] * len(values)
    total = 0.0
    valid = 0
    for i, v in enumerate(values):
        if v is None:
            total, valid = 0.0, 0
            continue
        total += v
        valid += 1
        if valid > n:
            total -= values[i - n]  # type: ignore[operator]
            valid = n
        if valid == n:
            out[i] = total / n
    return out


def _smoothed(values: Sequence[Optional[float]], n: int, alpha: float) -> Series:
    out: Series = [None] * len(values)
    prev: Optional[float] = None
    seed_sum, seed_count = 0.0, 0
    for i, v in enumerate(values):
        if v is None:
            if prev is not None:
                prev, seed_sum, seed_count = None, 0.0, 0
            continue
        if prev is None:
            seed_sum += v
            seed_count += 1
            if seed_count == n:
                prev = seed_sum / n
                out[i] = prev
            continue
        prev = alpha * v + (1 - alpha) * prev
        out[i] = prev
    return out


def ema(values: Sequence[Optional[float]], n: int) -> Series:
    """Exponential MA (alpha = 2/(n+1)), seeded with the SMA of the first n values."""
    return _smoothed(values, n, 2 / (n + 1))


def rma(values: Sequence[Optional[float]], n: int) -> Series:
    """Wilder's smoothing (alpha = 1/n), seeded with an SMA."""
    return _smoothed(values, n, 1 / n)


def stdev(values: Sequence[float], n: int) -> Series:
    """Population standard deviation over a rolling window."""
    mean = sma(values, n)
    out: Series = [None] * len(values)
    for i, m in enumerate(mean):
        if m is None:
            continue
        out[i] = math.sqrt(sum((values[j] - m) ** 2 for j in range(i - n + 1, i + 1)) / n)
    return out


def highest(values: Sequence[float], n: int) -> Series:
    return [None if i < n - 1 else max(values[i - n + 1 : i + 1]) for i in range(len(values))]


def lowest(values: Sequence[float], n: int) -> Series:
    return [None if i < n - 1 else min(values[i - n + 1 : i + 1]) for i in range(len(values))]


def zip_with(a: Series, b: Series, f: Callable[[float, float], Optional[float]]) -> Series:
    return [None if x is None or y is None else f(x, y) for x, y in zip(a, b)]


def true_range(high: Sequence[float], low: Sequence[float], close: Sequence[float]) -> list:
    return [
        h - low[i] if i == 0 else max(h - low[i], abs(h - close[i - 1]), abs(low[i] - close[i - 1]))
        for i, h in enumerate(high)
    ]


def rsi(close: Sequence[float], n: int) -> Series:
    """RSI with Wilder smoothing."""
    gains: Series = [None] + [max(close[i] - close[i - 1], 0.0) for i in range(1, len(close))]
    losses: Series = [None] + [max(close[i - 1] - close[i], 0.0) for i in range(1, len(close))]

    def f(g: float, l: float) -> float:
        if l == 0:
            return 50.0 if g == 0 else 100.0
        return 100 - 100 / (1 + g / l)

    return zip_with(rma(gains, n), rma(losses, n), f)


def atr(high: Sequence[float], low: Sequence[float], close: Sequence[float], n: int) -> Series:
    return rma(true_range(high, low, close), n)


def bollinger(close: Sequence[float], n: int, k: float) -> dict:
    mid = sma(close, n)
    sd = stdev(close, n)
    return {
        "upper": zip_with(mid, sd, lambda m, s: m + k * s),
        "mid": mid,
        "lower": zip_with(mid, sd, lambda m, s: m - k * s),
    }


def donchian(high: Sequence[float], low: Sequence[float], upper_n: int, lower_n: int) -> dict:
    return {"upper": highest(high, upper_n), "lower": lowest(low, lower_n)}
