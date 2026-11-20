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
            "레이 달리오 · 경제가 4계절(호황·불황·물가상승·물가하락) 중 어느 쪽으로 가도 버티는 분산 투자",
            "주식(30%) · 장기국채(40%) · 중기국채(15%) · 금(7.5%) · 원자재(7.5%)에 나눠 담고 비율이 틀어지면 자동 정리합니다",
            "세계 최대 펀드를 만든 레이 달리오의 '사계절 투자법'입니다. 내일 경기가 좋아질지 나빠질지, 물가가 오를지 내릴지 "
            "아무도 정확히 맞힐 수 없기 때문에 어떤 경제 날씨에서도 잘 버티는 자산 5가지를 정해진 비율로 섞어 두고, 정기적으로 원래 비율로 되돌립니다.",
            _fixed_weights,
            lambda p: 2,
            uses_weights=True,
            default_rebalance="quarterly",
            guide={
                "background": (
                    "날씨를 매번 정확히 맞히기 어렵다면 사계절 옷을 모두 챙겨두는 게 안전합니다. 전설적인 투자자 레이 달리오도 경제에 "
                    "4가지 계절(① 경기 호황 ② 경기 침체 ③ 물가 상승 ④ 물가 하락)이 있다고 보고, 호황에 강한 주식·불황에 강한 국채·물가가 오를 때 강한 금과 원자재를 골고루 담았습니다. "
                    "특히 주식은 국채보다 훨씬 크게 출렁이기 때문에 주식 비중을 30%로 줄이고 국채 비중을 55%로 늘려, 각 자산이 내 계좌를 흔드는 힘(위험)을 비슷하게 맞춘 것이 핵심입니다."
                ),
                "setup": [
                    "기본 비율: 주식 30% · 장기채 40% · 중기채 15% · 금 7.5% · 원자재 7.5% (숫자 합이 100이 아니어도 자동으로 100% 비율로 맞춰집니다).",
                    "리밸런싱(비율 재조정) 주기: '분기마다(3개월에 한 번)'를 추천합니다. 또는 '비중 이탈 ±5%'를 켜두면 특정 자산이 많이 올라 비율이 5% 이상 틀어졌을 때만 자동으로 정리해 줍니다.",
                    "운용 자금: 가장 비율이 작은 7.5% 종목(금·원자재)도 최소 1주 이상 살 수 있을 만큼 여유 있게 넣어주세요. (예: 1주가 3만 원이면 최소 50~100만 원 이상)",
                ],
                "execution": [
                    "정해진 날짜(또는 비율이 크게 틀어진 날) 장 시작 1분 뒤, 현재 내 계좌 금액을 기준으로 각 종목을 몇 주씩 가져야 하는지 계산합니다.",
                    "많이 올라서 목표 비율을 넘긴 종목부터 먼저 팔아 현금을 만듭니다. (자동으로 비쌀 때 일부 이익 실현)",
                    "그 현금으로 덜 올랐거나 떨어진 종목을 시장가로 사서 원래 비율로 되돌립니다. (자동으로 쌀 때 추가 매수)",
                ],
                "caution": "금리가 아주 빠르게 치솟는 특별한 시기(예: 2022년)에는 주식과 장기 국채가 이례적으로 같이 떨어져 방어력이 잠시 약해질 수 있습니다.",
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
            "해리 마코위츠 · 노벨경제학상 분산 투자 이론의 가장 쉬운 정석 (주식 60 : 채권 40)",
            "돈의 60%는 주식, 40%는 채권에 넣어두고 비율이 흐트러질 때마다 60:40으로 다시 맞춥니다",
            "노벨경제학상을 받은 해리 마코위츠의 분산 투자 이론을 가장 간단하게 실천하는 방법입니다. "
            "수익을 키우는 '주식(60%)'과 주식이 떨어질 때 쿠션 역할을 하는 '채권(40%)'을 함께 들고 있으면, "
            "주식만 100% 들었을 때보다 계좌 출렁임을 크게 줄이면서 꾸준히 자산을 불릴 수 있습니다.",
            _fixed_weights,
            lambda p: 2,
            uses_weights=True,
            default_rebalance="quarterly",
            guide={
                "background": (
                    "'계란을 한 바구니에 담지 마라'는 격언을 수학으로 증명해 노벨경제학상을 받은 해리 마코위츠의 이론(현대 포트폴리오 이론)입니다. "
                    "보통 경기가 좋으면 주식이 오르고, 경기가 나빠 주식이 흔들릴 때는 안전한 채권이 버텨주거나 오르는 경향이 있습니다. "
                    "이렇게 움직임이 서로 다른 주식과 채권을 60 대 40으로 섞어 두면, 폭락장에서도 계좌가 반토막 나는 충격을 크게 줄여 끝까지 장기 투자를 버틸 수 있게 해줍니다."
                ),
                "setup": [
                    "기본 비율: 주식 60% + 채권 40% (예: 100만 원이면 주식 60만 원, 채권 40만 원). 더 공격적으로 원하면 70:30, 더 편안하게 원하면 40:60으로 바꿔도 좋습니다.",
                    "리밸런싱(비율 재조정) 주기: 분기마다(3개월) 또는 1년에 한 번이면 충분합니다. 너무 자주 사고팔면 수수료만 늘어납니다.",
                    "종목이 딱 2개뿐이라 소액으로도 시작하기 가장 부담 없는 입문자용 모델입니다.",
                ],
                "execution": [
                    "정해진 리밸런싱 날짜의 장 시작 1분 뒤, 현재 주식과 채권 비율이 60:40에서 얼마나 벗어났는지 확인합니다.",
                    "예를 들어 주식이 많이 올라 70:30이 되었다면, 초과한 주식 10%를 먼저 팔아서 채권을 사 다시 60:40으로 맞춥니다.",
                    "리밸런싱 날이 아닌 평소에는 매매하지 않고 가만히 지켜보며 일일 수익 추이만 기록합니다.",
                ],
                "caution": "주식이 쉬지 않고 계속 오르는 대세 상승장에서는 주식 100% 투자보다 수익률이 낮습니다. 대신 하락장에서 훨씬 덜 잃고 마음 편히 버틸 수 있는 전략입니다.",
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
            "단순함의 힘 · 복잡한 예측 대신 모든 종목을 똑같이 N분의 1씩 나누어 담기",
            "담은 종목이 4개면 25%씩, 5개면 20%씩 똑같이 나누고 정기적으로 다시 N분의 1로 맞춥니다",
            "비율을 복잡하게 계산할 필요 없이, 내가 고른 여러 자산에 돈을 똑같이 나누어 담는 전략입니다. "
            "학계 연구에서도 복잡한 수학 모델보다 단순한 '똑같이 나누기(1/N)'가 실제 투자에서는 더 꾸준한 성과를 낸다는 결과가 나왔습니다. "
            "매달 또는 분기마다 다시 똑같이 맞추면 자동으로 비싼 걸 팔고 싼 걸 사게 됩니다.",
            _equal_weights,
            lambda p: 2,
            default_rebalance="monthly",
            guide={
                "background": (
                    "금융학자 드미겔(DeMiguel) 등이 2009년 발표한 유명한 논문에서, 복잡한 컴퓨터 최적화 모델 14개와 초등학생도 할 수 있는 '똑같이 나누기(1/N)'를 실제 데이터로 대결시켰습니다. "
                    "결과는 놀랍게도 단순한 1/N을 꾸준히 이긴 복잡한 모델이 거의 없었습니다. 과거 데이터로 미래를 너무 빡빡하게 계산하면 오히려 빗나가기 쉽기 때문입니다. "
                    "여기에 주기적으로 다시 N분의 1로 비율을 맞춰주면, 오른 종목은 일부 익절하고 떨어진 종목은 싸게 주워 담는 효과(리밸런싱 보너스)까지 자연스럽게 생깁니다."
                ),
                "setup": [
                    "성격이 서로 다른 자산 3~6개를 고를수록 좋습니다. (예: 미국주식 + 기술주 + 미국국채 + 금처럼 주식과 안전자산을 함께 조합)",
                    "비율을 따로 입력할 필요 없이, 목록에 넣은 종목 수대로 자동으로 똑같이 나뉩니다. (4개면 각 25%)",
                    "리밸런싱 주기는 '매월' 또는 '분기마다'를 추천합니다.",
                ],
                "execution": [
                    "리밸런싱 날 장 시작 1분 뒤, 현재 전체 평가 금액을 종목 수(N)로 똑같이 나눠 각 종목의 목표 수량을 계산합니다.",
                    "그동안 많이 올라서 평균보다 금액이 커진 종목을 먼저 시장가로 팔아 현금을 확보합니다.",
                    "확보된 현금으로 상대적으로 덜 올랐거나 내린 종목을 사서 다시 모두 똑같은 금액으로 맞춥니다.",
                ],
                "caution": "안전한 채권과 변동성이 매우 큰 종목(예: 레버리지 ETF·급등 테마주)을 똑같이 1/N로 섞으면, 크게 출렁이는 종목 하나 때문에 계좌 전체가 요동칠 수 있습니다.",
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
            "에드워드 치엔 · 금액이 아니라 '출렁이는 크기(위험)'를 똑같이 나누기",
            "최근에 많이 출렁인 종목은 적게 담고, 잔잔한 종목은 많이 담아 계좌 흔들림을 고르게 맞춥니다",
            "돈을 똑같이 나누는 게 아니라 '위험(출렁임)'을 똑같이 나누는 전략입니다. 파도가 거센 배에는 짐을 적게 싣고 "
            "잔잔한 배에는 짐을 많이 싣는 것처럼, 최근에 위아래로 크게 흔들리는 자산은 비중을 자동으로 줄이고 안정적인 자산은 비중을 늘려 계좌 전체를 부드럽게 관리합니다.",
            _risk_parity_weights,
            lambda p: int(p["vol_window"]) + 1,
            params=[Param("vol_window", "변동성 기간 (일)", 60, 10, 252)],
            default_rebalance="monthly",
            guide={
                "background": (
                    "투자 전문가 에드워드 치엔이 정리한 '위험 균등 배분(리스크 패리티)' 이론입니다. 예를 들어 주식과 채권에 돈을 50만 원씩 반반 넣더라도, "
                    "주식이 채권보다 4~5배 더 크게 출렁이기 때문에 실제 계좌 손익의 대부분은 주식 혼자 좌우하게 됩니다. "
                    "이 전략은 최근 N일 동안 각 종목이 얼마나 출렁였는지(변동성)를 계산해서, 출렁임이 2배 큰 종목은 비중을 절반으로 줄여 모든 종목이 계좌를 흔드는 힘이 똑같아지도록 자동으로 맞춰줍니다."
                ),
                "setup": [
                    "변동성 측정 기간(기본 60일 = 약 3개월): 최근 3개월간의 출렁임을 보고 비중을 정합니다. 시장 변화에 빠르게 반응하고 싶으면 20~40일, 차분하게 바꾸고 싶으면 60~120일을 씁니다.",
                    "성격과 출렁임이 다른 ETF 3~5개(예: 주식 + 장기채 + 중기채 + 금)를 조합하는 것이 가장 효과가 좋습니다.",
                    "리밸런싱 주기: '매월 초'가 최근 출렁임 변화를 제때 반영하면서도 수수료가 과하지 않아 가장 무난합니다.",
                ],
                "execution": [
                    "매월 리밸런싱 날 장 시작 1분 뒤, 어제까지 최근 60일 동안 각 종목이 하루에 평균 얼마나 출렁였는지를 계산합니다.",
                    "출렁임이 큰 종목일수록 목표 비율을 작게, 잔잔한 종목일수록 목표 비율을 크게 자동 계산합니다. (갑자기 폭락하며 요동친 종목은 자동으로 비중 축소)",
                    "비율이 줄어든 종목을 먼저 시장가로 팔고, 그 현금으로 비율이 늘어난 종목을 시장가로 매수합니다.",
                ],
                "caution": "거의 움직이지 않는 초단기 채권이나 예금성(파킹) ETF를 같이 넣으면, 너무 잔잔하다는 이유로 그 종목에 돈이 지나치게 몰려 수익률이 예금 수준으로 낮아질 수 있습니다.",
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
            "게리 안토나치 · ① 잘나가는 1등 주식 고르기 + ② 분위기 나쁘면 안전 채권으로 대피하기",
            "공격 자산 중 최근 수익률 1등을 고르고, 그마저 마이너스면 안전한 채권으로 100% 피신합니다",
            "미국 투자 전문가 게리 안토나치의 유명한 '듀얼 모멘텀(GEM)' 전략입니다. 두 가지를 확인합니다. "
            "① 여러 주식 시장(예: 미국 vs 선진국) 중 최근 1년간 더 잘 오른 승자를 고르고, ② 그 1등마저 최근 1년 수익률이 마이너스(하락장)라면 "
            "욕심내지 않고 안전한 채권으로 갈아타서 큰 폭락장을 피합니다.",
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
                    "게리 안토나치가 2012년 학술상을 받은 논문과 책으로 널리 알려진 전략입니다. 핵심은 두 번의 거름망(듀얼)입니다. "
                    "첫째(상대 비교): 미국 주식과 해외(또는 국내) 주식 중 요즘 더 힘 있게 오르는 '달리기 1등'을 골라 수익을 챙깁니다. "
                    "둘째(절대 비교): 아무리 1등이라도 최근 1년간 수익률이 0% 아래(마이너스)라면 글로벌 경제 전체에 비가 내리고 있다는 뜻이므로, "
                    "주식에서 완전히 나와 안전한 채권 우산 아래로 대피합니다. 덕분에 2008년 금융위기 같은 큰 폭락장을 피하는 힘이 뛰어납니다."
                ),
                "setup": [
                    "종목 역할 지정(필수): 목록에서 '공격(주식 ETF 2개 이상)'과 '방어(안전 채권 ETF 1개 이상)' 역할을 꼭 나눠서 지정해 주세요.",
                    "수익률 확인 기간(기본 252일 = 1년): 원작 기본값은 1년(252거래일)입니다. 시장 전환을 더 빨리 눈치채고 싶으면 6개월(126일)을 쓰기도 합니다.",
                    "상위 선택 개수(기본 1개): 1등에 집중 투자하려면 1개, 공격 종목이 3~4개 이상이라 나누어 담고 싶으면 2개로 설정하세요.",
                    "리밸런싱 주기: '매월 초'에 딱 한 번만 확인하고 갈아타는 것이 정석입니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 목록에 있는 모든 종목의 최근 1년(설정한 기간) 수익률을 계산합니다.",
                    "공격 종목 중 수익률이 가장 높은 종목을 고릅니다. 그 종목의 수익률이 0%보다 높으면 그 종목에 투자하고, 0% 이하면 방어(채권) 종목으로 100% 갈아탑니다.",
                    "들고 있던 종목과 새로 뽑힌 종목이 다르면, 기존 종목을 먼저 전량 시장가로 판 뒤 체결을 확인하고 새 종목을 시장가로 삽니다.",
                ],
                "caution": "시장이 갑자기 폭락했다가 며칠 만에 V자로 급반등할 때는 채권으로 대피했다가 다시 주식으로 돌아오는 타이밍이 한 달 늦어져 단기 손해가 날 수 있습니다.",
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
            "켈러 & 큐닝 · '광산의 카나리아'처럼 위험 신호가 하나라도 울리면 즉시 안전지대로 대피",
            "공격 자산 4개가 모두 상승세일 때만 투자하고, 1개라도 꺾이면 즉시 안전 채권으로 100% 대피합니다",
            "옛날 탄광에서 유독가스를 가장 먼저 감지하던 카나리아 새처럼, 시장의 위험 신호를 재빨리 알아채고 피하는 초고속 방어 전략입니다. "
            "공격 자산 4개의 최근 흐름을 점수화해서 모두 플러스(+)일 때만 1등 공격 자산에 투자하고, 단 1개라도 마이너스(-)로 꺾이면 즉시 가장 안전한 채권으로 전액 피신합니다.",
            _vaa_weights,
            lambda p: 253,
            uses_roles=True,
            params=[Param("top_offensive", "공격 상위 종목 수", 1, 1, 4)],
            default_rebalance="monthly",
            guide={
                "background": (
                    "네덜란드 금융 연구자 켈러와 큐닝(2017)이 만든 '경계(Vigilant) 자산배분' 이론입니다. 탄광에 데려간 카나리아가 쓰러지면 즉시 대피하듯, "
                    "미국·선진국·신흥국·채권 등 4개의 공격 자산 중 '단 하나라도 하락세'로 돌아서면 전 세계 시장 어딘가에 균열이 생겼다는 경보로 봅니다. "
                    "특히 1년 전보다 '최근 1개월·3개월 성적'에 훨씬 높은 점수를 주는 공식(13612W)을 써서, 폭락장이 시작되자마자 남들보다 빠르게 안전 채권으로 도망쳐 계좌 손실 폭을 크게 줄이는 것이 목표입니다."
                ),
                "setup": [
                    "공격 자산(4개 추천): 기본 프리셋처럼 미국주식(SPY)·선진국(VEA)·신흥국(VWO)·종합채권(AGG)처럼 서로 다른 시장을 골고루 넣어 감시망을 만듭니다.",
                    "방어 자산(2~3개 추천): 위기 때 도망갈 안전 피난처로 단기채(SHY)·중기채(IEF)·회사채(LQD) 등을 넣어두면, 그중 요즘 가장 잘 버티는 1곳으로 자동 대피합니다.",
                    "공격 상위 종목 수: 원작처럼 1등에 집중하려면 1, 조금 나누어 담으려면 2로 설정하세요.",
                    "리밸런싱 주기: '매월 초' 고정 추천. 1년 수익률을 계산해야 하므로 상장한 지 1년 넘은 ETF만 넣어야 합니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 모든 종목의 최근 1·3·6·12개월 수익률을 합쳐 '최근 흐름 점수(13612W)'를 계산합니다. (최근 1개월 성적이 가장 중요하게 반영됨)",
                    "공격 자산 '전부'가 플러스(+) 점수일 때만, 그중 점수가 가장 높은 공격 종목에 투자합니다.",
                    "공격 자산 중 '단 1개라도' 0 이하(마이너스)이면 즉시 공격 종목을 전부 팔고, 방어 채권 중 점수가 가장 높은 1종목에 100% 투자합니다.",
                ],
                "caution": "위험 신호에 매우 민감해서 공격(주식)과 방어(채권) 사이를 100% 통째로 갈아타는 일이 잦습니다. 수수료가 낮고 거래량이 풍부한 대표 ETF로 운용하는 것이 유리합니다.",
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
