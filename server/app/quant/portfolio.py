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
        guide: Optional[dict[str, Any]] = None,
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
        self.guide = guide or {}

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
            "guide": self.guide,
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
            guide={
                "background": (
                    "세계 최대 헤지펀드 브리지워터(Bridgewater)의 레이 달리오(Ray Dalio)가 창안한 거시경제 리스크 균형 이론입니다. "
                    "미래 경제는 예측할 수 없으므로 ① 성장 상승 ② 성장 둔화 ③ 물가 상승 ④ 물가 하락의 4계절 각각에서 강세를 보이는 "
                    "주식(성장)·국채(침체/디플레)·금/원자재(인플레)에 위험 비중을 균등 배분해 어떤 경기 국면에서도 치명적 손실을 피하는 구조입니다."
                ),
                "setup": [
                    "기본 비중: 주식 30% / 장기채 40% / 중기채 15% / 금 7.5% / 원자재 7.5% (합계 100%로 자동 정규화).",
                    "리밸런싱 주기: 분기 초(권장) 또는 비중 편차 이탈 ±5% 설정 시 거래비용 대비 리밸런싱 보너스가 우수합니다.",
                    "운용 자금은 5개 ETF 중 비중이 가장 작은 7.5% 종목도 최소 1주 이상 매수 가능한 금액 이상으로 잡으세요.",
                ],
                "execution": [
                    "정기일(또는 편차 초과일) 정규장 시작 1분 뒤 현재 평가총액과 각 자산의 목표 수량을 산출합니다.",
                    "목표 비중보다 초과 보유한 자산(오른 자산)의 매도 주문을 먼저 전송하고 짧은 체결 확인 폴링을 수행합니다.",
                    "확보된 현금과 봇 자금 한도 내에서 부족한 자산(내린 자산)을 시장가로 매수해 목표 비중을 복원합니다.",
                ],
                "caution": "2022년처럼 금리 급등으로 주식과 장기채가 동시에 하락하는 이례적 인플레이션 국면에서는 방어력이 약화될 수 있습니다.",
            },
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
            guide={
                "background": (
                    "1952년 해리 마코위츠(Harry Markowitz, 노벨경제학상)의 현대 포트폴리오 이론(MPT)을 가장 단순하고 견고하게 구현한 "
                    "표준 자산배분입니다. 기대수익이 높은 주식과 상관관계가 낮거나 음(-)인 채권을 60:40으로 결합하면, "
                    "단독 자산 대비 변동성(위험)을 크게 낮추면서 위험조정수익(샤프비율)을 개선할 수 있습니다."
                ),
                "setup": [
                    "기본 비중: 주식(SPY / KODEX 200) 60% + 채권(AGG / 국고채10년) 40%. 공격 성향이면 70:30, 안정 성향이면 40:60으로 조절하세요.",
                    "리밸런싱 주기: 분기 초 또는 연간 1회(또는 편차 ±5%)로 불필요한 매매 회전율을 줄이는 것이 유리합니다.",
                ],
                "execution": [
                    "리밸런싱 예정일 정규장 시작 1분 뒤 주식·채권 현재 비중과 목표 비중의 차이를 계산합니다.",
                    "비중이 커진 자산을 먼저 시장가 매도해 현금을 확보한 뒤, 비중이 줄어든 자산을 시장가 매수합니다.",
                    "비예정일에는 매매하지 않고 일일 평가자산 추이만 기록합니다.",
                ],
                "caution": "장기 주식 강세장에서는 주식 100% 대비 수익률이 뒤처지며, 급격한 금리 인상기에는 채권 완충 효과가 줄어듭니다.",
            },
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
            guide={
                "background": (
                    "DeMiguel, Garlappi & Uppal(2009, Review of Financial Studies)이 14개 정교한 최적화 모델과 단순 1/N 분산의 "
                    "표본 외(Out-of-Sample) 성과를 비교해 '추정 오차 때문에 단순 동일 비중(1/N)을 이기기 매우 어렵다'는 결론을 낸 "
                    "학술 이론에 기반합니다. 정기적으로 1/N로 되돌리면 자연스럽게 고점 매도·저점 매수(리밸런싱 보너스)가 발생합니다."
                ),
                "setup": [
                    "서로 상관관계가 낮은 3~6개 자산군(예: 미국주식 + 기술주 + 장기채 + 금)을 고를수록 분산 효과가 커집니다.",
                    "별도 비중 입력 없이 편입된 모든 종목에 자동으로 정확히 1/N씩 균등 배분됩니다.",
                    "리밸런싱은 월간 또는 분기 주기를 권장합니다.",
                ],
                "execution": [
                    "리밸런싱 시점마다 총평가액을 종목 수(N)로 균등 분할해 종목별 목표 수량을 계산합니다.",
                    "목표 수량을 초과한 종목을 먼저 시장가 매도한 뒤, 부족한 종목을 시장가 매수합니다.",
                ],
                "caution": "변동성이 극단적으로 다른 자산을 섞으면 변동성이 큰 종목(예: 레버리지·테마주)이 전체 포트폴리오 위험을 지배할 수 있습니다.",
            },
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
            guide={
                "background": (
                    "에드워드 치엔(Edward Qian, PanAgora)이 정립한 리스크 패리티(Risk Parity) 이론입니다. 전통 60/40 포트폴리오는 자본은 60:40이지만 "
                    "주식의 변동성이 채권보다 훨씬 커 실제 위험의 약 90%가 주식에 쏠립니다. 각 자산의 최근 실현 변동성 역수(w_i ∝ 1/σ_i)로 "
                    "비중을 동적 조절하면 모든 자산이 전체 변동성에 균등하게 기여하도록 제어할 수 있습니다."
                ),
                "setup": [
                    "변동성 기간(vol_window): 기본 60거래일(약 3개월). 민감한 위험 대응은 20~40일, 안정적 회전율은 60~120일이 적합합니다.",
                    "주식·장기채·중기채·금 등 변동성 수준과 상관관계가 다른 ETF 3~5개 조합을 권장합니다.",
                    "리밸런싱 주기: 매월 초가 변동성 변화를 반영하면서도 회전율을 과도하게 높이지 않는 균형점입니다.",
                ],
                "execution": [
                    "리밸런싱일 정규장 시작 1분 뒤, 전일 종가 기준 최근 vol_window일간 각 자산의 일간 수익률 표준편차(σ)를 계산합니다.",
                    "각 자산의 1/σ 비율로 목표 비중을 자동 산출합니다(변동성이 급등한 자산의 목표 비중 자동 축소).",
                    "초과 보유 종목을 먼저 매도하고, 확보된 현금으로 부족 종목을 매수해 새 목표 비중을 맞춥니다.",
                ],
                "caution": "변동성이 매우 낮은 단기채·현금성 ETF를 함께 넣으면 해당 자산으로 비중이 지나치게 쏠려 기대수익이 낮아질 수 있습니다.",
            },
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
            guide={
                "background": (
                    "게리 안토나치(Gary Antonacci)가 2012년 Wagner Award 수상 논문과 저서 'Dual Momentum Investing'에서 체계화한 "
                    "글로벌 주식 모멘텀(GEM) 이론입니다. ① 횡단면 '상대 모멘텀'(공격 자산 중 승자 선택)으로 수익을 극대화하고, "
                    "② 시계열 '절대 모멘텀'(선택 자산의 과거 수익률 > 0 여부)으로 글로벌 금융위기 같은 대형 하락장에서 채권으로 대피합니다."
                ),
                "setup": [
                    "자산 역할(role): 반드시 공격(offensive: 미국/선진국/국내 주식 ETF)과 방어(defensive: 종합채권/국채 ETF)를 각각 1개 이상 지정하세요.",
                    "모멘텀 기간(lookback): 원전 기본값은 252거래일(12개월). 빠른 전환은 126일(6개월)도 널리 쓰입니다.",
                    "상위 선택 개수(top_n): 원전 GEM은 1개(집중), 분산을 원하면 공격 자산 3~4개 중 상위 2개로 설정합니다.",
                    "리밸런싱 주기: 매월 초(월 1회 모멘텀 점검)가 표준입니다.",
                ],
                "execution": [
                    "매월 첫 거래일 정규장 시작 1분 뒤, 전일 종가 기준 모든 자산의 lookback일 누적 수익률을 계산합니다.",
                    "공격 자산 중 수익률 상위 top_n개를 선정하고, 각 자산의 수익률이 0% 초과면 투자, 0% 이하면 최고 모멘텀 방어 자산으로 대체합니다.",
                    "교체가 발생하면 기존 보유 종목을 먼저 전량 시장가 매도 → 체결 확인 후 신규 선정 종목을 시장가 매수합니다.",
                ],
                "caution": "급락 후 V자 급반등하는 장세에서는 모멘텀 전환이 한 템포 늦어 단기 '채찍 효과(휩소)' 손실이 발생할 수 있습니다.",
            },
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
            guide={
                "background": (
                    "Wouter Keller & Jan Willem Keuning(2017, 'Breadth Momentum and Vigilant Asset Allocation')이 발표한 "
                    "카나리아 기반 동적 자산배분 이론입니다. 최근 수익률에 높은 가중치를 주는 13612W 모멘텀(12×r1m + 4×r3m + 2×r6m + 1×r12m)을 "
                    "계산해, 공격 자산 중 '단 1개라도 음수'면 시장 전반의 균열 신호(광산의 카나리아)로 간주하고 즉시 방어 채권으로 100% 대피합니다."
                ),
                "setup": [
                    "공격 자산(offensive): 원전 G4는 SPY(미국)·VEA(선진국)·VWO(신흥국)·AGG(종합채권) 4종 조합입니다.",
                    "방어 자산(defensive): 원전은 SHY(단기채)·IEF(중기채)·LQD(회사채) 3종 중 가장 강한 1종으로 대피합니다.",
                    "공격 상위 종목 수(top_offensive): 공격형은 1(집중), 안정적 분산은 2로 설정합니다.",
                    "리밸런싱 주기: 매월 초 고정(최소 1년 이상 상장 이력이 있는 ETF 필요).",
                ],
                "execution": [
                    "매월 첫 거래일 정규장 시작 1분 뒤, 모든 편입 ETF의 1·3·6·12개월 가중 모멘텀(13612W) 점수를 산출합니다.",
                    "공격 자산 전부의 점수가 양수(+)일 때만 공격 상위 top_offensive개 종목에 균등 배분합니다.",
                    "공격 자산 중 1개라도 0 이하이면 공격 자산을 전량 매도하고, 방어 자산 중 13612W 점수가 가장 높은 1종목에 100% 투자합니다.",
                ],
                "caution": "공격↔방어 전환이 100% 단위로 자주 일어나므로 연간 회전율이 높습니다. 거래비용이 낮은 미국/국내 대표 유동성 ETF로 운용하세요.",
            },
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
