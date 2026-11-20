"""Build larger candles from smaller ones (Toss only serves 1m and 1d)."""
from __future__ import annotations

import calendar
from datetime import date, timedelta
from typing import Callable, Hashable

from .models import Candle


def _merge(group: list[Candle], time: int) -> Candle:
    return Candle(
        time=time,
        open=group[0].open,
        high=max(c.high for c in group),
        low=min(c.low for c in group),
        close=group[-1].close,
        volume=sum(c.volume for c in group),
    )


def _bucket(candles: list[Candle], key: Callable[[Candle], Hashable], time_of: Callable[[Candle], int]) -> list[Candle]:
    """Group consecutive ascending candles by key; bar time = time_of(first candle in bucket)."""
    out: list[Candle] = []
    group: list[Candle] = []
    current = None
    for c in candles:
        k = key(c)
        if group and k != current:
            out.append(_merge(group, time_of(group[0])))
            group = []
        current = k
        group.append(c)
    if group:
        out.append(_merge(group, time_of(group[0])))
    return out


def resample_intraday(candles: list[Candle], step_sec: int) -> list[Candle]:
    """1m -> 5m/15m/1h, aligned to UTC multiples of step (KR 09:00 KST = 00:00 UTC)."""
    floor = lambda c: c.time // step_sec * step_sec  # noqa: E731
    return _bucket(candles, floor, floor)


def resample_weekly(daily: list[Candle]) -> list[Candle]:
    """1d -> 1w (Mon-Sun). Daily candle time must be UTC midnight of the trading date."""
    def week_start(c: Candle) -> int:
        d = date(1970, 1, 1) + timedelta(days=c.time // 86400)
        return calendar.timegm((d - timedelta(days=d.weekday())).timetuple())

    return _bucket(daily, week_start, week_start)
