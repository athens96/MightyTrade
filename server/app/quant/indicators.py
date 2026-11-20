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


def macd(close: Sequence[float], fast: int = 12, slow: int = 26, signal: int = 9) -> dict:
    """MACD matching client/src/indicators/calc.ts (EMA fast - EMA slow, EMA signal)."""
    line = zip_with(ema(close, fast), ema(close, slow), lambda f, s: f - s)
    sig = ema(line, signal)
    hist = zip_with(line, sig, lambda m, s: m - s)
    return {"macd": line, "signal": sig, "hist": hist}


def session_buckets(times: Sequence[int]) -> list[int]:
    """Assign an integer session id to each bar (resets on overnight gaps or synthetic session length).

    Works identically on KR/US real intraday bars (gap > max(4*step, 1h)), continuous mock intraday
    bars (~6.5h session chunks), and daily bars (10-bar chunks for smoke testing).
    """
    n = len(times)
    if n == 0:
        return []
    if n == 1:
        return [0]
    diffs = sorted(times[i] - times[i - 1] for i in range(1, n) if times[i] > times[i - 1])
    step = diffs[len(diffs) // 2] if diffs else 86400
    if step >= 43200:
        # Daily or coarser fallback (e.g. generic strategy tests on daily bars)
        return [i // 10 for i in range(n)]
    gap_thresh = max(step * 4, 3600)
    max_bars = max(int(round(6.5 * 3600 / step)), 12)
    out = [0]
    sid = 0
    in_sess = 1
    for i in range(1, n):
        if times[i] - times[i - 1] > gap_thresh or in_sess >= max_bars:
            sid += 1
            in_sess = 1
        else:
            in_sess += 1
        out.append(sid)
    return out


def intraday_vwap(
    times: Sequence[int], high: Sequence[float], low: Sequence[float],
    close: Sequence[float], volume: Sequence[float],
) -> dict:
    """Session-cumulative VWAP and volume-weighted standard deviation per bar."""
    sessions = session_buckets(times)
    vwap_out: Series = []
    sd_out: Series = []
    cum_pv = 0.0
    cum_p2v = 0.0
    cum_v = 0.0
    prev_s: Optional[int] = None
    for s, h, l, c, v in zip(sessions, high, low, close, volume):
        if s != prev_s:
            cum_pv = cum_p2v = cum_v = 0.0
            prev_s = s
        tp = (h + l + c) / 3.0
        vol = max(float(v), 0.0)
        cum_pv += tp * vol
        cum_p2v += tp * tp * vol
        cum_v += vol
        if cum_v > 0:
            mean = cum_pv / cum_v
            var = max(cum_p2v / cum_v - mean * mean, 0.0)
            vwap_out.append(mean)
            sd_out.append(math.sqrt(var))
        else:
            vwap_out.append(tp)
            sd_out.append(0.0)
    return {"vwap": vwap_out, "sd": sd_out, "session": sessions}
