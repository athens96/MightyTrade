from __future__ import annotations

import re
import time
from dataclasses import dataclass
from typing import Any, Optional

from flask import Blueprint, current_app, jsonify, request

from ..auth.decorators import login_required
from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from . import market_timing, portfolio, screener, screener_data, worker
from .data import cost_defaults
from .engine import Backtester, Bars, Costs, RiskRules, buy_and_hold
from .metrics import performance
from .optimize import OBJECTIVES, Axis, combos, max_warmup, parse_axis
from .strategies import STRATEGIES, Strategy, StrategyParamError

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
    risk: RiskRules
    market_timing: dict


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
    try:
        risk = RiskRules.from_dict(body.get("risk"))
    except (TypeError, ValueError) as e:
        raise ApiError(400, "invalid_risk", str(e))
    mt_cfg = market_timing.validate_market_timing(body.get("market_timing"), market.value)
    d = cost_defaults(market) if "commission_pct" not in body else {}
    return Setup(
        market, symbol, cls, body.get("params") or {}, period,
        capital=_num(body, "capital", d.get("capital", 0), 1, 1e13),
        costs=Costs(
            commission_pct=_num(body, "commission_pct", d.get("commission_pct", 0), 0, 5),
            tax_pct=_num(body, "tax_pct", d.get("tax_pct", 0), 0, 5),
            slippage_pct=_num(body, "slippage_pct", d.get("slippage_pct", 0), 0, 5),
        ),
        risk=risk,
        market_timing=mt_cfg,
    )


def _strategy(cls: type, params: dict) -> Strategy:
    try:
        return cls(params)
    except StrategyParamError as e:
        raise ApiError(400, "invalid_params", str(e))


def _load(s: Setup, warmup: int) -> tuple:
    """Fetch bars covering the period + warm-up. Uses fast public history/intraday when live (Toss fallback)."""
    tf = getattr(s.cls, "timeframe", "1d")
    is_mock = current_app.config["MARKET_PROVIDERS"][s.market.value] != "toss"
    if tf != "1d":
        interval = Interval(tf)
        bars: Optional[Bars] = None
        candles = []
        if not is_mock:
            bars = screener_data.fetch_public_intraday(s.market.value, s.symbol, tf)
            if bars is not None:
                candles = screener_data.bars_to_candles(bars)
        if bars is None:
            candles = get_provider(s.market).get_candles(s.market, s.symbol, interval, limit=400)
            bars = Bars.from_candles(candles)
        start = warmup
        if start >= len(bars) - 1:
            raise ApiError(400, "not_enough_data", "분봉 백테스트에 필요한 데이터가 부족합니다")
        return candles, bars, start, []
    days = PERIOD_DAYS[s.period]
    start_ts = 0 if days is None else int(time.time() - days * 86400)
    # extra calendar days so indicators are warmed up at the start date (~1.5 calendar days per trading day)
    fetch_from = 0 if days is None else int(start_ts - (warmup * 1.6 + 15) * 86400)
    bars_d: Optional[Bars] = None
    candles_d = []
    if not is_mock:
        pub = screener_data.fetch_public_history(s.market.value, [s.symbol], period=s.period)
        bars_d = pub.get(s.symbol)
        if bars_d is not None:
            candles_d = screener_data.bars_to_candles(bars_d)
    if bars_d is None:
        candles_d = get_provider(s.market).get_history(s.market, s.symbol, Interval.D1, fetch_from)
        bars_d = Bars.from_candles(candles_d)
    first = next((i for i, t in enumerate(bars_d.time) if t >= start_ts), len(bars_d))
    start = max(first, warmup)
    if start >= len(bars_d) - 1:
        raise ApiError(400, "not_enough_data", "백테스트에 필요한 데이터가 부족합니다")
    warn = ["start_adjusted"] if start > first and days is not None else []
    return candles_d, bars_d, start, warn


def _context(s: Setup, bars: Bars, start: int) -> dict:
    return {
        "market": s.market.value,
        "symbol": s.symbol,
        "strategy": s.cls.id,
        "interval": getattr(s.cls, "timeframe", "1d"),
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
    return x, y, objective, min_trades, base, items


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
    regimes = None
    if s.market_timing["mode"] != "off":
        bench_bars = market_timing.load_benchmark_bars(s.market.value, s.market_timing["benchmark"], bars.time[0])
        regimes = market_timing.align_regimes_to_bars(
            bars, bench_bars, s.market_timing["ma_short"], s.market_timing["ma_long"]
        )
    result = Backtester(
        bars, strategy, s.capital, s.costs, start,
        risk=s.risk, regimes=regimes, market_timing_mode=s.market_timing["mode"],
    ).run()
    codes = result.pop("warnings") + warn
    return jsonify(
        **_context(s, bars, start),
        params=strategy.p,
        risk=s.risk.to_dict(),
        market_timing=s.market_timing,
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
    x, y, objective, min_trades, base, items = _grid_inputs(s, body)
    if len(items) > MAX_GRID:
        raise ApiError(400, "range_too_large", f"조합이 {len(items)}개입니다. 최대 {MAX_GRID}개까지 가능합니다")
    t0 = time.time()
    # every combination is evaluated on the same period: start after the longest warm-up in the grid
    _, bars, start, warn = _load(s, max_warmup(items))
    grid = worker.run_optimize(bars, s.cls.id, base, x, y, s.capital, s.costs, start, objective, min_trades)
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
    if getattr(s.cls, "timeframe", "1d") != "1d":
        raise ApiError(
            400,
            "daily_strategy_only",
            "워크포워드는 수년 단위 검증이므로 일봉(스윙·일봉 단타) 전략에서만 사용할 수 있습니다. 분봉 단타 전략은 [백테스트 · 최적화 · 몬테카를로]를 이용해 주세요.",
        )
    x, y, objective, min_trades, base, items = _grid_inputs(s, body)
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
    wf = worker.run_walkforward(
        bars, s.cls.id, base, x, y, s.capital, s.costs, start, objective, min_trades, is_years, oos_months, mode
    )
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
    result = worker.run_montecarlo(bars, s.cls.id, strategy.p, s.capital, s.costs, start, runs, method)
    return jsonify(
        **_context(s, bars, start),
        params=strategy.p,
        warnings=[WARNINGS.get(c, c) for c in warn],
        **result,
    )


# ------------------------------------------------------------------ portfolio (Phase 3)
@bp.get("/portfolio/models")
@login_required
def portfolio_models():
    return jsonify(
        items=[m.meta() for m in portfolio.PORTFOLIO_MODELS.values()],
        rebalance_modes=portfolio.REBALANCE_MODES,
    )


@bp.post("/portfolio/backtest")
@login_required
def portfolio_backtest():
    """Multi-asset ETF / portfolio allocation backtest across classic financial theories."""
    body = request.get_json(silent=True) or {}
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    model = portfolio.PORTFOLIO_MODELS.get(body.get("model"))
    if model is None:
        raise ApiError(400, "unknown_model", "알 수 없는 포트폴리오 이론입니다")
    try:
        params = model.resolve(body.get("params"))
    except StrategyParamError as e:
        raise ApiError(400, "invalid_params", str(e))
    assets = portfolio.validate_assets(market.value, body.get("assets") or model.presets.get(market.value), model)
    rebalance = str(body.get("rebalance") or model.default_rebalance)
    if rebalance not in portfolio.REBALANCE_MODES:
        raise ApiError(400, "invalid_rebalance")
    drift_pct = _num(body, "drift_pct", 5.0, 0.5, 50.0)
    period = body.get("period", "5y")
    if period not in PERIOD_DAYS:
        raise ApiError(400, "invalid_period")
    d = cost_defaults(market) if "commission_pct" not in body else {}
    capital = _num(body, "capital", d.get("capital", 0), 1, 1e13)
    costs = Costs(
        commission_pct=_num(body, "commission_pct", d.get("commission_pct", 0), 0, 5),
        tax_pct=_num(body, "tax_pct", d.get("tax_pct", 0), 0, 5),
        slippage_pct=_num(body, "slippage_pct", d.get("slippage_pct", 0), 0, 5),
    )

    days = PERIOD_DAYS[period]
    start_ts = 0 if days is None else int(time.time() - days * 86400)
    warm = model.warmup(params)
    fetch_from = 0 if days is None else int(start_ts - (warm * 1.6 + 20) * 86400)
    is_mock = current_app.config["MARKET_PROVIDERS"][market.value] != "toss"
    syms = [a["symbol"] for a in assets]
    bars_by_sym: dict[str, Bars] = {}
    if not is_mock:
        bars_by_sym = screener_data.fetch_public_history(market.value, syms, period=period)
    missing = [s for s in syms if s not in bars_by_sym]
    if missing:
        provider = get_provider(market)
        for sym in missing:
            bars_by_sym[sym] = Bars.from_candles(provider.get_history(market, sym, Interval.D1, fetch_from))
    res = worker.run_portfolio(bars_by_sym, model.id, assets, params, rebalance, drift_pct, capital, costs, start_ts)
    warn_codes = res.pop("warnings", [])
    return jsonify(
        market=market.value,
        model=model.id,
        assets=assets,
        params=params,
        rebalance=rebalance,
        drift_pct=drift_pct,
        capital=capital,
        currency="KRW" if market == Market.KR else "USD",
        costs=costs.__dict__,
        warnings=[WARNINGS.get(c, c) for c in warn_codes],
        **res,
    )


# ------------------------------------------------------------------ screener (Quant Stock Discovery)
@bp.get("/screener/presets")
@login_required
def screener_presets():
    return jsonify(screener.presets_meta())


@bp.post("/screener/run")
@login_required
def screener_run():
    body = request.get_json(silent=True) or {}
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    preset = str(body.get("preset") or "momentum_quality")
    universe = str(body.get("universe") or ("kr_all_150" if market == Market.KR else "us_sp100"))
    custom = body.get("symbols")
    if isinstance(custom, str):
        custom = [s.strip() for s in re.split(r"[\s,;]+", custom) if s.strip()]
    top_n = int(_num(body, "top_n", 20, 1, 40))
    return jsonify(
        screener.run_screen(
            market,
            preset,
            universe,
            custom_symbols=custom if isinstance(custom, list) else None,
            strategy_id=body.get("strategy"),
            exclude_warnings=bool(body.get("exclude_warnings", True)),
            strict_only=bool(body.get("strict_only", False)),
            top_n=top_n,
        )
    )


@bp.post("/screener/rotation-backtest")
@login_required
def screener_rotation_backtest():
    """Historical backtest for screener-linked periodic top-N stock rotation (MT-6)."""
    from flask import g
    from ..autotrade import rotation

    body = request.get_json(silent=True) or {}
    try:
        market = Market(str(body.get("market", "")).upper())
    except ValueError:
        raise ApiError(400, "invalid_market")
    preset = str(body.get("preset") or "momentum_quality")
    universe = str(body.get("universe") or ("kr_all_150" if market == Market.KR else "us_sp100"))
    custom = body.get("symbols")
    if isinstance(custom, str):
        custom = [s.strip() for s in re.split(r"[\s,;]+", custom) if s.strip()]
    top_n = int(_num(body, "top_n", 5, 2, 15))
    hold_buffer = int(_num(body, "hold_buffer", max(top_n * 2, 10), top_n, 40))
    cadence = str(body.get("cadence") or "monthly")
    period = str(body.get("period") or "3y")
    d = cost_defaults(market)
    capital = _num(body, "capital", d.get("capital", 10_000_000), 1, 1e13)
    return jsonify(
        rotation.run_rotation_backtest(
            market,
            preset,
            universe,
            custom_symbols=custom if isinstance(custom, list) else None,
            top_n=top_n,
            hold_buffer=hold_buffer,
            cadence=cadence,
            period=period,
            capital=capital,
            user_id=g.user["id"],
        )
    )
