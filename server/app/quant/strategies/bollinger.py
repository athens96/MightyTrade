from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class BollingerReversion(Strategy):
    id = "bollinger"
    name = "볼린저 밴드 회귀"
    category = "swing"
    timeframe = "1d"
    theory = "존 볼린저 · 통계적 평균회귀 (Mean Reversion)"
    summary = "종가가 하단 밴드 아래로 내려가면 매수, 중심선 위로 회복하면 매도"
    description = (
        "통계적으로 가격은 대부분 밴드 안에 머문다는 가정의 평균회귀 전략. 종가가 하단 밴드(이평 − 표준편차×배수) "
        "아래로 내려가면 다음 날 시가에 매수하고, 종가가 중심선(이평)을 넘으면 매도합니다. 박스권에서 잘 맞고, "
        "강한 하락 추세에서는 계속 떨어지는 칼날을 잡을 위험이 있습니다."
    )
    params = [
        Param("n", "기간", 20, 5, 200),
        Param("k", "표준편차 배수", 2, 0.5, 4, step=0.1, integer=False),
    ]

    def warmup(self) -> int:
        return self.p["n"]

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.bb = ind.bollinger(bars.close, self.p["n"], self.p["k"])

    def on_bar(self, i: int, ctx: Context) -> list:
        lower, mid = self.bb["lower"][i], self.bb["mid"][i]
        if lower is None or mid is None:
            return []
        c = self.close[i]
        if not ctx.position.is_open and c < lower:
            return [Order("buy", reason="종가 < 하단 밴드")]
        if ctx.position.is_open and c > mid:
            return [Order("sell", reason="종가 > 중심선")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "bb", "params": {"n": self.p["n"], "k": self.p["k"]}}]
