from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class Rsi2MeanReversion(Strategy):
    id = "rsi2"
    name = "RSI(2) 평균회귀 (래리 코너스)"
    category = "swing"
    timeframe = "1d"
    theory = "래리 코너스 · 장기 추세 내 단기 과매도 평균회귀"
    summary = "장기 상승 추세 속 단기 급락(RSI(2) 과매도)에 매수, 단기 이평 회복 시 매도"
    description = (
        "래리 코너스의 단기 평균회귀 전략. 종가가 장기 이평(기본 200일) 위인 상승 추세에서만, 초단기 RSI(2)가 "
        "기준값(기본 10) 아래로 떨어지면 다음 날 시가에 매수합니다. 종가가 단기 이평(기본 5일)을 넘으면 다음 날 시가에 "
        "매도합니다. 승률이 높고 보유 기간이 짧지만, 한 번의 큰 하락에 손실이 커질 수 있습니다."
    )
    params = [
        Param("rsi_n", "RSI 기간", 2, 2, 30),
        Param("entry", "매수 RSI 기준", 10, 1, 50),
        Param("trend", "추세 이평 (0=끔)", 200, 0, 400),
        Param("exit_ma", "청산 이평", 5, 2, 50),
    ]

    def warmup(self) -> int:
        return max(self.p["trend"], self.p["exit_ma"], self.p["rsi_n"] + 1)

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.rsi = ind.rsi(bars.close, self.p["rsi_n"])
        self.trend = ind.sma(bars.close, self.p["trend"]) if self.p["trend"] else None
        self.exit_ma = ind.sma(bars.close, self.p["exit_ma"])

    def on_bar(self, i: int, ctx: Context) -> list:
        c, r = self.close[i], self.rsi[i]
        if ctx.position.is_open:
            if self.exit_ma[i] is not None and c > self.exit_ma[i]:
                return [Order("sell", reason=f"종가 > {self.p['exit_ma']}일 이평")]
            return []
        trend_ok = self.trend is None or (self.trend[i] is not None and c > self.trend[i])
        if r is not None and r < self.p["entry"] and trend_ok:
            return [Order("buy", reason=f"RSI({self.p['rsi_n']}) {r:.1f} < {self.p['entry']}")]
        return []

    def chart_indicators(self) -> list:
        return [
            {"id": "ma", "params": {"p1": self.p["exit_ma"], "p2": self.p["trend"], "p3": 0, "p4": 0}},
            {"id": "rsi", "params": {"n": self.p["rsi_n"]}},
        ]
