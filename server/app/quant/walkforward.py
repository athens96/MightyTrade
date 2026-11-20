"""Walk-forward validation.

Repeat: optimise parameters on an in-sample (IS) window, then trade the following out-of-sample (OOS)
window with those parameters, using no data from after the OOS window. Stitching all OOS segments gives
an equity curve made only of "unseen" periods — a much more honest estimate than one optimised backtest.

- rolling:  IS window of fixed length slides forward by the OOS length
- anchored: IS always starts at the beginning and grows

Each OOS segment starts flat with the previous segment's ending equity (an open position at a segment's
end is valued at that day's close).
"""
from __future__ import annotations

import bisect
from typing import Any, Optional

from ..errors import ApiError
from .engine import Backtester, Bars, Costs, buy_and_hold, slice_bars
from .metrics import drawdown_series, performance, trade_stats
from .optimize import Axis, run_grid

YEAR = 365.25 * 86400
MONTH = YEAR / 12
MAX_WINDOWS = 20
MIN_OOS_BARS = 20


def make_windows(times: list, start: int, is_years: float, oos_months: float, mode: str) -> list:
    """[(is_start, is_end, oos_end)] as bar indices; IS = [is_start, is_end), OOS = [is_end, oos_end)."""
    idx = lambda t: bisect.bisect_left(times, t)  # noqa: E731
    t0 = times[start]
    out = []
    k = 0
    while True:
        is_start_t = t0 if mode == "anchored" else t0 + k * oos_months * MONTH
        is_end_t = t0 + is_years * YEAR + k * oos_months * MONTH
        oos_end_t = is_end_t + oos_months * MONTH
        is_start, is_end, oos_end = idx(is_start_t), idx(is_end_t), min(idx(oos_end_t), len(times))
        if is_end >= len(times) or oos_end - is_end < MIN_OOS_BARS:
            break
        out.append((max(is_start, start), is_end, oos_end))
        k += 1
        if len(out) > MAX_WINDOWS:
            raise ApiError(400, "too_many_windows", f"구간이 {MAX_WINDOWS}개를 넘습니다. 검증 기간을 늘리세요")
    return out


def walk_forward(bars: Bars, items: list, x: Axis, y: Optional[Axis], capital: float, costs: Costs, start: int,
                 objective: str, min_trades: int, is_years: float, oos_months: float, mode: str) -> dict[str, Any]:
    windows = make_windows(bars.time, start, is_years, oos_months, mode)
    if not windows:
        raise ApiError(400, "not_enough_data", "기간이 학습+검증 기간보다 짧습니다. 기간을 늘리거나 학습 기간을 줄이세요")

    equity_pts: list = []
    all_trades: list = []
    out_windows = []
    eq = capital
    is_cagrs = []
    for is_start, is_end, oos_end in windows:
        grid = run_grid(slice_bars(bars, is_end), items, x, y, capital, costs, is_start, objective, min_trades)
        best = grid["best"]
        if best is None:  # nothing eligible in-sample: sit out this window in cash
            seg_times = bars.time[is_end:oos_end]
            equity_pts += [(t, eq) for t in seg_times]
            out_windows.append(_window(bars, is_start, is_end, oos_end, None, None, None))
            continue
        strategy = next(s for _, _, s, _ in items if s is not None and s.p == best["params"])
        strategy = type(strategy)(best["params"])  # fresh instance
        r = Backtester(slice_bars(bars, oos_end), strategy, eq, costs, is_end).run()
        seg = [(p["time"], p["equity"]) for p in r["equity"]]
        equity_pts += seg
        all_trades += r["trades"]
        if best["metrics"].get("cagr_pct") is not None:
            is_cagrs.append(best["metrics"]["cagr_pct"])
        oos = {k: r["metrics"].get(k) for k in ("total_return_pct", "cagr_pct", "mdd_pct", "sharpe", "trades")}
        out_windows.append(_window(bars, is_start, is_end, oos_end, best["params"], best, oos))
        eq = seg[-1][1]

    first_oos, last_oos = windows[0][1], windows[-1][2]
    times = [t for t, _ in equity_pts]
    values = [v for _, v in equity_pts]
    bench = buy_and_hold(bars, first_oos, last_oos, capital, costs)
    metrics = {**performance(times, values), **trade_stats(all_trades)}
    mean_is = sum(is_cagrs) / len(is_cagrs) if is_cagrs else None
    oos_cagr = metrics.get("cagr_pct")
    return {
        "windows": out_windows,
        "equity": [
            {"time": t, "equity": round(v, 4), "benchmark": round(b, 4), "drawdown": round(d * 100, 4)}
            for t, v, b, d in zip(times, values, bench, drawdown_series(values))
        ],
        "metrics": metrics,
        "benchmark": performance(times, bench),
        "trades": all_trades,
        # Walk-forward efficiency: OOS CAGR / average optimised IS CAGR. ~>0.5 suggests the edge survives.
        "efficiency": oos_cagr / mean_is if oos_cagr is not None and mean_is and mean_is > 0 else None,
        "mean_is_cagr_pct": mean_is,
    }


def _window(bars: Bars, is_start: int, is_end: int, oos_end: int, params, best, oos) -> dict:
    return {
        "is_start": bars.time[is_start],
        "is_end": bars.time[is_end - 1],
        "oos_start": bars.time[is_end],
        "oos_end": bars.time[oos_end - 1],
        "params": params,
        "is_score": best["score"] if best else None,
        "is_cagr_pct": best["metrics"].get("cagr_pct") if best else None,
        "oos": oos,
    }
