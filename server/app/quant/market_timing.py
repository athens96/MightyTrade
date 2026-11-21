"""Market Timing ('시장 온도 신호등') regime calculation for Quant Lab & AutoTrade bots (MT-5).

Evaluates a market benchmark ETF (KR: KODEX 200 `069500` or KOSDAQ 150 `229200`; US: `SPY` or `QQQ`)
against short/long moving averages (default 20d / 60d) on completed daily bars (zero look-ahead):
- `bull`    (🟢 상승장): close >= MA_short and close >= MA_long
- `caution` (🟡 주의장): between MA_short and MA_long
- `bear`    (🔴 하락장): close < MA_short and close < MA_long
"""
from __future__ import annotations

import bisect
import time
from typing import Any, Optional

from flask import current_app

from ..errors import ApiError
from ..market.models import Interval, Market
from ..market.registry import get_provider
from . import indicators as ind
from . import screener_data as sdata
from .engine import Bars

VALID_MODES = ("off", "block_buy", "half_size", "exit_all")
BENCHMARKS: dict[str, list[tuple[str, str]]] = {
    "KR": [("069500", "KODEX 200 (코스피)"), ("229200", "KODEX 코스닥150")],
    "US": [("SPY", "SPY (미국 S&P500)"), ("QQQ", "QQQ (미국 나스닥100)")],
}
DEFAULT_BENCHMARK = {"KR": "069500", "US": "SPY"}
REGIME_LABELS = {
    "bull": "🟢 상승장",
    "caution": "🟡 혼조·주의",
    "bear": "🔴 하락·방어",
}

_REGIME_CACHE: dict[str, tuple[float, dict[str, Any]]] = {}
_CACHE_TTL_SEC = 300.0


def default_market_timing(market: str = "KR") -> dict[str, Any]:
    return {
        "mode": "off",
        "benchmark": DEFAULT_BENCHMARK.get(market, "069500"),
        "ma_short": 20,
        "ma_long": 60,
    }


def validate_market_timing(raw: Optional[dict], market: str) -> dict[str, Any]:
    d = default_market_timing(market)
    if not raw or not isinstance(raw, dict):
        return d
    mode = str(raw.get("mode") or "off")
    if mode not in VALID_MODES:
        raise ApiError(400, "invalid_market_timing", "올바르지 않은 마켓 타이밍 모드입니다")
    allowed_syms = {s for s, _ in BENCHMARKS.get(market, [])}
    bench = str(raw.get("benchmark") or d["benchmark"]).strip().upper()
    if bench not in allowed_syms:
        bench = d["benchmark"]
    try:
        ma_s = int(float(raw.get("ma_short", 20)))
        ma_l = int(float(raw.get("ma_long", 60)))
    except (TypeError, ValueError):
        raise ApiError(400, "invalid_market_timing", "이동평균 기간은 정수여야 합니다")
    if not (5 <= ma_s <= 120 and 10 <= ma_l <= 300 and ma_s < ma_l):
        raise ApiError(400, "invalid_market_timing", "단기 이평(5~120)은 장기 이평(10~300)보다 작아야 합니다")
    return {"mode": mode, "benchmark": bench, "ma_short": ma_s, "ma_long": ma_l}


def compute_regime_series(bars: Bars, ma_short: int = 20, ma_long: int = 60) -> list[str]:
    """Regime string ('bull' | 'caution' | 'bear') at each bar index of `bars`."""
    n = len(bars)
    if n == 0:
        return []
    s_line = ind.sma(bars.close, ma_short)
    l_line = ind.sma(bars.close, ma_long)
    out: list[str] = []
    for i in range(n):
        c = bars.close[i]
        sv = s_line[i]
        lv = l_line[i]
        if sv is None or lv is None:
            out.append("bull")
        elif c >= sv and c >= lv:
            out.append("bull")
        elif c < sv and c < lv:
            out.append("bear")
        else:
            out.append("caution")
    return out


def align_regimes_to_bars(target_bars: Bars, bench_bars: Bars, ma_short: int = 20, ma_long: int = 60) -> list[str]:
    """For each bar `j` of `target_bars`, return the regime decided BEFORE `target_bars.time[j]` opens.

    Specifically uses the latest benchmark daily bar with `time < target_day_start` so there is zero look-ahead.
    """
    if len(bench_bars) == 0:
        return ["bull"] * len(target_bars)
    b_regimes = compute_regime_series(bench_bars, ma_short, ma_long)
    b_days = [int(t // 86400) for t in bench_bars.time]
    out: list[str] = []
    for t in target_bars.time:
        t_day = int(t // 86400)
        idx = bisect.bisect_left(b_days, t_day) - 1
        out.append(b_regimes[idx] if idx >= 0 else "bull")
    return out


def load_benchmark_bars(market: str, benchmark: str, start_ts: int = 0) -> Bars:
    m = Market(market)
    is_mock = current_app.config["MARKET_PROVIDERS"][market] != "toss"
    if not is_mock:
        period = "10y" if start_ts > 0 and (time.time() - start_ts) > 5 * 365 * 86400 else "5y"
        pub = sdata.fetch_public_history(market, [benchmark], period=period)
        if benchmark in pub and len(pub[benchmark]) >= 30:
            return pub[benchmark]
    fetch_from = max(0, int((start_ts or time.time()) - 400 * 86400))
    candles = get_provider(m).get_history(m, benchmark, Interval.D1, fetch_from)
    return Bars.from_candles(candles)


def current_regime_snapshot(market: str, benchmark: Optional[str] = None, ma_short: int = 20, ma_long: int = 60,
                            before_date: Optional[str] = None) -> dict[str, Any]:
    """Latest completed-bar regime snapshot for UI badges and live/paper bot execution."""
    bench = benchmark or DEFAULT_BENCHMARK.get(market, "069500")
    cache_key = f"{market}:{bench}:{ma_short}:{ma_long}:{before_date or 'latest'}"
    now = time.time()
    is_mock = current_app.config["MARKET_PROVIDERS"][market] != "toss"
    if not is_mock and before_date is None and cache_key in _REGIME_CACHE:
        ts, cached = _REGIME_CACHE[cache_key]
        if now - ts < _CACHE_TTL_SEC:
            return cached

    bars = load_benchmark_bars(market, bench, int(now - 260 * 86400))
    if before_date:
        from datetime import datetime, timezone
        keep_idx = [
            i for i, t in enumerate(bars.time)
            if datetime.fromtimestamp(t, timezone.utc).date().isoformat() < before_date
        ]
        if keep_idx:
            end_i = keep_idx[-1] + 1
            bars = Bars(bars.time[:end_i], bars.open[:end_i], bars.high[:end_i], bars.low[:end_i], bars.close[:end_i], bars.volume[:end_i])

    if len(bars) == 0:
        snap = {"market": market, "benchmark": bench, "regime": "bull", "label": REGIME_LABELS["bull"],
                "close": None, "ma_short": ma_short, "ma_long": ma_long, "ma_short_val": None, "ma_long_val": None}
        return snap

    regimes = compute_regime_series(bars, ma_short, ma_long)
    s_val = ind.sma(bars.close, ma_short)[-1]
    l_val = ind.sma(bars.close, ma_long)[-1]
    reg = regimes[-1]
    bench_name = next((nm for sym, nm in BENCHMARKS.get(market, []) if sym == bench), bench)
    snap = {
        "market": market,
        "benchmark": bench,
        "benchmark_name": bench_name,
        "regime": reg,
        "label": REGIME_LABELS[reg],
        "close": round(bars.close[-1], 2),
        "ma_short": ma_short,
        "ma_long": ma_long,
        "ma_short_val": round(s_val, 2) if s_val is not None else None,
        "ma_long_val": round(l_val, 2) if l_val is not None else None,
    }
    if not is_mock and before_date is None:
        _REGIME_CACHE[cache_key] = (now, snap)
    return snap


def buy_scale_for(mode: str, regime: str) -> float:
    """Multiplier in [0.0, 1.0] applied to new BUY sizing under `mode` and `regime`."""
    if mode == "off":
        return 1.0
    if mode in ("block_buy", "exit_all"):
        return 0.0 if regime == "bear" else 1.0
    if mode == "half_size":
        if regime == "bear":
            return 0.5
        if regime == "caution":
            return 0.75
    return 1.0
