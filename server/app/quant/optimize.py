"""Parameter grid search over 1–2 strategy parameters.

Overfitting guard built in:
- cells with fewer than `min_trades` closed trades are not eligible as "best" (too few samples)
- every cell also gets a neighbourhood average of the objective (3x3, or 3 wide for 1-D);
  a best cell surrounded by poor cells is a lucky spike, a high plateau is more trustworthy.
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Optional

from ..errors import ApiError
from .engine import Backtester, Bars, Costs
from .strategies import Strategy, StrategyParamError

OBJECTIVES = {
    "sharpe": "샤프 지수",
    "cagr_pct": "연평균 수익률",
    "calmar": "칼마 지수",
    "sortino": "소르티노 지수",
    "total_return_pct": "총 수익률",
    "profit_factor": "손익비",
}
CELL_METRICS = ("total_return_pct", "cagr_pct", "mdd_pct", "sharpe", "sortino", "calmar", "trades",
                "win_rate_pct", "profit_factor")
MAX_AXIS = 25


@dataclass
class Axis:
    key: str
    label: str
    values: list


def parse_axis(cls: type, raw: Any) -> Axis:
    if not isinstance(raw, dict):
        raise ApiError(400, "invalid_range", "range must be an object")
    prm = next((p for p in cls.params if p.key == raw.get("key")), None)
    if prm is None:
        raise ApiError(400, "invalid_range", f"unknown parameter: {raw.get('key')}")
    try:
        start, stop, step = float(raw["start"]), float(raw["stop"]), float(raw["step"])
    except (KeyError, TypeError, ValueError):
        raise ApiError(400, "invalid_range", f"{prm.label}: 시작/끝/간격을 숫자로 입력하세요")
    if step <= 0 or stop < start:
        raise ApiError(400, "invalid_range", f"{prm.label}: 간격은 0보다 크고 끝 ≥ 시작이어야 합니다")
    if start < prm.min or stop > prm.max:
        raise ApiError(400, "invalid_range", f"{prm.label}: {prm.min:g} ~ {prm.max:g} 범위여야 합니다")
    values: list = []
    i = 0
    while True:
        v = start + i * step
        if v > stop + step * 1e-9:
            break
        v = int(round(v)) if prm.integer else round(v, 6)
        if v not in values:
            values.append(v)
        i += 1
        if len(values) > MAX_AXIS:
            raise ApiError(400, "range_too_large", f"{prm.label}: 값은 최대 {MAX_AXIS}개까지 (간격을 늘리세요)")
    return Axis(prm.key, prm.label, values)


def combos(cls: type, base: dict, x: Axis, y: Optional[Axis]) -> list:
    """[(xi, yi, params dict | None, error | None)] — None params when the combination is invalid."""
    out = []
    for yi, yv in enumerate(y.values if y else [None]):
        for xi, xv in enumerate(x.values):
            params = {**base, x.key: xv}
            if y:
                params[y.key] = yv
            try:
                strategy = cls(params)
                out.append((xi, yi, strategy, None))
            except StrategyParamError as e:
                out.append((xi, yi, None, str(e)))
    return out


def max_warmup(items: list) -> int:
    return max((s.warmup() for _, _, s, _ in items if s is not None), default=0)


def evaluate(bars: Bars, strategy: Strategy, capital: float, costs: Costs, start: int) -> dict:
    r = Backtester(bars, strategy, capital, costs, start).run()
    m = r["metrics"]
    return {k: m.get(k) for k in CELL_METRICS}


def run_grid(bars: Bars, items: list, x: Axis, y: Optional[Axis], capital: float, costs: Costs, start: int,
             objective: str, min_trades: int) -> dict[str, Any]:
    rows = len(y.values) if y else 1
    cells: list = [[None] * len(x.values) for _ in range(rows)]
    for xi, yi, strategy, err in items:
        if strategy is None:
            cells[yi][xi] = {"invalid": err}
            continue
        m = evaluate(bars, strategy, capital, costs, start)
        score = m.get(objective)
        cells[yi][xi] = {
            "params": strategy.p,
            "metrics": m,
            "score": score,
            "eligible": score is not None and (m["trades"] or 0) >= min_trades,
        }

    # neighbourhood average of eligible scores (robustness)
    for yi in range(rows):
        for xi in range(len(x.values)):
            c = cells[yi][xi]
            if not c or "invalid" in c:
                continue
            near = [
                cells[ny][nx]["score"]
                for ny in range(max(0, yi - 1), min(rows, yi + 2))
                for nx in range(max(0, xi - 1), min(len(x.values), xi + 2))
                if cells[ny][nx] and cells[ny][nx].get("eligible")
            ]
            c["neighbors"] = len(near)
            c["neighbor_avg"] = sum(near) / len(near) if near else None

    eligible = [(yi, xi, c) for yi, row in enumerate(cells) for xi, c in enumerate(row) if c and c.get("eligible")]
    best = max(eligible, key=lambda t: t[2]["score"], default=None)
    need = 3 if y else 2  # self + at least 2 (2-D) / 1 (1-D) eligible neighbours
    stable_pool = [t for t in eligible if t[2]["neighbors"] >= need] or eligible
    robust = max(stable_pool, key=lambda t: t[2]["neighbor_avg"], default=None)
    pick = lambda t: None if t is None else {"x": t[1], "y": t[0], "params": t[2]["params"], "metrics": t[2]["metrics"],  # noqa: E731
                                             "score": t[2]["score"], "neighbor_avg": t[2]["neighbor_avg"]}
    return {"cells": cells, "best": pick(best), "robust": pick(robust)}
