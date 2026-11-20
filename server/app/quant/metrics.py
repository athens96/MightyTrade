"""Performance statistics. Returns plain floats (None when undefined) so results are JSON-safe."""
from __future__ import annotations

import math
from typing import Any, Optional, Sequence

PERIODS_PER_YEAR = 252  # daily bars


def _clean(v: Optional[float]) -> Optional[float]:
    return None if v is None or math.isnan(v) or math.isinf(v) else v


def drawdown_series(equity: Sequence[float]) -> list:
    peak = -math.inf
    out = []
    for e in equity:
        peak = max(peak, e)
        out.append(e / peak - 1 if peak > 0 else 0.0)
    return out


def _periods_per_year(times: Sequence[int]) -> float:
    if len(times) < 2:
        return float(PERIODS_PER_YEAR)
    diffs = sorted(times[i] - times[i - 1] for i in range(1, len(times)) if times[i] > times[i - 1])
    if not diffs:
        return float(PERIODS_PER_YEAR)
    step = diffs[len(diffs) // 2]
    if step >= 43200:  # daily or coarser
        return float(PERIODS_PER_YEAR)
    # Intraday bars: ~6.5 regular trading hours per day x 252 trading days
    return PERIODS_PER_YEAR * (6.5 * 3600 / step)


def performance(times: Sequence[int], equity: Sequence[float]) -> dict[str, Any]:
    start, end = equity[0], equity[-1]
    years = (times[-1] - times[0]) / 86400 / 365.25
    ppy = _periods_per_year(times)
    rets = [equity[i] / equity[i - 1] - 1 for i in range(1, len(equity)) if equity[i - 1] > 0]
    mean = sum(rets) / len(rets) if rets else 0.0
    sd = math.sqrt(sum((r - mean) ** 2 for r in rets) / (len(rets) - 1)) if len(rets) > 1 else 0.0
    downside = math.sqrt(sum(min(r, 0.0) ** 2 for r in rets) / len(rets)) if rets else 0.0
    dd = drawdown_series(equity)
    mdd = min(dd) if dd else 0.0

    # longest drawdown: from a peak until equity recovers to it (or the end if it never does)
    longest = 0
    peak, peak_t, underwater = -math.inf, times[0], False
    for t, e in zip(times, equity):
        if e >= peak:
            if underwater:
                longest = max(longest, t - peak_t)
                underwater = False
            peak, peak_t = e, t
        else:
            underwater = True
            longest = max(longest, t - peak_t)

    cagr = (end / start) ** (1 / years) - 1 if years > 0.05 and start > 0 and end > 0 else None
    return {
        "start_equity": start,
        "final_equity": end,
        "total_return_pct": _clean((end / start - 1) * 100) if start else None,
        "cagr_pct": _clean(cagr * 100) if cagr is not None else None,
        "mdd_pct": mdd * 100,
        "max_dd_days": round(longest / 86400),
        "volatility_pct": _clean(sd * math.sqrt(ppy) * 100),
        "sharpe": _clean(mean / sd * math.sqrt(ppy)) if sd > 0 else None,
        "sortino": _clean(mean / downside * math.sqrt(ppy)) if downside > 0 else None,
        "calmar": _clean(cagr / abs(mdd)) if cagr is not None and mdd < 0 else None,
    }


def trade_stats(trades: Sequence[dict]) -> dict[str, Any]:
    closed = [t for t in trades if not t["open"]]
    wins = [t for t in closed if t["pnl"] > 0]
    losses = [t for t in closed if t["pnl"] <= 0]
    gross_win = sum(t["pnl"] for t in wins)
    gross_loss = -sum(t["pnl"] for t in losses)
    avg = lambda xs: sum(xs) / len(xs) if xs else None  # noqa: E731
    return {
        "trades": len(closed),
        "win_rate_pct": len(wins) / len(closed) * 100 if closed else None,
        "avg_return_pct": avg([t["return_pct"] for t in closed]),
        "avg_win_pct": avg([t["return_pct"] for t in wins]),
        "avg_loss_pct": avg([t["return_pct"] for t in losses]),
        "best_pct": max((t["return_pct"] for t in closed), default=None),
        "worst_pct": min((t["return_pct"] for t in closed), default=None),
        "profit_factor": _clean(gross_win / gross_loss) if gross_loss > 0 else None,
        "avg_bars": avg([t["bars"] for t in closed]),
    }
