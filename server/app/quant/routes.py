from __future__ import annotations

import re
import time
from dataclasses import dataclass
from typing import Any, Optional

from flask import Blueprint, jsonify, request

from ..auth.decorators import login_required
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from . import montecarlo
from .data import cost_defaults
from .engine import Backtester, Bars, Costs, buy_and_hold
from .metrics import performance
from .optimize import OBJECTIVES, Axis, combos, max_warmup, parse_axis, run_grid
from .strategies import STRATEGIES, Strategy, StrategyParamError
from .walkforward import walk_forward

bp = Blueprint("quant", __name__)

PERIOD_DAYS = {"1y": 365, "3y": 365 * 3, "5y": 365 * 5, "10y": 3652, "max": None}
SYMBOL_RE = re.compile(r"[0-9A-Z][0-9A-Z.\-]{0,19}")
MAX_GRID = 400  # optimisation backtests per request
MAX_WF_RUNS = 3000  # walk-forward: windows x combinations
WARNINGS = {
    "insufficient_capital": "초기 자본이 1주 가격보다 작거나 위험 비중이 작아 매수하지 못한 신호가 있습니다.",
    "start_adjusted": "상장 기간이 짧거나 지표 계산에 필요한 데이터가 부족해 시작일이 뒤로 조정되었습니다.",
}


def _num(body: dict, key: str, default: float, lo: float, hi: float) -> float:
    try:
        v = float(body.get(key, default))
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_number", f"{key} must be a number")
    if not lo <= v <= hi:
        raise ApiError(400, "out_of_range", f"{key} must be between {lo} and {hi}")
    return v


@dataclass
class Setup:
    market: Market
    symbol: str
    cls: type
    params: dict  # raw params from the request (validated per run)
    period: str
    capital: float
    costs: Costs


def _setup(body: dict) -> Setup:
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    symbol = str(body.get("symbol", "")).strip().upper()
    if not SYMBOL_RE.fullmatch(symbol):
        raise ApiError(400, "invalid_symbol")
    cls = STRATEGIES.get(body.get("strategy"))
    if cls is None:
        raise ApiError(400, "unknown_strategy")
    period = body.get("period", "5y")
    if period not in PERIOD_DAYS:
        raise ApiError(400, "invalid_period")
    d = cost_defaults(market) if "commission_pct" not in body else {}
    return Setup(
        market, symbol, cls, body.get("params") or {}, period,
        capital=_num(body, "capital", d.get("capital", 0), 1, 1e13),
        costs=Costs(
            commission_pct=_num(body, "commission_pct", d.get("commission_pct", 0), 0, 5),
            tax_pct=_num(body, "tax_pct", d.get("tax_pct", 0), 0, 5),
            slippage_pct=_num(body, "slippage_pct", d.get("slippage_pct", 0), 0, 5),
        ),
    )


def _strategy(cls: type, params: dict) -> Strategy:
    try:
        return cls(params)
    except StrategyParamError as e:
        raise ApiError(400, "invalid_params", str(e))


def _load(s: Setup, warmup: int) -> tuple:
    """Fetch daily bars covering the period + warm-up. Returns (candles, bars, start_index, warnings)."""
    days = PERIOD_DAYS[s.period]
    start_ts = 0 if days is None else int(time.time() - days * 86400)
    # extra calendar days so indicators are warmed up at the start date (~1.5 calendar days per trading day)
    fetch_from = 0 if days is None else int(start_ts - (warmup * 1.6 + 15) * 86400)
    candles = get_provider(s.market).get_history(s.market, s.symbol, Interval.D1, fetch_from)
    bars = Bars.from_candles(candles)
    first = next((i for i, t in enumerate(bars.time) if t >= start_ts), len(bars))
    start = max(first, warmup)
    if start >= len(bars) - 1:
        raise ApiError(400, "not_enough_data", "백테스트에 필요한 데이터가 부족합니다")
    warn = ["start_adjusted"] if start > first and days is not None else []
    return candles, bars, start, warn


def _context(s: Setup, bars: Bars, start: int) -> dict:
    return {
        "market": s.market.value,
        "symbol": s.symbol,
        "strategy": s.cls.id,
        "costs": s.costs.__dict__,
        "capital": s.capital,
        "currency": "KRW" if s.market == Market.KR else "USD",
        "period": {"start": bars.time[start], "end": bars.time[-1]},
    }


def _grid_inputs(s: Setup, body: dict) -> tuple:
    x = parse_axis(s.cls, body.get("x"))
    y: Optional[Axis] = parse_axis(s.cls, body["y"]) if body.get("y") else None
    if y and y.key == x.key:
        raise ApiError(400, "invalid_range", "X와 Y는 서로 다른 파라미터여야 합니다")
    objective = body.get("objective", "sharpe")
    if objective not in OBJECTIVES:
        raise ApiError(400, "invalid_objective")
    min_trades = int(_num(body, "min_trades", 5, 0, 1000))
    try:
        base = s.cls.resolve(s.params)  # fixed values for the parameters not on an axis
    except StrategyParamError as e:
        raise ApiError(400, "invalid_params", str(e))
    items = combos(s.cls, base, x, y)
    if not any(st for _, _, st, _ in items):
        raise ApiError(400, "invalid_params", next(e for _, _, _, e in items if e))
    return x, y, objective, min_trades, items


def _axis_json(a: Optional[Axis]) -> Optional[dict]:
    return None if a is None else {"key": a.key, "label": a.label, "values": a.values}


# ------------------------------------------------------------------ endpoints
@bp.get("/strategies")
@login_required
def strategies():
    return jsonify(items=[cls.meta() for cls in STRATEGIES.values()], objectives=OBJECTIVES)


@bp.get("/defaults")
@login_required
def defaults():
    try:
        market = Market(request.args.get("market", "").upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    return jsonify(cost_defaults(market))


@bp.post("/backtest")
@login_required
def backtest():
    s = _setup(request.get_json(silent=True) or {})
    strategy = _strategy(s.cls, s.params)
    candles, bars, start, warn = _load(s, strategy.warmup())
    result = Backtester(bars, strategy, s.capital, s.costs, start).run()
    codes = result.pop("warnings") + warn
    return jsonify(
        **_context(s, bars, start),
        params=strategy.p,
        start_index=start,
        candles=[c.to_dict() for c in candles],
        chart_indicators=strategy.chart_indicators(),
        warnings=[WARNINGS.get(c, c) for c in codes],
        **result,
    )


@bp.post("/optimize")
@login_required
def optimize():
    """Grid search over 1-2 parameters: body = backtest fields + x/y {key,start,stop,step} + objective."""
    body = request.get_json(silent=True) or {}
    s = _setup(body)
    x, y, objective, min_trades, items = _grid_inputs(s, body)
    if len(items) > MAX_GRID:
        raise ApiError(400, "range_too_large", f"조합이 {len(items)}개입니다. 최대 {MAX_GRID}개까지 가능합니다")
    t0 = time.time()
    # every combination is evaluated on the same period: start after the longest warm-up in the grid
    _, bars, start, warn = _load(s, max_warmup(items))
    grid = run_grid(bars, items, x, y, s.capital, s.costs, start, objective, min_trades)
    bench = performance(bars.time[start:], buy_and_hold(bars, start, len(bars), s.capital, s.costs))
    return jsonify(
        **_context(s, bars, start),
        x=_axis_json(x), y=_axis_json(y), objective=objective, min_trades=min_trades,
        benchmark=bench,
        elapsed_sec=round(time.time() - t0, 2),
        warnings=[WARNINGS.get(c, c) for c in warn],
        **grid,
    )


@bp.post("/walkforward")
@login_required
def walkforward():
    """body = backtest fields + x/y ranges + objective + is_years + oos_months + mode (rolling|anchored)."""
    body = request.get_json(silent=True) or {}
    s = _setup(body)
    x, y, objective, min_trades, items = _grid_inputs(s, body)
    is_years = _num(body, "is_years", 3, 0.5, 20)
    oos_months = _num(body, "oos_months", 12, 1, 60)
    mode = body.get("mode", "rolling")
    if mode not in ("rolling", "anchored"):
        raise ApiError(400, "invalid_mode")
    t0 = time.time()
    _, bars, start, warn = _load(s, max_warmup(items))
    est_windows = max(1, int(((bars.time[-1] - bars.time[start]) / 86400 / 365.25 - is_years) * 12 / oos_months) + 1)
    if est_windows * len(items) > MAX_WF_RUNS:
        raise ApiError(400, "range_too_large",
                       f"약 {est_windows}개 구간 × {len(items)}개 조합 = 계산량이 너무 큽니다. 조합을 줄이거나 검증 기간을 늘리세요")
    wf = walk_forward(bars, items, x, y, s.capital, s.costs, start, objective, min_trades, is_years, oos_months, mode)
    return jsonify(
        **_context(s, bars, start),
        x=_axis_json(x), y=_axis_json(y), objective=objective, min_trades=min_trades,
        is_years=is_years, oos_months=oos_months, mode=mode,
        elapsed_sec=round(time.time() - t0, 2),
        warnings=[WARNINGS.get(c, c) for c in warn],
        **wf,
    )


@bp.post("/montecarlo")
@login_required
def monte_carlo():
    """body = backtest fields + runs + method (bootstrap|shuffle)."""
    body = request.get_json(silent=True) or {}
    s = _setup(body)
    runs = int(_num(body, "runs", 1000, 100, 5000))
    method = body.get("method", "bootstrap")
    if method not in ("bootstrap", "shuffle"):
        raise ApiError(400, "invalid_method")
    strategy = _strategy(s.cls, s.params)
    _, bars, start, warn = _load(s, strategy.warmup())
    r = Backtester(bars, strategy, s.capital, s.costs, start).run()
    impacts = montecarlo.trade_impacts(r["trades"], r["equity"])
    years = (bars.time[-1] - bars.time[start]) / 86400 / 365.25
    result: dict[str, Any] = montecarlo.simulate(impacts, runs=runs, method=method, years=years)
    return jsonify(
        **_context(s, bars, start),
        params=strategy.p,
        backtest_metrics=r["metrics"],
        warnings=[WARNINGS.get(c, c) for c in warn],
        **result,
    )
