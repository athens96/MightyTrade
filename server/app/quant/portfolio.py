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
MAX_ASSETS = 16
VALID_ROLES = ("offensive", "defensive", "canary", "core")


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
    off_scores = [(_vaa_score(closes[s], i), s) for s in off]
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


def _sma12_ratio(cs: list[float], i: int) -> float:
    """Keller & Keuning SMA12 ratio: current close / average of current + last 12 monthly closes - 1."""
    if i < 252:
        return -math.inf
    pts = [cs[i - k * 21] for k in range(13)]
    avg = sum(pts) / len(pts)
    return cs[i] / avg - 1.0 if avg > 0 else -math.inf


def _daa_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    top_off = max(1, int(params["top_offensive"]))
    threshold = max(1, int(params["canary_threshold"]))
    canaries = [a["symbol"] for a in assets if a.get("role") == "canary"]
    off = [a["symbol"] for a in assets if a.get("role") == "offensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]
    if not off:
        off = [a["symbol"] for a in assets if a.get("role") != "defensive"]
    if not canaries:
        canaries = off

    bad = sum(1 for s in canaries if _vaa_score(closes[s], i) <= 0)
    b_cap = min(threshold, max(1, len(canaries)))
    protect = min(1.0, bad / b_cap)

    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    if protect > 0 and dfn:
        best_def = max((_vaa_score(closes[s], i), s) for s in dfn)[1]
        out[best_def] = protect
    off_w = 1.0 - (protect if dfn else 0.0)
    if off_w > 0 and off:
        picks = sorted(((_vaa_score(closes[s], i), s) for s in off), reverse=True)[: min(top_off, len(off))]
        each = off_w / len(picks)
        for _, sym in picks:
            out[sym] = out.get(sym, 0.0) + each
    return out


def _baa_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    top_off = max(1, int(params["top_offensive"]))
    top_def = max(1, int(params["top_defensive"]))
    canaries = [a["symbol"] for a in assets if a.get("role") == "canary"]
    off = [a["symbol"] for a in assets if a.get("role") == "offensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]
    if not canaries:
        canaries = off

    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    if canaries and all(_vaa_score(closes[s], i) > 0 for s in canaries) and off:
        picks = sorted(((_sma12_ratio(closes[s], i), s) for s in off), reverse=True)[: min(top_off, len(off))]
        each = 1.0 / len(picks)
        for _, sym in picks:
            out[sym] = each
    elif dfn:
        ranked_def = sorted(((_sma12_ratio(closes[s], i), s) for s in dfn), reverse=True)
        picks = ranked_def[: min(top_def, len(ranked_def))]
        # Best defensive asset acts as cash-safe fallback when another defensive pick is below its 12m avg
        safest = ranked_def[0][1]
        each = 1.0 / len(picks)
        for ratio, sym in picks:
            target = sym if ratio >= 0 else safest
            out[target] = out.get(target, 0.0) + each
    return out


def _laa_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    trend_days = max(20, int(params["trend_days"]))
    cores = [a for a in assets if a.get("role") == "core"]
    off = [a["symbol"] for a in assets if a.get("role") == "offensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]

    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    core_sum = sum(max(float(a.get("weight") or 0.0), 0.0) for a in cores)
    if core_sum >= 100.0 and cores:
        for a in cores:
            out[a["symbol"]] = max(float(a.get("weight") or 0.0), 0.0) / core_sum
        return out

    for a in cores:
        out[a["symbol"]] = max(float(a.get("weight") or 0.0), 0.0) / 100.0
    timing_w = max(0.0, 1.0 - core_sum / 100.0)
    if timing_w <= 0:
        return out

    # Use first core asset (typically SPY / KOSPI200) as broad market trend gauge
    gauge = cores[0]["symbol"] if cores else (off[0] if off else assets[0]["symbol"])
    cs = closes[gauge]
    if i >= trend_days:
        sma = sum(cs[i - trend_days + 1 : i + 1]) / trend_days
        bull = cs[i] > sma
    else:
        bull = True

    sleeve = off if (bull and off) else (dfn or off)
    if sleeve:
        each = timing_w / len(sleeve)
        for s in sleeve:
            out[s] = out.get(s, 0.0) + each
    return out


def _hrp_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    """Pure-Python Hierarchical Risk Parity (López de Prado) with tree-aligned recursive bisection."""
    syms = [a["symbol"] for a in assets]
    n = len(syms)
    if n == 1:
        return {syms[0]: 1.0}
    w = max(10, int(params["vol_window"]))
    start = max(1, i - w + 1)

    rets: list[list[float]] = []
    for s in syms:
        cs = closes[s]
        rets.append([cs[k] / cs[k - 1] - 1.0 if cs[k - 1] > 0 else 0.0 for k in range(start, i + 1)])
    m = len(rets[0])
    if m < 3:
        return {s: 1.0 / n for s in syms}

    means = [sum(r) / m for r in rets]
    cov = [[0.0] * n for _ in range(n)]
    for a in range(n):
        for b in range(a, n):
            v = sum((rets[a][t] - means[a]) * (rets[b][t] - means[b]) for t in range(m)) / (m - 1)
            if a == b:
                v = max(v, 1e-8)
            cov[a][b] = cov[b][a] = v

    dist = [[0.0] * n for _ in range(n)]
    for a in range(n):
        for b in range(a + 1, n):
            corr = max(-1.0, min(1.0, cov[a][b] / math.sqrt(cov[a][a] * cov[b][b])))
            d = math.sqrt(max(0.0, 0.5 * (1.0 - corr)))
            dist[a][b] = dist[b][a] = d

    # Single-linkage agglomerative clustering storing binary merge tree
    nodes: list[dict[str, Any]] = [{"leaves": [k], "left": None, "right": None} for k in range(n)]
    while len(nodes) > 1:
        best_d = math.inf
        bi, bj = 0, 1
        for a in range(len(nodes)):
            for b in range(a + 1, len(nodes)):
                d = min(dist[u][v] for u in nodes[a]["leaves"] for v in nodes[b]["leaves"])
                if d < best_d:
                    best_d, bi, bj = d, a, b
        merged = {
            "leaves": nodes[bi]["leaves"] + nodes[bj]["leaves"],
            "left": nodes[bi],
            "right": nodes[bj],
        }
        nodes.pop(bj)
        nodes[bi] = merged

    def cluster_var(idxs: list[int]) -> float:
        iv = [1.0 / cov[k][k] for k in idxs]
        s_iv = sum(iv) or 1.0
        cw = [x / s_iv for x in iv]
        total = 0.0
        for p, kp in enumerate(idxs):
            for q, kq in enumerate(idxs):
                total += cw[p] * cw[q] * cov[kp][kq]
        return max(total, 1e-10)

    weights = [1.0] * n

    def bisect(node: dict[str, Any]) -> None:
        if node["left"] is None or node["right"] is None:
            return
        l_leaves = node["left"]["leaves"]
        r_leaves = node["right"]["leaves"]
        vl, vr = cluster_var(l_leaves), cluster_var(r_leaves)
        alpha = 1.0 - vl / (vl + vr) if (vl + vr) > 0 else 0.5
        for k in l_leaves:
            weights[k] *= alpha
        for k in r_leaves:
            weights[k] *= 1.0 - alpha
        bisect(node["left"])
        bisect(node["right"])

    bisect(nodes[0])
    total_w = sum(weights) or 1.0
    return {syms[k]: weights[k] / total_w for k in range(n)}


def _sector_rotation_weights(closes: dict[str, list[float]], i: int, assets: list[dict], params: dict) -> dict[str, float]:
    lb = max(20, int(params["lookback"]))
    ma_days = max(20, int(params["ma_days"]))
    top_n = max(1, int(params["top_n"]))
    off = [a["symbol"] for a in assets if a.get("role", "offensive") != "defensive"]
    dfn = [a["symbol"] for a in assets if a.get("role") == "defensive"]
    if not off:
        off = [a["symbol"] for a in assets]

    def ret_of(sym: str) -> float:
        cs = closes[sym]
        return cs[i] / cs[i - lb] - 1.0 if i >= lb and cs[i - lb] > 0 else -math.inf

    def risk_adj_score(sym: str) -> float:
        r = ret_of(sym)
        if not math.isfinite(r):
            return -math.inf
        cs = closes[sym]
        vw = min(60, lb)
        start = max(1, i - vw + 1)
        rs = [cs[k] / cs[k - 1] - 1.0 for k in range(start, i + 1) if cs[k - 1] > 0]
        if len(rs) < 2:
            return r
        mu = sum(rs) / len(rs)
        vol = math.sqrt(sum((x - mu) ** 2 for x in rs) / (len(rs) - 1)) * math.sqrt(252)
        return r / max(vol, 0.05)

    def above_ma(sym: str) -> bool:
        cs = closes[sym]
        if i < ma_days:
            return True
        return cs[i] > sum(cs[i - ma_days + 1 : i + 1]) / ma_days

    ranked_off = sorted(((risk_adj_score(s), s) for s in off), reverse=True)
    best_def = max(((ret_of(s), s) for s in dfn), default=(0.0, None))[1] if dfn else None
    picks = ranked_off[: min(top_n, len(ranked_off))]
    slice_w = 1.0 / len(picks) if picks else 0.0
    out: dict[str, float] = {a["symbol"]: 0.0 for a in assets}
    for _, sym in picks:
        if ret_of(sym) > 0 and above_ma(sym):
            out[sym] = out.get(sym, 0.0) + slice_w
        elif best_def is not None:
            out[best_def] = out.get(best_def, 0.0) + slice_w
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
        allowed_roles: tuple[str, ...] = ("offensive", "defensive"),
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
        self.allowed_roles = allowed_roles
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
            "allowed_roles": list(self.allowed_roles),
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
        PortfolioModel(
            "daa",
            "DAA 방어적 자산배분",
            "켈러 & 큐닝 · 감시병 2명의 건강 상태에 따라 0% → 50% → 100% 단계별 대피",
            "감시병(신흥국·채권)이 1개 꺾이면 절반만 대피, 2개 다 꺾이면 안전 채권으로 100% 피신합니다",
            "VAA의 업그레이드 버전입니다. VAA가 0% 아니면 100%로 극단적으로 갈아타서 잦은 매매가 생기는 단점을 고쳤습니다. "
            "따로 정한 '감시병(카나리아) 2종'만 지켜보다가 1종이 아프면 돈의 절반(50%)만 안전 채권으로 옮기고, 2종 다 아프면 100% 대피해 훨씬 부드럽게 움직입니다.",
            _daa_weights,
            lambda p: 253,
            uses_roles=True,
            allowed_roles=("offensive", "defensive", "canary"),
            params=[
                Param("top_offensive", "공격 상위 종목 수", 2, 1, 6),
                Param("canary_threshold", "100% 대피 감시병 수", 2, 1, 4),
            ],
            default_rebalance="monthly",
            guide={
                "background": (
                    "VAA를 만든 켈러와 큐닝이 2018년에 발표한 개량형 전략(Defensive Asset Allocation)입니다. "
                    "VAA는 공격 자산 중 하나만 꺾여도 100% 통째로 갈아타기 때문에 시장이 잠깐 흔들릴 때 헛걸음(수수료·손실)이 잦았습니다. "
                    "DAA는 실제 투자할 종목과 '경보용 감시병(신흥국 주식·미국 종합채권)'을 분리하고, 감시병이 1개 꺾이면 50%만 대피, 2개 다 꺾이면 100% 대피하는 "
                    "'단계적 방어' 방식을 도입해 훨씬 안정적으로 수익을 지키도록 만들었습니다."
                ),
                "setup": [
                    "감시병(canary): 기본 프리셋처럼 경기 변화에 민감한 신흥국(VWO)과 미국 채권(BND) 2종을 두는 것이 원작 공식입니다.",
                    "공격(offensive): 미국주식·나스닥·소형주·선진국·금·리츠 등 4~8개 ETF를 넣어두면, 안전한 달에 상위 2종(top_offensive)을 자동으로 뽑아 투자합니다.",
                    "방어(defensive): 위기 때 피신할 단기채·중기채·장기채 중 요즘 가장 잘 버티는 1종으로 자동 대피합니다.",
                    "100% 대피 감시병 수(기본 2): 감시병이 2개일 때 2로 두면 '0개=공격 100%, 1개=반반(50:50), 2개=방어 100%'로 작동합니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 감시병 종목들의 최근 흐름 점수(1·3·6·12개월 가중 점수)를 확인합니다.",
                    "감시병이 모두 플러스(+)면 공격 자산 중 1~2등에 100% 투자합니다.",
                    "감시병 중 1개만 마이너스(-)면 50%는 공격 상위 종목, 50%는 방어 1등 채권에 나누어 담습니다.",
                    "감시병이 2개 이상 마이너스면 공격 종목을 모두 팔고 방어 1등 채권으로 100% 대피합니다.",
                ],
                "caution": "감시병 종목 자체는 신호 확인용이라 매수하지 않습니다. 최소 1년 이상 상장된 ETF만 목록에 넣어야 점수가 정확히 계산됩니다.",
            },
            presets={
                "US": [
                    {"symbol": "VWO", "name": "신흥국 주식 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "BND", "name": "미국 종합채권 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 0},
                    {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                    {"symbol": "IWM", "name": "미국 소형주", "role": "offensive", "weight": 0},
                    {"symbol": "VEA", "name": "선진국 주식", "role": "offensive", "weight": 0},
                    {"symbol": "GLD", "name": "금", "role": "offensive", "weight": 0},
                    {"symbol": "VNQ", "name": "미국 리츠", "role": "offensive", "weight": 0},
                    {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "TLT", "name": "장기국채 (방어)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "360750", "name": "미국 S&P500 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "148070", "name": "국고채10년 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 0},
                    {"symbol": "133690", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                    {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 0},
                    {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
                    {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0},
                ],
            },
        ),
        PortfolioModel(
            "baa",
            "BAA 대담한 자산배분",
            "켈러 & 큐닝 (2022) · 금리 급등기에도 단기채·원자재로 피하는 최신 방어 모델",
            "감시병 4종이 모두 좋을 때만 공격 투자, 1개라도 꺾이면 1년 평균보다 강한 안전자산 3곳으로 대피합니다",
            "2022년처럼 금리가 급등해 주식과 장기채가 동시에 떨어지는 위기까지 대비해 만들어진 최신 인기 모델(Bold Asset Allocation)입니다. "
            "감시병 4종 중 하나라도 경고등이 켜지면, 안전자산 후보(초단기채·물가연동채·원자재·중기채 등) 중 최근 1년 평균보다 강한 3종목으로 분산 대피합니다.",
            _baa_weights,
            lambda p: 253,
            uses_roles=True,
            allowed_roles=("offensive", "defensive", "canary"),
            params=[
                Param("top_offensive", "공격 상위 종목 수", 1, 1, 4),
                Param("top_defensive", "방어 분산 종목 수", 3, 1, 5),
            ],
            default_rebalance="monthly",
            guide={
                "background": (
                    "켈러와 큐닝이 2022년 발표한 최신 동적 자산배분(BAA)입니다. 기존 올웨더나 60/40이 '금리 급등 + 물가 상승' 시기에 채권마저 떨어지며 흔들렸던 약점을 해결했습니다. "
                    "① 감시병 4종(미국·선진국·신흥국·종합채권)으로 위험을 빠르게 감지하고, ② 종목을 고를 때는 '현재 가격 ÷ 지난 12개월 평균 가격' 공식을 씁니다. "
                    "③ 특히 위기 때 도망갈 방어 자산에 초단기채(현금성)·물가연동채·원자재까지 포함해 3곳에 나누어 담고, 그중에서도 1년 평균 아래로 빠진 채권은 가장 강한 안전자산으로 대체해 방어력을 극대화합니다."
                ),
                "setup": [
                    "감시병(canary): 미국(SPY)·선진국(VEA)·신흥국(VWO)·미국채권(BND) 4종이 원작 기준입니다.",
                    "공격(offensive): 나스닥(QQQ)·미국전체(VTI)·해외주식(VXUS)·리츠(VNQ) 등 성장력이 강한 ETF를 넣습니다.",
                    "방어(defensive): 초단기채(BIL)·물가채(TIP)·원자재(DBC)·중기채(IEF)·장기채(TLT)·회사채(LQD)처럼 성격이 다른 안전자산 4~6종을 넣으세요.",
                    "공격 상위 종목 수: 공격형(G4)은 1개 집중, 균형형(G12)은 2~3개 분산을 권장합니다. 방어 분산은 3개가 원작 기본값입니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 감시병 4종의 최근 흐름 점수(13612W)를 먼저 검사합니다.",
                    "감시병 4종이 모두 플러스(+)면 공격 자산 중 '현재가 ÷ 12개월 평균가'가 가장 높은 종목에 투자합니다.",
                    "감시병 중 1개라도 0 이하면 공격 종목을 전량 매도하고, 방어 자산 상위 3종에 1/3씩 나누어 투자합니다. (단, 12개월 평균보다 떨어진 방어 종목 몫은 1등 안전자산으로 몰아줍니다.)",
                ],
                "caution": "방어 자산에 단기채·원자재까지 포함되어 있어 위기 방어력은 뛰어나지만, 상승장이 갑자기 시작될 때는 한 달 늦게 올라탈 수 있습니다.",
            },
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국주식 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "VEA", "name": "선진국 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "VWO", "name": "신흥국 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "BND", "name": "미국채권 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                    {"symbol": "VTI", "name": "미국 전체주식", "role": "offensive", "weight": 0},
                    {"symbol": "VXUS", "name": "글로벌 해외주식", "role": "offensive", "weight": 0},
                    {"symbol": "VNQ", "name": "미국 리츠", "role": "offensive", "weight": 0},
                    {"symbol": "BIL", "name": "초단기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "TIP", "name": "물가연동채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "DBC", "name": "원자재 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "IEF", "name": "중기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "TLT", "name": "장기국채 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "LQD", "name": "회사채 (방어)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "360750", "name": "미국 S&P500 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "148070", "name": "국고채10년 (감시병)", "role": "canary", "weight": 0},
                    {"symbol": "133690", "name": "미국 나스닥100", "role": "offensive", "weight": 0},
                    {"symbol": "229200", "name": "코스닥150", "role": "offensive", "weight": 0},
                    {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
                    {"symbol": "153130", "name": "단기채권 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "114260", "name": "국고채3년 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "261220", "name": "WTI원유 (방어)", "role": "defensive", "weight": 0},
                    {"symbol": "305080", "name": "미국채10년선물 (방어)", "role": "defensive", "weight": 0},
                ],
            },
        ),
        PortfolioModel(
            "laa",
            "LAA 게으른 자산배분",
            "켈러 & 브룩스 · 75%는 가만히 두고, 25%만 시장 분위기에 따라 갈아타기",
            "75%(주식·채권·금 각 25%)는 고정하고, 나머지 25%만 상승장엔 나스닥·하락장엔 단기채로 바꿉니다",
            "매달 전부 갈아타는 게 번거롭고 수수료가 아까운 분들을 위한 '게으른 자산배분(Lethargic Asset Allocation)'입니다. "
            "전체 돈의 75%는 든든한 기본 자산(미국주식·채권·금)에 편하게 묶어두고, 딱 25%만 시장이 200일 평균선 위일 때 나스닥(공격), 아래일 때 단기채(방어)로 옮깁니다.",
            _laa_weights,
            lambda p: int(p["trend_days"]) + 1,
            uses_roles=True,
            allowed_roles=("core", "offensive", "defensive"),
            uses_weights=True,
            params=[Param("trend_days", "시장 추세 기준선 (일)", 200, 50, 300)],
            default_rebalance="monthly",
            guide={
                "background": (
                    "켈러와 브룩스가 제안한 '게으른 자산배분(LAA)' 전략입니다. 이름처럼 '최소한의 매매로 최대한 편하게' 장기 수익을 내는 게 목표입니다. "
                    "자산의 75%는 어떤 경제 상황에서도 균형을 잡는 영구 고정 자산(미국 대형주 25% + 중기국채 25% + 금 25%)에 두어 계좌를 단단히 받치고, "
                    "남은 25%만 대표 지수(첫 번째 고정 자산)가 장기 추세선(200일 평균선) 위에 있으면 성장주(나스닥), 아래에 있으면 안전한 단기채(현금성)로 바꿔 추가 수익과 하락장 방어를 동시에 챙깁니다."
                ),
                "setup": [
                    "고정 자산(core): 기본 3종(주식 25% + 국채 25% + 금 25% = 합계 75%). 첫 번째 고정 자산이 '상승장/하락장'을 판별하는 기준 지수 역할을 합니다.",
                    "공격(offensive) · 방어(defensive): 비중 칸은 0으로 두세요. 고정 자산 합계(75%)를 뺀 나머지 25%가 상승장엔 공격, 하락장엔 방어로 자동 배정됩니다.",
                    "시장 추세 기준선(기본 200일): 1년 가까이 되는 200일 평균선이 가장 검증된 표준값입니다.",
                    "리밸런싱 주기: 매월 초 확인을 권장합니다. 실제로 바뀌는 달이 1년에 1~2번뿐이라 매매가 매우 적습니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 첫 번째 고정 자산(예: S&P500) 가격이 최근 200일 평균선 위인지 아래인지 확인합니다.",
                    "200일선 위(상승장)면 '고정 자산 75% + 공격(나스닥) 25%'를 목표 비율로 잡습니다.",
                    "200일선 아래(하락장)면 공격 종목을 팔아 '고정 자산 75% + 방어(단기채) 25%'로 바꿉니다.",
                ],
                "caution": "75%가 고정 자산이라 하락장에서도 방어가 되지만, 200일선 근처에서 주가가 위아래로 살짝씩 넘나들 때는 25% 몫이 몇 달 연속 바뀔 수 있습니다.",
            },
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 S&P500 (고정·기준)", "role": "core", "weight": 25},
                    {"symbol": "IEF", "name": "미국 중기국채 (고정)", "role": "core", "weight": 25},
                    {"symbol": "GLD", "name": "금 (고정)", "role": "core", "weight": 25},
                    {"symbol": "QQQ", "name": "나스닥100 (상승장 25%)", "role": "offensive", "weight": 0},
                    {"symbol": "SHY", "name": "단기국채 (하락장 25%)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "360750", "name": "미국 S&P500 (고정·기준)", "role": "core", "weight": 25},
                    {"symbol": "148070", "name": "국고채10년 (고정)", "role": "core", "weight": 25},
                    {"symbol": "132030", "name": "골드선물 (고정)", "role": "core", "weight": 25},
                    {"symbol": "133690", "name": "미국 나스닥100 (상승장 25%)", "role": "offensive", "weight": 0},
                    {"symbol": "153130", "name": "단기채권 (하락장 25%)", "role": "defensive", "weight": 0},
                ],
            },
        ),
        PortfolioModel(
            "permanent",
            "영구 포트폴리오 (25×4)",
            "해리 브라운 · 주식·장기채·금·현금(단기채) 딱 4등분으로 어떤 위기에도 버티기",
            "주식 25% · 장기국채 25% · 금 25% · 초단기채(현금성) 25%를 똑같이 나누어 유지합니다",
            "1980년대 투자 저술가 해리 브라운이 만든 가장 단순하고 마음 편한 방어형 분산 투자입니다. "
            "호황엔 주식, 불황엔 장기채, 물가 폭등엔 금, 현금 부족·금리 급등엔 초단기채가 각각 계좌를 지켜주도록 정확히 25%씩 4등분합니다.",
            _fixed_weights,
            lambda p: 2,
            uses_weights=True,
            default_rebalance="yearly",
            guide={
                "background": (
                    "해리 브라운(Harry Browne)이 '평생 신경 쓰지 않고 들고 갈 수 있는 포트폴리오'로 고안한 영구 포트폴리오입니다. "
                    "올웨더와 비슷한 경제 4계절 원리이지만 훨씬 단순합니다. ① 경기 호황 → 주식(25%), ② 경기 침체 → 장기국채(25%), "
                    "③ 물가 상승·화폐 가치 하락 → 금(25%), ④ 금리 급등·현금 경색 → 초단기채/현금(25%). "
                    "특히 안전 현금성 자산이 25%나 들어 있어, 주식 폭락장에서도 체감 손실이 매우 작습니다."
                ),
                "setup": [
                    "기본 비율: 주식 25% · 장기채 25% · 금 25% · 단기채(현금성) 25% 딱 4등분 그대로 쓰시면 됩니다.",
                    "리밸런싱 주기: '매년 초(1년에 1번)' 또는 '비중 이탈 ±10%(한 종목이 35% 넘거나 15% 밑으로 떨어질 때)'가 해리 브라운 원작 규칙입니다.",
                    "4종목 모두 25%씩 똑같아서 소액으로도 비율 맞추기가 아주 깔끔합니다.",
                ],
                "execution": [
                    "1년에 한 번(또는 비율이 크게 틀어진 날) 장 시작 1분 뒤 4개 자산의 현재 비율을 확인합니다.",
                    "많이 올라서 25%를 넘긴 자산을 먼저 일부 팔아 이익을 챙깁니다.",
                    "그 현금으로 25%보다 줄어든 자산을 싸게 사서 다시 정확히 25%씩 4등분으로 되돌립니다.",
                ],
                "caution": "현금성 단기채와 금이 절반(50%)을 차지하므로, 주식 대세 상승장에서는 수익률이 다소 답답하게 느껴질 수 있습니다. '크게 잃지 않는 꾸준함'이 목표인 분께 추천합니다.",
            },
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 25},
                    {"symbol": "TLT", "name": "미국 장기국채", "role": "defensive", "weight": 25},
                    {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 25},
                    {"symbol": "SHY", "name": "초단기국채 (현금성)", "role": "defensive", "weight": 25},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 25},
                    {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 25},
                    {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 25},
                    {"symbol": "153130", "name": "단기채권 (현금성)", "role": "defensive", "weight": 25},
                ],
            },
        ),
        PortfolioModel(
            "hrp",
            "HRP 계층적 위험 분산",
            "로페스 데 프라도 (2016) · 비슷하게 움직이는 종목끼리 묶어서 중복 위험 차단",
            "출렁임 크기뿐 아니라 '같이 떨어지는 성질'까지 계산해 비슷한 종목에 돈이 쏠리지 않게 자동 배분합니다",
            "기존 리스크 패리티에서 한 단계 더 진화한 최신 분산 투자 이론(Hierarchical Risk Parity)입니다. "
            "예를 들어 S&P500과 나스닥100은 거의 쌍둥이처럼 같이 움직이는데, 이 전략은 둘을 자동으로 한 묶음(그룹)으로 인식해 주식에 위험이 두 배로 쏠리는 실수를 막아줍니다.",
            _hrp_weights,
            lambda p: int(p["vol_window"]) + 1,
            params=[Param("vol_window", "상관·변동성 기간 (일)", 60, 20, 252)],
            default_rebalance="monthly",
            guide={
                "background": (
                    "세계적 퀀트 석학 마르코스 로페스 데 프라도(Marcos López de Prado)가 2016년 발표한 계층적 위험 분산(HRP) 이론입니다. "
                    "보통 분산 투자를 할 때 S&P500·나스닥100·채권을 넣으면 주식 ETF가 2개라서 자칫 주식 쪽에 돈이 과하게 몰리기 쉽습니다. "
                    "HRP는 최근 가격 움직임을 보고 '서로 비슷하게 오르내리는 종목끼리' 먼저 그룹(트리)으로 묶은 뒤, 그룹끼리 위험을 똑같이 나누고 "
                    "그 안에서 다시 종목별로 나누어 주기 때문에 진정한 의미의 균형 분산이 이루어집니다."
                ),
                "setup": [
                    "상관·변동성 측정 기간(기본 60일 = 약 3개월): 최근 3개월 동안 종목들이 서로 얼마나 비슷하게 움직이고 출렁였는지 분석합니다.",
                    "주식(2종) + 채권(2종) + 금처럼 성격이 겹치는 종목과 다른 종목이 섞여 있을 때 가장 빛을 발합니다.",
                    "비율을 직접 입력할 필요 없이 매월 초 자동으로 가장 안전한 균형 비율을 계산해 줍니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 최근 60일 수익률로 종목 간 '비슷하게 움직이는 정도(상관관계)'를 계산합니다.",
                    "가장 비슷하게 움직이는 종목끼리 차례대로 묶어 나무(계층) 구조를 만든 뒤, 위에서 아래로 내려오며 출렁임이 큰 그룹은 적게·잔잔한 그룹은 많이 비율을 배정합니다.",
                    "계산된 목표 비율보다 많은 종목을 먼저 시장가로 팔고, 부족한 종목을 시장가로 사서 균형을 맞춥니다.",
                ],
                "caution": "리스크 패리티와 마찬가지로, 거의 안 움직이는 초단기 예금성 ETF를 함께 넣으면 그쪽으로 비중이 과하게 쏠릴 수 있습니다.",
            },
            presets={
                "US": [
                    {"symbol": "SPY", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                    {"symbol": "QQQ", "name": "미국 나스닥100", "role": "offensive", "weight": 20},
                    {"symbol": "TLT", "name": "장기국채", "role": "defensive", "weight": 20},
                    {"symbol": "IEF", "name": "중기국채", "role": "defensive", "weight": 20},
                    {"symbol": "GLD", "name": "금", "role": "defensive", "weight": 20},
                ],
                "KR": [
                    {"symbol": "069500", "name": "KODEX 200", "role": "offensive", "weight": 20},
                    {"symbol": "360750", "name": "미국 S&P500", "role": "offensive", "weight": 20},
                    {"symbol": "148070", "name": "국고채10년", "role": "defensive", "weight": 20},
                    {"symbol": "114260", "name": "국고채3년", "role": "defensive", "weight": 20},
                    {"symbol": "132030", "name": "골드선물", "role": "defensive", "weight": 20},
                ],
            },
        ),
        PortfolioModel(
            "sector_rotation",
            "섹터·종목 모멘텀 로테이션",
            "메브 파버 · 요즘 가장 잘나가는 업종(종목) TOP 3로 매달 자동 갈아타기",
            "여러 섹터·우량주 중 '출렁임 대비 가장 잘 오른 상위 N개'를 고르고, 200일선 아래로 꺾인 몫은 안전 채권으로 대피합니다",
            "주식 시장은 시기마다 주도하는 업종(반도체·에너지·금융·헬스케어 등)이 계속 바뀝니다. "
            "이 전략은 매달 여러 섹터 ETF나 우량주 후보군을 채점해서 '흔들림이 적으면서 가장 힘 있게 오르는 상위 3개'로 자동 갈아타고, 상승 추세가 깨진 자리는 단기채에 안전하게 맡겨둡니다.",
            _sector_rotation_weights,
            lambda p: max(int(p["lookback"]), int(p["ma_days"])) + 1,
            uses_roles=True,
            allowed_roles=("offensive", "defensive"),
            params=[
                Param("lookback", "수익률 비교 기간 (일)", 126, 20, 252),
                Param("ma_days", "상승 추세 확인선 (일)", 200, 20, 252),
                Param("top_n", "상위 선택 종목 수", 3, 1, 6),
            ],
            default_rebalance="monthly",
            guide={
                "background": (
                    "유명 펀드매니저 메브 파버(Meb Faber)의 업종 순환(섹터 로테이션) 이론과 학계의 모멘텀 연구를 결합한 자동 종목 교체 전략입니다. "
                    "경제 상황에 따라 어떤 해는 기술주, 어떤 해는 에너지·금융·필수소비재가 시장을 이끕니다. "
                    "단순히 많이 오른 종목만 고르면 롤러코스터 종목이 뽑힐 수 있어, '수익률 ÷ 출렁임(안정성 점수)'으로 부드럽게 잘 오르는 주도주 TOP 3를 뽑고, "
                    "200일 평균선 아래로 꺾인 종목이 있다면 그 몫만큼 안전 채권으로 대피시켜 하락장 손실을 막습니다."
                ),
                "setup": [
                    "공격(offensive): 기본 프리셋의 미국 8대 업종 ETF(기술·헬스케어·금융·에너지 등)나, [종목 발굴] 탭에서 뽑은 유망 우량주 5~10개를 넣어 자동 교체 후보군으로 쓰세요.",
                    "방어(defensive): 장이 안 좋을 때 잠시 돈을 맡겨둘 단기국채(SHY / 153130)를 1개 이상 꼭 넣어주세요.",
                    "수익률 비교 기간(기본 126일 = 약 6개월): 업종 순환 속도에 가장 잘 맞는 기간입니다.",
                    "상위 선택 종목 수(기본 3개): 1개에 몰빵하는 것보다 2~3개에 나누면 훨씬 안정적입니다.",
                ],
                "execution": [
                    "매월 첫 거래일 장 시작 1분 뒤, 공격 후보군 전체의 최근 6개월 '출렁임 대비 수익률 점수'를 매겨 1등부터 줄 세웁니다.",
                    "상위 3개 종목을 뽑고, 각각 '최근 6개월 수익률 > 0'이면서 '200일 평균선 위'인지 확인합니다.",
                    "조건을 통과한 종목은 1/3씩 투자하고, 조건이 깨진 자리(예: 3개 중 1개 탈락)는 방어 채권(단기채)에 대신 넣어둡니다.",
                    "지난달 들고 있던 종목이 순위에서 밀리면 먼저 전량 매도한 뒤 새로 뽑힌 주도주를 시장가로 매수합니다.",
                ],
                "caution": "주도 업종이 한 달도 안 돼 급하게 바뀌는 혼조 장세에서는 갈아탄 직후 주춤할 수 있습니다. 최소 6개월~1년 이상 꾸준히 돌려야 효과가 나타납니다.",
            },
            presets={
                "US": [
                    {"symbol": "XLK", "name": "기술 (IT)", "role": "offensive", "weight": 0},
                    {"symbol": "XLV", "name": "헬스케어", "role": "offensive", "weight": 0},
                    {"symbol": "XLF", "name": "금융", "role": "offensive", "weight": 0},
                    {"symbol": "XLE", "name": "에너지", "role": "offensive", "weight": 0},
                    {"symbol": "XLI", "name": "산업재", "role": "offensive", "weight": 0},
                    {"symbol": "XLY", "name": "경기소비재", "role": "offensive", "weight": 0},
                    {"symbol": "XLP", "name": "필수소비재", "role": "offensive", "weight": 0},
                    {"symbol": "XLU", "name": "유틸리티", "role": "offensive", "weight": 0},
                    {"symbol": "SHY", "name": "단기국채 (방어)", "role": "defensive", "weight": 0},
                ],
                "KR": [
                    {"symbol": "091160", "name": "반도체", "role": "offensive", "weight": 0},
                    {"symbol": "091170", "name": "은행", "role": "offensive", "weight": 0},
                    {"symbol": "091180", "name": "자동차", "role": "offensive", "weight": 0},
                    {"symbol": "102970", "name": "증권", "role": "offensive", "weight": 0},
                    {"symbol": "244580", "name": "바이오", "role": "offensive", "weight": 0},
                    {"symbol": "305720", "name": "2차전지산업", "role": "offensive", "weight": 0},
                    {"symbol": "132030", "name": "골드선물", "role": "offensive", "weight": 0},
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
        if role not in VALID_ROLES:
            raise ApiError(400, "invalid_role", f"자산 역할은 {', '.join(VALID_ROLES)} 중 하나여야 합니다")
        if model.uses_roles and role not in model.allowed_roles:
            raise ApiError(400, "invalid_role", f"이 모델에서 허용하는 역할은 {', '.join(model.allowed_roles)}입니다")
        try:
            weight = float(item.get("weight", 0.0))
        except (TypeError, ValueError):
            raise ApiError(400, "invalid_weight", f"{sym}: 비중은 숫자여야 합니다")
        if weight < 0:
            raise ApiError(400, "invalid_weight", f"{sym}: 비중은 0 이상이어야 합니다")
        clean.append({"symbol": sym, "name": str(item.get("name") or sym)[:40], "role": role, "weight": weight})
    if model.uses_weights and not model.uses_roles and sum(a["weight"] for a in clean) <= 0:
        raise ApiError(400, "invalid_weight", "목표 비중 합계는 0보다 커야 합니다")
    if model.uses_roles:
        if not any(a["role"] == "offensive" for a in clean):
            raise ApiError(400, "invalid_assets", "공격(offensive) 자산이 최소 1개 필요합니다")
        if not any(a["role"] == "defensive" for a in clean):
            raise ApiError(400, "invalid_assets", "방어(defensive) 자산이 최소 1개 필요합니다")
        if "canary" in model.allowed_roles and not any(a["role"] == "canary" for a in clean):
            raise ApiError(400, "invalid_assets", "감시병(canary) 자산이 최소 1개 필요합니다")
        if "core" in model.allowed_roles:
            cores = [a for a in clean if a["role"] == "core"]
            if not cores:
                raise ApiError(400, "invalid_assets", "고정(core) 자산이 최소 1개 필요합니다")
            core_sum = sum(a["weight"] for a in cores)
            if core_sum <= 0 or core_sum > 100:
                raise ApiError(400, "invalid_weight", "고정(core) 자산 비중 합계는 0보다 크고 100 이하여야 합니다")
    return clean


def align_bars(bars_by_symbol: dict[str, Bars]) -> tuple[list[int], dict[str, list[float]], dict[str, list[float]]]:
    """Intersect trading calendar dates across all symbols (robust across providers/timezones)."""
    import calendar as _cal

    maps: dict[str, dict[str, tuple[float, float]]] = {}
    day_ts: dict[str, int] = {}
    for sym, b in bars_by_symbol.items():
        m: dict[str, tuple[float, float]] = {}
        for t, o, c in zip(b.time, b.open, b.close):
            d = _day_str(t)
            m[d] = (o, c)
            if d not in day_ts:
                day_ts[d] = _cal.timegm(datetime.strptime(d, "%Y-%m-%d").timetuple())
        maps[sym] = m
    common_days = sorted(set.intersection(*(set(m.keys()) for m in maps.values()))) if maps else []
    times = [day_ts[d] for d in common_days]
    opens = {sym: [maps[sym][d][0] for d in common_days] for sym in bars_by_symbol}
    closes = {sym: [maps[sym][d][1] for d in common_days] for sym in bars_by_symbol}
    return times, opens, closes


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

    # Benchmark: equal-weight buy-and-hold across investable (non-canary) assets
    bench_syms = [a["symbol"] for a in assets if a.get("role") != "canary"] or syms
    eq_w = 1.0 / len(bench_syms)
    bh_qty = {}
    bh_cash = capital
    for s in bench_syms:
        px = closes[s][start] * (1.0 + slip) * (1.0 + comm)
        q = math.floor(capital * eq_w / px) if px > 0 else 0
        bh_qty[s] = q
        bh_cash -= q * px
    bench = [bh_cash + sum(bh_qty[s] * closes[s][j] for s in bench_syms) for j in range(start, len(times))]

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
