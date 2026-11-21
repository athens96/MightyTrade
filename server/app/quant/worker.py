"""Process pool for CPU-heavy quant jobs (grid optimisation, walk-forward, Monte Carlo).

Production runs a single multi-threaded gunicorn worker (to share one Toss WebSocket per user), so
running hundreds of pure-Python backtests in request threads would contend for the GIL and stall
SSE streams and the autotrade scheduler. Offloading to a `spawn` ProcessPoolExecutor isolates the
GIL without risking fork-in-multithreaded-process deadlocks or inheriting locks/sockets.
"""
from __future__ import annotations

import atexit
import multiprocessing
import threading
from concurrent.futures import ProcessPoolExecutor
from concurrent.futures.process import BrokenProcessPool
from typing import Any, Optional

from flask import current_app

from . import montecarlo, portfolio
from .engine import Backtester, Bars, Costs
from .optimize import Axis, combos, run_grid
from .strategies import STRATEGIES
from .walkforward import walk_forward

_pool_lock = threading.Lock()


# ------------------------------------------------------------------ top-level worker entrypoints
def _exec_optimize(
    bars: Bars, strategy_id: str, base_params: dict, x: Axis, y: Optional[Axis],
    capital: float, costs: Costs, start: int, objective: str, min_trades: int,
) -> dict[str, Any]:
    cls = STRATEGIES[strategy_id]
    items = combos(cls, base_params, x, y)
    return run_grid(bars, items, x, y, capital, costs, start, objective, min_trades)


def _exec_walkforward(
    bars: Bars, strategy_id: str, base_params: dict, x: Axis, y: Optional[Axis],
    capital: float, costs: Costs, start: int, objective: str, min_trades: int,
    is_years: float, oos_months: float, mode: str,
) -> dict[str, Any]:
    cls = STRATEGIES[strategy_id]
    items = combos(cls, base_params, x, y)
    return walk_forward(bars, items, x, y, capital, costs, start, objective, min_trades, is_years, oos_months, mode)


def _exec_montecarlo(
    bars: Bars, strategy_id: str, params: dict, capital: float, costs: Costs,
    start: int, runs: int, method: str,
) -> dict[str, Any]:
    strategy = STRATEGIES[strategy_id](params)
    r = Backtester(bars, strategy, capital, costs, start).run()
    impacts = montecarlo.trade_impacts(r["trades"], r["equity"])
    years = (bars.time[-1] - bars.time[start]) / 86400 / 365.25
    result = montecarlo.simulate(impacts, runs=runs, method=method, years=years)
    return {"backtest_metrics": r["metrics"], **result}


def _exec_portfolio(
    bars_by_symbol: dict[str, Bars], model_id: str, assets: list[dict], params: dict,
    rebalance: str, drift_pct: float, capital: float, costs: Costs, start_ts: int, buy_only: bool = False,
) -> dict[str, Any]:
    return portfolio.run_portfolio_backtest(
        bars_by_symbol, model_id, assets, params, rebalance, drift_pct, capital, costs, start_ts, buy_only=buy_only
    )


# ------------------------------------------------------------------ pool management
def _get_pool(workers: int) -> ProcessPoolExecutor:
    ext = current_app.extensions
    with _pool_lock:
        pool = ext.get("quant_pool")
        if pool is None:
            pool = ProcessPoolExecutor(max_workers=workers, mp_context=multiprocessing.get_context("spawn"))
            ext["quant_pool"] = pool
            atexit.register(_shutdown_pool, pool)
        return pool


def _shutdown_pool(pool: ProcessPoolExecutor) -> None:
    try:
        pool.shutdown(wait=False, cancel_futures=True)
    except Exception:  # noqa: BLE001
        pass


def _reset_pool(broken: ProcessPoolExecutor) -> None:
    ext = current_app.extensions
    with _pool_lock:
        if ext.get("quant_pool") is broken:
            ext.pop("quant_pool", None)
    _shutdown_pool(broken)


def shutdown_app_pool(app: Any) -> None:
    with _pool_lock:
        pool = app.extensions.pop("quant_pool", None)
    if pool is not None:
        pool.shutdown(wait=True, cancel_futures=True)


def _submit(fn, *args: Any) -> dict[str, Any]:
    workers = int(current_app.config.get("QUANT_WORKERS", 0))
    if workers <= 0:
        return fn(*args)
    pool = _get_pool(workers)
    try:
        return pool.submit(fn, *args).result()
    except BrokenProcessPool:
        _reset_pool(pool)
        # Retry once on a fresh pool (e.g. after a worker was OOM-killed)
        return _get_pool(workers).submit(fn, *args).result()


# ------------------------------------------------------------------ public API
def run_optimize(
    bars: Bars, strategy_id: str, base_params: dict, x: Axis, y: Optional[Axis],
    capital: float, costs: Costs, start: int, objective: str, min_trades: int,
) -> dict[str, Any]:
    return _submit(_exec_optimize, bars, strategy_id, base_params, x, y, capital, costs, start, objective, min_trades)


def run_walkforward(
    bars: Bars, strategy_id: str, base_params: dict, x: Axis, y: Optional[Axis],
    capital: float, costs: Costs, start: int, objective: str, min_trades: int,
    is_years: float, oos_months: float, mode: str,
) -> dict[str, Any]:
    return _submit(
        _exec_walkforward, bars, strategy_id, base_params, x, y, capital, costs,
        start, objective, min_trades, is_years, oos_months, mode,
    )


def run_montecarlo(
    bars: Bars, strategy_id: str, params: dict, capital: float, costs: Costs,
    start: int, runs: int, method: str,
) -> dict[str, Any]:
    return _submit(_exec_montecarlo, bars, strategy_id, params, capital, costs, start, runs, method)


def run_portfolio(
    bars_by_symbol: dict[str, Bars], model_id: str, assets: list[dict], params: dict,
    rebalance: str, drift_pct: float, capital: float, costs: Costs, start_ts: int, buy_only: bool = False,
) -> dict[str, Any]:
    return _submit(
        _exec_portfolio, bars_by_symbol, model_id, assets, params,
        rebalance, drift_pct, capital, costs, start_ts, buy_only,
    )
