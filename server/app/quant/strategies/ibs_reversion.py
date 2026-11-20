from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class IbsMeanReversion(Strategy):
    id = "ibs_reversion"
    name = "IBS 내부봉 평균회귀"
    category = "swing"
    timeframe = "1d"
    theory = "파고니디스 · 단기 종가 위치(Internal Bar Strength) 평균회귀"
    summary = "상승 추세 속 당일 저점 부근 마감(IBS < 기준)에 매수, 고점 부근 반등 시 매도"
    description = (
        "IBS = (종가 − 저가) ÷ (고가 − 저가)로 당일 변동폭 내 종가 위치(0~1)를 측정하는 학술 검증 스윙 전략. "
        "장기 상승 추세(기본 200일 이평 위)에서 일시적 매도 압력으로 당일 저가 부근(IBS < 0.2)에 마감하면 "
        "단기 과잉반응으로 보고 다음 날 시가에 매수하며, 고가 부근(IBS > 0.8) 반등 또는 최대 보유일 도달 시 청산합니다."
    )
    params = [
        Param("entry_ibs", "매수 IBS 기준", 0.2, 0.05, 0.5, step=0.05, integer=False),
        Param("exit_ibs", "청산 IBS 기준", 0.8, 0.5, 0.95, step=0.05, integer=False),
        Param("trend_ma", "추세 이평 (0=끔)", 200, 0, 400),
        Param("max_hold", "최대 보유일", 5, 1, 30),
    ]

    def validate(self) -> None:
        if self.p["entry_ibs"] >= self.p["exit_ibs"]:
            raise StrategyParamError("매수 IBS 기준은 청산 IBS 기준보다 작아야 합니다")

    def warmup(self) -> int:
        return max(self.p["trend_ma"], 2)

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.ibs = [
            0.5 if h <= l else (c - l) / (h - l)
            for h, l, c in zip(bars.high, bars.low, bars.close)
        ]
        self.trend = ind.sma(bars.close, self.p["trend_ma"]) if self.p["trend_ma"] else None

    def on_bar(self, i: int, ctx: Context) -> list:
        ibs = self.ibs[i]
        if ctx.position.is_open:
            held = i - ctx.position.entry_index if ctx.position.entry_index >= 0 else 0
            if ibs > self.p["exit_ibs"]:
                return [Order("sell", reason=f"IBS {ibs:.2f} > {self.p['exit_ibs']:g}")]
            if held >= self.p["max_hold"]:
                return [Order("sell", reason=f"최대 보유일({self.p['max_hold']}일) 청산")]
            return []
        trend_ok = self.trend is None or (self.trend[i] is not None and self.close[i] > self.trend[i])
        if trend_ok and ibs < self.p["entry_ibs"]:
            return [Order("buy", reason=f"IBS {ibs:.2f} < {self.p['entry_ibs']:g}")]
        return []

    def chart_indicators(self) -> list:
        if not self.p["trend_ma"]:
            return []
        return [{"id": "ma", "params": {"p1": self.p["trend_ma"], "p2": 0, "p3": 0, "p4": 0}}]
