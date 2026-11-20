from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class MovingAverageCross(Strategy):
    id = "ma_cross"
    name = "이동평균 크로스"
    summary = "단기 이평이 장기 이평을 상향 돌파하면 매수, 하향 돌파하면 매도"
    description = (
        "가장 기본적인 추세추종 전략. 단기 이동평균이 장기 이동평균을 위로 뚫으면(골든크로스) 다음 날 시가에 매수하고, "
        "아래로 뚫으면(데드크로스) 다음 날 시가에 매도합니다. 추세가 길게 이어지는 장에서 강하고, 횡보장에서는 잦은 "
        "가짜 신호(휩소)로 손실이 쌓이는 경향이 있습니다."
    )
    params = [
        Param("fast", "단기 이평", 20, 2, 200),
        Param("slow", "장기 이평", 60, 3, 400),
    ]

    def validate(self) -> None:
        if self.p["fast"] >= self.p["slow"]:
            raise StrategyParamError("단기 이평은 장기 이평보다 짧아야 합니다")

    def warmup(self) -> int:
        return self.p["slow"] + 1

    def prepare(self, bars: Bars) -> None:
        self.fast = ind.sma(bars.close, self.p["fast"])
        self.slow = ind.sma(bars.close, self.p["slow"])

    def on_bar(self, i: int, ctx: Context) -> list:
        if i < 1:
            return []
        f0, s0, f1, s1 = self.fast[i - 1], self.slow[i - 1], self.fast[i], self.slow[i]
        if None in (f0, s0, f1, s1):
            return []
        if not ctx.position.is_open and f0 <= s0 and f1 > s1:
            return [Order("buy", reason=f"골든크로스 ({self.p['fast']}/{self.p['slow']})")]
        if ctx.position.is_open and f0 >= s0 and f1 < s1:
            return [Order("sell", reason=f"데드크로스 ({self.p['fast']}/{self.p['slow']})")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "ma", "params": {"p1": self.p["fast"], "p2": self.p["slow"], "p3": 0, "p4": 0}}]
