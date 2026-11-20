"""Multi-asset portfolio allocation theories and rebalancing backtest engine (Quant Phase 3).

Shared by Quant Lab portfolio backtests and multi-asset rebalancing bots so research and live
rebalancing use the exact same target-weight math (zero look-ahead: weights decided at bar i close
are traded at bar i+1 open, sells before buys).
"""
from __future__ import annotations

import math
import re
from dataclasses import asdict
from datetime import datetime, timezone
from typing import Any, Callable, Optional

from ..errors import ApiError
from .engine import Bars, Costs
from .metrics import drawdown_series, performance
from .strategies.base import Param, StrategyParamError

SYMBOL_RE = {"KR": re.compile(r"[0-9A-Z]{6}"), "US": re.compile(r"[A-Z][A-Z0-9.\-]{0,11}")}
REBALANCE_MODES = {
    "monthly": "매월 초",
    "quarterly": "분기 초",
    "yearly": "매년 초",
    "drift": "비중 편차 이탈 시",
}
MIN_ASSETS = 2
MAX_ASSETS = 12


def _day_str(t: int) -> str:
    return datetime.fromtimestamp(t, timezone.utc).date().isoformat()


def _period_key(day: str, mode: str) -> tuple[int, int]:
    y, m = int(day[:4]), int(day[5:7])
    if mode == "monthly":
        return (y, m)
    if mode == "quarterly":
        return (y, (m - 1) // 3 + 1)
    if mode == "yearly":
        return (y, 1)
    return (0, 0)


def should_rebalance(
    today: str,
    last_rebalance_date: Optional[str],
    mode: str,
    current_weights: dict[str, float],
    target_weights: dict[str, float],
    drift_pct: float = 5.0,
) -> Optional[str]:
    """Return human-readable rebalance reason if due today, else None."""
    if last_rebalance_date is None:
        return "최초 포트폴리오 구성"
    if mode in ("monthly", "quarterly", "yearly"):
        if _period_key(today, mode) != _period_key(last_rebalance_date, mode):
            return f"정기 리밸런싱 ({REBALANCE_MODES[mode]})"
    if mode == "drift" or drift_pct > 0:
        syms = set(current_weights) | set(target_weights)
        max_drift = max((abs(current_weights.get(s, 0.0) - target_weights.get(s, 0.0)) * 100.0 for s in syms), default=0.0)
        if mode == "drift" and max_drift >= drift_pct:
            return f"비중 편차 {max_drift:.1f}% ≥ {drift_pct:g}% 리밸런싱"
    return None


# ------------------------------------------------------------------ allocation theories
def _resolve_params(param_defs: list[Param], raw: Optional[dict]) -> dict[str, Any]:
    raw = raw or {}
    out: dict[str, Any] = {}
    for prm in param_defs:
        v = raw.get(prm.key, prm.default)
        try:
            v = float(v)
        except (TypeError, ValueError):
            raise StrategyParamError(f"{prm.label}: 숫자를 입력하세요")
        if not prm.min <= v <= prm.max:
            raise StrategyParamError(f"{prm.label}: {prm.min:g} ~ {prm.max:g} 범위여야 합니다")
        out[prm.key] = int(v) if prm.integer else v
    return out


def _fixed_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    total = sum(max(float(a.get("weight") or 0.0), 0.0) for a in assets)
    if total <= 0:
        eq = 1.0 / len(assets)
        return {a["symbol"]: eq for a in assets}
    return {a["symbol"]: max(float(a.get("weight") or 0.0), 0.0) / total for a in assets}


def _equal_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    eq = 1.0 / len(assets)
    return {a["symbol"]: eq for a in assets}


def _risk_parity_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    w = int(params["vol_window"])
    inv: dict[str, float] = {}
    for a in assets:
        s = a["symbol"]
        cs = closes[s]
        start = max(1, i - w + 1)
        rets = [cs[k] / cs[k - 1] - 1.0 for k in range(start, i + 1) if cs[k - 1] > 0]
        if len(rets) < 2:
            inv[s] = 1.0
            continue
        mean = sum(rets) / len(rets)
        sd = math.sqrt(sum((r - mean) ** 2 for r in rets) / (len(rets) - 1))
        inv[s] = 1.0 / max(sd, 1e-6)
    total = sum(inv.values()) or 1.0
    return {s: v / total for s, v in inv.items()}


def _dual_momentum_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    lb = int(params["lookback"])
    top_n = max(1, int(params["top_n"]))
    off = [a["symbol"] for a in assets if a.get("role", "offensive") != "defensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]
    if not off:
        off = [a["symbol"] for a in assets]

    def ret(sym: str) -> float:
        cs = closes[sym]
        if i < lb or cs[i - lb] <= 0:
            return -math.inf
        return cs[i] / cs[i - lb] - 1.0

    ranked_off = sorted(((ret(s), s) for s in off), reverse=True)
    best_def = max(((ret(s), s) for s in dfn), default=(0.0, None))[1] if dfn else None
    picks = ranked_off[: min(top_n, len(ranked_off))]
    slice_w = 1.0 / len(picks) if picks else 0.0
    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    for r, sym in picks:
        if r > 0:
            out[sym] = out.get(sym, 0.0) + slice_w
        elif best_def is not None:
            out[best_def] = out.get(best_def, 0.0) + slice_w
    return out


def _vaa_score(cs: list[float], i: int) -> float:
    """Keller & Keuning 13612W weighted momentum: 12*r1m + 4*r3m + 2*r6m + 1*r12m."""
    if i < 252:
        return -math.inf
    c = cs[i]
    r1 = c / cs[i - 21] - 1.0 if cs[i - 21] > 0 else 0.0
    r3 = c / cs[i - 63] - 1.0 if cs[i - 63] > 0 else 0.0
    r6 = c / cs[i - 126] - 1.0 if cs[i - 126] > 0 else 0.0
    r12 = c / cs[i - 252] - 1.0 if cs[i - 252] > 0 else 0.0
    return 12.0 * r1 + 4.0 * r3 + 2.0 * r6 + r12


def _vaa_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    top_off = max(1, int(params["top_offensive"]))
    off = [a["symbol"] for a in assets if a.get("role", "offensive") != "defensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]
    if not off:
        off = [a["symbol"] for a in assets]
    off_scores = [( _vaa_score(closes[s], i), s) for s in off]
    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    if all(sc > 0 for sc, _ in off_scores):
        picks = sorted(off_scores, reverse=True)[: min(top_off, len(off_scores))]
        w = 1.0 / len(picks)
        for _, sym in picks:
            out[sym] = w
    elif dfn:
        best_def = max((_vaa_score(closes[s], i), s) for s in dfn)[1]
        out[best_def] = 1.0
    return out


class PortfolioModel:
    def __init__(
        self,
        id: str,
        name: str,
        theory: str,
        summary: str,
        description: str,
        fn: Callable[[dict[str, list[float]], int, list[dict], dict], dict[str, float]],
        warmup_fn: Callable[[dict], int],
        *,
        uses_roles: bool = False,
        uses_weights: bool = False,
        params: Optional[list[Param]] = None,
        presets: Optional[dict[str, list[dict]]] = None,
        default_rebalance: str = "monthly",
    ):
        self.id = id
        self.name = name
        self.theory = theory
        self.summary = summary
        self.description = description
        self.fn = fn
        self.warmup_fn = warmup_fn
        self.uses_roles = uses_roles
        self.uses_weights = uses_weights
        self.params = params or []
        self.presets = presets or {}
        self.default_rebalance = default_rebalance

    def resolve(self, raw: Optional[dict]) -> dict[str, Any]:
        return _resolve_params(self.params, raw)

    def warmup(self, params: dict) -> int:
        return self.warmup_fn(params)

    def target_weights(self, closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
        return self.fn(closes, i, assets, params)

    def meta(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "name": self.name,
            "theory": self.theory,
            "summary": self.summary,
            "description": self.description,
            "uses_roles": self.uses_roles,
            "uses_weights": self.uses_weights,
            "default_rebalance": self.default_rebalance,
            "params": [asdict(p) for p in self.params],
            "presets": self.presets,
        }


PORTFOLIO_MODELS: dict[str, PortfolioModel] = {
    m.id: m
    for m in (
        PortfolioModel(
            "all_weather",
            "올웨더 (All Weather)",
            "레이 달리오 · 경제 4계절(성장·물가) 리스크 균형 자산배분",
            "주식 30% · 장기채 40% · 중기채 15% · 금 7.5% · 원자재 7.5% 고정 목표 비중 리밸런싱",
            "레이 달리오(Bridgewater)의 올웨더 포트폴리오. 경기 상승·침체와 인플레이션·디플레이션 4국면에서 "
            "서로 보완하는 자산군을 조합하고 정기·편차 리밸런싱으로 고평가 자산을 팔고 저평가 자산을 매수합니다.",
            _fixed_weights,
            lambda p: 2,
            uses_weights=True,
            default_rebalance="quarterly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 30},
                    {"symbol": "TLT", "name": "미국 장기국채", "role": "defensive", "weight": 40},
                    {"symbol": "IEF", "name": "미국 중기국채", "role": "defensive", "weight": 15},
                    {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 7.5},
                    {"symbol": "DBC", "name": "원자재", "role": "offensive", "weight": 7.5},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 30},
                    {"symbol": "148070", "name": "KOSEF 국고채10년", "role": "defensive", "weight": 40},
                    {"symbol": "114260", "name": "KODEX 국고채3년", "role": "defensive", "weight": 15},
                    {"symbol": "132030", "name": "KODEX 골드선물", "role": "defensive", "weight": 7.5},
                    {"symbol": "261220", "name": "KODEX WTI원유", "role": "offensive", "weight": 7.5},
                ],
            },
        ),
        PortfolioModel(
            "classic_60_40",
            "60/40 클래식 균형",
            "해리 마코위츠 · 현대 포트폴리오 이론(MPT) 주식·채권 분산",
            "주식 60% · 채권 40% 비중을 유지해 변동성 대비 수익(샤프비율) 개선",
            "해리 마코위츠의 현대 포트폴리오 이론(MPT)에 기반한 대표적 균형 자산배분. "
            "낮은 상관관계를 갖는 주식과 채권을 60:40으로 배분해 하락장 방어와 장기 복리 성장을 동시에 노립니다.",
            _fixed_weights,
            lambda p: 2,
            uses_weights=True,
            default_rebalance="quarterly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 60},
                    {"symbol": "AGG", "name": "미국 종합채권", "role": "defensive", "weight": 40},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 60},
                    {"symbol": "148070", "name": "KOSEF 국고채10년", "role": "defensive", "weight": 40},
                ],
            },
        ),
        PortfolioModel(
            "equal_weight",
            "동일 비중 (1/N)",
            "DeMiguel, Garlappi & Uppal (RFS 2009) · Naive 1/N 분산",
            "모든 편입 자산을 정확히 1/N 동일 비중으로 유지하고 정기 리밸런싱",
            "과거 데이터 추정 오차로 인한 평균-분산 최적화의 과적합을 피하고, 모든 자산에 1/N 동일 비중을 "
            "배분하는 학술 벤치마크 전략. 리밸런싱 시 오른 자산을 일부 매도하고 내린 자산을 매수하는 역추세 보너스를 얻습니다.",
            _equal_weights,
            lambda p: 2,
            default_rebalance="monthly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "S&P500", "role": "offensive", "weight": 25},
                    {"symbol": "QQQ", "name": "나스닥100", "role": "offensive", "weight": 25},
                    {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 25},
                    {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                    {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 25},
                    {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                    {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25},
                ],
            },
        ),
        PortfolioModel(
            "risk_parity",
            "리스크 패리티 (역변동성)",
            "에드워드 치엔 (Edward Qian) · 위험 균등 배분 (Inverse Volatility)",
            "최근 N일 실현 변동성의 역수(1/σ)로 비중을 자동 산출해 자산별 위험 기여도를 균등화",
            "자본이 아닌 '위험(변동성)'을 균등하게 나누는 리스크 패리티 이론. 변동성이 높은 주식 비중은 낮추고 "
            "변동성이 낮은 채권 비중은 높여 특정 자산의 폭락이 전체 포트폴리오를 지배하지 않도록 제어합니다.",
            _risk_parity_weights,
            lambda p: int(p["vol_window"]) + 1,
            params=[Param("vol_window", "변동성 기간 (일)", 60, 10, 252)],
            default_rebalance="monthly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "S&P500", "role": "offensive", "weight": 25},
                    {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 25},
                    {"symbol": "IEF", "name": "중기국채", "role": "defensive", "weight": 25},
                    {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                    {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                    {"symbol": "114260", "name": "국고채3년", "role": "defensive", "weight": 25},
                    {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25},
                ],
            },
        ),
        PortfolioModel(
            "dual_momentum",
            "글로벌 듀얼 모멘텀 (GEM)",
            "게리 안토나치 (Gary Antonacci) · 상대 모멘텀 + 절대 모멘텀",
            "공격 자산 중 수익률 상위 종목 선택, 수익률이 음수면 방어(채권) 자산으로 대피",
            "게리 안토나치의 듀얼 모멘텀(GEM) 이론. ① 상대 모멘텀으로 공격 자산 중 가장 강한 종목을 고르고, "
            "② 절대 모멘텀(과거 N일 수익률 > 0)이 깨지면 안전 채권으로 전환해 대형 약세장을 회피합니다.",
            _dual_momentum_weights,
            lambda p: int(p["lookback"]) + 1,
            uses_roles=True,
            params=[
                Param("lookback", "모멘텀 기간 (일)", 252, 20, 360),
                Param("top_n", "상위 선택 개수", 1, 1, 5),
            ],
            default_rebalance="monthly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 주식", "role": "offensive", "weight": 50},
                    {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 50},
                    {"symbol": "AGG", "name": "미국 종합채권 (방어)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "069500", "name": "국내 코스피200", "role": "offensive", "weight": 50},
                    {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 50},
                    {"symbol": "148070", "name": "국고채10년 (방어)", "role": "defensive", "weight": 0},
                ],
            },
        ),
        PortfolioModel(
            "vaa",
            "VAA 경계 자산배분",
            "Wouter Keller & Jan Willem Keuning (2017) · 13612W 카나리아 모멘텀",
            "공격 자산 전부 13612W 모멘텀 양수면 공격 투자, 1개라도 음수면 최고 방어 자산 100%",
            "Keller & Keuning의 Vigilant Asset Allocation(VAA). 1·3·6·12개월 가중 모멘텀(13612W)을 계산해 "
            "공격 자산군 중 단 하나라도 음수(카나리아 경보)가 뜨면 즉시 방어 채권으로 100% 전환해 최대낙폭(MDD)을 억제합니다.",
            _vaa_weights,
            lambda p: 253,
            uses_roles=True,
            params=[Param("top_offensive", "공격 상위 종목 수", 1, 1, 4)],
            default_rebalance="monthly",
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 주식", "role": "offensive", "weight": 25},
                    {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 25},
                    {"symbol": "VWO", "name": "신흥국 주식", "role": "offensive", "weight": 25},
                    {"symbol": "AGG", "name": "종합채권", "role": "offensive", "weight": 25},
                    {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "LQD", "name": "회사채 (방어)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                    {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 25},
                    {"symbol": "148070", "name": "국고채10년", "role": "offensive", "weight": 25},
                    {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 25},
                    {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0},
                ],
            },
        ),
    )
}


# ------------------------------------------------------------------ validation & alignment
def validate_assets(market: str, raw_assets: Any, model: PortfolioModel) -> list[dict]:
    if not isinstance(raw_assets, list) or not (MIN_ASSETS <= len(raw_assets) <= MAX_ASSETS):
        raise ApiError(400, "invalid_assets", f"자산은 {MIN_ASSETS}~{MAX_ASSETS}개여야 합니다")
    seen: set[str] = set()
    clean: list[dict] = []
    for item in raw_assets:
        if not isinstance(item, dict):
            raise ApiError(400, "invalid_assets", "각 자산은 객체여야 합니다")
        sym = str(item.get("symbol") or "").strip().upper()
        if not SYMBOL_RE[market].fullmatch(sym):
            raise ApiError(400, "invalid_symbol", f"{sym or '(빈 값)'}은(는) {market} 종목 코드가 아닙니다")
        if sym in seen:
            raise ApiError(400, "duplicate_symbol", f"중복된 종목입니다: {sym}")
        seen.add(sym)
        role = str(item.get("role") or "offensive").lower()
        if role not in ("offensive", "defensive"):
            raise ApiError(400, "invalid_role", "자산 역할은 offensive 또는 defensive여야 합니다")
        try:
            weight = float(item.get("weight", 0.0))
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_weight", f"{sym}: 비중은 숫자여야 합니다")
        if weight < 0:
            raise ApiError(400, "invalid_weight", f"{sym}: 비중은 0 이상이어야 합니다")
        clean.append({"symbol": sym, "name": str(item.get("name") or sym)[:40], "role": role, "weight": weight})
    if model.uses_weights and sum(a["weight"] for a in clean) <= 0:
        raise ApiError(400, "invalid_weight", "목표 비중 합계는 0보다 커야 합니다")
    if model.uses_roles:
        if not any(a["role"] == "offensive" for a in clean):
            raise ApiError(400, "invalid_assets", "공격(offensive) 자산이 최소 1개 필요합니다")
        if not any(a["role"] == "defensive" for a in clean):
            raise ApiError(400, "invalid_assets", "방어(defensive) 자산이 최소 1개 필요합니다")
    return clean


def align_bars(bars_by_symbol: dict[str, Bars]) -> tuple[list[int], dict[str, list[float]], dict[str, list[float]]]:
    """Intersect trading dates across all symbols. Returns (times, opens_by_sym, closes_by_sym)."""
    maps: dict[str, dict[int, tuple[float, float]]] = {}
    for sym, b in bars_by_symbol.items():
        maps[sym] = {t: (o, c) for t, o, c in zip(b.time, b.open, b.close)}
    common = sorted(set.intersection(*(set(m.keys()) for m in maps.values()))) if maps else []
    opens = {sym: [maps[sym][t][0] for t in common] for sym in bars_by_symbol}
    closes = {sym: [maps[sym][t][1] for t in common] for sym in bars_by_symbol}
    return common, opens, closes


# ------------------------------------------------------------------ backtest engine
def run_portfolio_backtest(
    bars_by_symbol: dict[str, Bars],
    model_id: str,
    assets: list[dict],
    params: dict,
    rebalance: str,
    drift_pct: float,
    capital: float,
    costs: Costs,
    start_ts: int,
) -> dict[str, Any]:
    """Multi-asset rebalancing backtest (bar i close decision -> bar i+1 open execution, sells before buys)."""
    model = PORTFOLIO_MODELS[model_id]
    times, opens, closes = align_bars(bars_by_symbol)
    warm = model.warmup(params)
    first = next((i for i, t in enumerate(times) if t >= start_ts), len(times))
    start = max(first, warm)
    if start >= len(times) - 2:
        raise ApiError(400, "not_enough_data", "포트폴리오 백테스트에 필요한 공통 거래일이 부족합니다")

    syms = [a["symbol"] for a in assets]
    comm = costs.commission_pct / 100.0
    tax = costs.tax_pct / 100.0
    slip = costs.slippage_pct / 100.0

    cash = capital
    holdings = {s: 0 for s in syms}
    last_rebal: Optional[str] = None
    pending_target: Optional[tuple[dict[str, float], str]] = None
    equity: list[float] = []
    rebalances: list[dict[str, Any]] = []
    total_trades = 0

    for j in range(start, len(times)):
        day = _day_str(times[j])
        # Execute pending rebalance decided at bar j-1 close at bar j open (sells first, then buys)
        if pending_target is not None:
            target_w, reason = pending_target
            pending_target = None
            val_open = cash + sum(holdings[s] * opens[s][j] for s in syms)
            desired: dict[str, int] = {}
            for s in syms:
                w = target_w.get(s, 0.0)
                px_buy = opens[s][j] * (1.0 + slip) * (1.0 + comm)
                desired[s] = math.floor(val_open * w / px_buy) if w > 0 and px_buy > 0 else 0

            traded_val = 0.0
            n_orders = 0
            # 1) Sells first to free cash
            for s in syms:
                diff = desired[s] - holdings[s]
                if diff < 0:
                    qty = -diff
                    px = opens[s][j] * (1.0 - slip)
                    gross = qty * px
                    cash += gross * (1.0 - comm - tax)
                    holdings[s] -= qty
                    traded_val += gross
                    n_orders += 1
            # 2) Buys second within available cash
            for s in syms:
                diff = desired[s] - holdings[s]
                if diff > 0:
                    px = opens[s][j] * (1.0 + slip)
                    unit_cost = px * (1.0 + comm)
                    qty = min(diff, math.floor(cash / unit_cost)) if unit_cost > 0 else 0
                    if qty > 0:
                        cash -= qty * unit_cost
                        holdings[s] += qty
                        traded_val += qty * px
                        n_orders += 1
            last_rebal = day
            total_trades += n_orders
            rebalances.append({
                "time": times[j],
                "date": day,
                "reason": reason,
                "trades": n_orders,
                "turnover_pct": round((traded_val / val_open * 100.0) if val_open > 0 else 0.0, 2),
                "weights": {s: round(target_w.get(s, 0.0) * 100.0, 2) for s in syms if target_w.get(s, 0.0) > 0},
            })

        # Mark-to-market at bar j close
        eq = cash + sum(holdings[s] * closes[s][j] for s in syms)
        equity.append(eq)

        # Decide at bar j close whether to rebalance on bar j+1 open
        cur_w = {s: (holdings[s] * closes[s][j] / eq if eq > 0 else 0.0) for s in syms}
        tgt_w = model.target_weights(closes, j, assets, params)
        next_day = _day_str(times[j + 1]) if j + 1 < len(times) else day
        reason = should_rebalance(next_day, last_rebal, rebalance, cur_w, tgt_w, drift_pct)
        if reason is not None and j + 1 < len(times):
            pending_target = (tgt_w, reason)

    # Benchmark: equal-weight buy-and-hold across the same aligned assets
    eq_w = 1.0 / len(syms)
    bh_qty = {}
    bh_cash = capital
    for s in syms:
        px = closes[s][start] * (1.0 + slip) * (1.0 + comm)
        q = math.floor(capital * eq_w / px) if px > 0 else 0
        bh_qty[s] = q
        bh_cash -= q * px
    bench = [bh_cash + sum(bh_qty[s] * closes[s][j] for s in syms) for j in range(start, len(times))]

    seg_times = times[start:]
    latest_target = model.target_weights(closes, len(times) - 1, assets, params)
    metrics = {
        **performance(seg_times, equity),
        "rebalances": len(rebalances),
        "trades": total_trades,
    }
    return {
        "equity": [
            {"time": t, "equity": round(e, 4), "benchmark": round(be, 4), "drawdown": round(dd * 100, 4)}
            for t, e, be, dd in zip(seg_times, equity, bench, drawdown_series(equity))
        ],
        "metrics": metrics,
        "benchmark": performance(seg_times, bench),
        "rebalances": rebalances,
        "current_weights": {s: round(latest_target.get(s, 0.0) * 100.0, 2) for s in syms},
        "period": {"start": seg_times[0], "end": seg_times[-1]},
        "warnings": ["start_adjusted"] if start > first and start_ts > 0 else [],
    }
