"""Monte Carlo analysis of a backtest's trades.

Each closed trade is reduced to its impact on the account: pnl / equity just before entry. Then:
- bootstrap: draw N trades with replacement (asks "what if the same kind of trades came in another mix?")
- shuffle:   reorder the actual trades (same final return; shows how bad the drawdown could have been)
For every simulated sequence we compound the impacts into an equity path and record its final return and
maximum drawdown. The spread of those numbers shows how much of the backtest result was luck of ordering.
"""
from __future__ import annotations

import bisect
import random
from typing import Any, Optional, Sequence

from ..errors import ApiError

PERCENTILES = (5, 25, 50, 75, 95)
MAX_PATH_POINTS = 200
MIN_TRADES = 5


def trade_impacts(trades: Sequence[dict], equity: Sequence[dict]) -> list:
    """Closed trades → fraction of account equity gained/lost (pnl / equity at the prior close)."""
    times = [p["time"] for p in equity]
    out = []
    for t in trades:
        if t["open"] or t["pnl"] is None:
            continue
        i = bisect.bisect_left(times, t["entry_time"]) - 1
        base = equity[max(i, 0)]["equity"]
        if base > 0:
            out.append(t["pnl"] / base)
    return out


def _percentile(sorted_vals: Sequence[float], p: float) -> float:
    if not sorted_vals:
        return 0.0
    k = (len(sorted_vals) - 1) * p / 100
    lo, hi = int(k), min(int(k) + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (k - lo)


def _rank(sorted_vals: Sequence[float], v: float) -> float:
    """Percentile rank of v within sorted_vals (0-100)."""
    return bisect.bisect_left(sorted_vals, v) / len(sorted_vals) * 100 if sorted_vals else 0.0


def _path(seq: Sequence[float]) -> tuple:
    eq, peak, mdd = 1.0, 1.0, 0.0
    path = [1.0]
    for r in seq:
        eq *= 1 + r
        peak = max(peak, eq)
        mdd = min(mdd, eq / peak - 1)
        path.append(eq)
    return path, eq - 1, mdd


def _histogram(values: Sequence[float], bins: int = 30) -> dict:
    lo, hi = min(values), max(values)
    if hi == lo:
        hi = lo + 1e-9
    width = (hi - lo) / bins
    counts = [0] * bins
    for v in values:
        counts[min(int((v - lo) / width), bins - 1)] += 1
    return {"edges": [lo + i * width for i in range(bins + 1)], "counts": counts}


def simulate(impacts: Sequence[float], runs: int = 1000, method: str = "bootstrap", seed: int = 42,
             years: Optional[float] = None) -> dict[str, Any]:
    n = len(impacts)
    if n < MIN_TRADES:
        raise ApiError(400, "not_enough_trades", f"몬테카를로에는 청산된 매매가 {MIN_TRADES}회 이상 필요합니다 (현재 {n}회)")
    rng = random.Random(seed)
    # equity is kept only at the downsampled points the chart needs (memory: runs x <=201 floats)
    step = max(1, (n + 1) // MAX_PATH_POINTS)
    idxs = list(range(0, n + 1, step))
    if idxs[-1] != n:
        idxs.append(n)
    finals, mdds, paths = [], [], []
    for _ in range(runs):
        if method == "shuffle":
            seq = list(impacts)
            rng.shuffle(seq)
        else:
            seq = rng.choices(impacts, k=n)
        path, final, mdd = _path(seq)
        finals.append(final * 100)
        mdds.append(mdd * 100)
        paths.append([path[k] for k in idxs])

    orig_path, orig_final, orig_mdd = _path(impacts)
    sf, sm = sorted(finals), sorted(mdds)

    # percentile bands of the equity path per trade number
    bands = {f"p{p}": [] for p in PERCENTILES}
    for j in range(len(idxs)):
        col = sorted(path[j] for path in paths)
        for p in PERCENTILES:
            bands[f"p{p}"].append((_percentile(col, p) - 1) * 100)

    def cagr(final_pct: float) -> Optional[float]:
        if not years or years <= 0 or final_pct <= -100:
            return None
        return ((1 + final_pct / 100) ** (1 / years) - 1) * 100

    return {
        "runs": runs,
        "method": method,
        "trades": n,
        "final_return_pct": {f"p{p}": _percentile(sf, p) for p in PERCENTILES},
        "cagr_pct": {f"p{p}": cagr(_percentile(sf, p)) for p in PERCENTILES},
        "mdd_pct": {f"p{p}": _percentile(sm, p) for p in PERCENTILES},
        "prob_loss_pct": sum(1 for f in finals if f < 0) / runs * 100,
        "prob_mdd_worse_pct": sum(1 for m in mdds if m < orig_mdd * 100) / runs * 100,
        "original": {
            "final_return_pct": orig_final * 100,
            "mdd_pct": orig_mdd * 100,
            "final_rank_pct": _rank(sf, orig_final * 100),
            "mdd_rank_pct": _rank(sm, orig_mdd * 100),
            "path": [(orig_path[k] - 1) * 100 for k in idxs],
        },
        "path_index": idxs,
        "bands": bands,
        "hist_final": _histogram(finals),
        "hist_mdd": _histogram(mdds),
    }
