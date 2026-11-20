from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class VolatilityBreakout(Strategy):
    id = "vol_breakout"
    name = "변동성 돌파 (래리 윌리엄스)"
    category = "daytrade"
    timeframe = "1d"
    theory = "래리 윌리엄스 · 장중 변동성 확장 돌파 (당일/익일 시가 청산)"
    summary = "오늘 가격이 시가 + 전일 변동폭×K를 넘으면 매수, 다음 날 시가에 매도"
    description = (
        "래리 윌리엄스의 단기 돌파 전략. 매수 기준가 = 당일 시가 + (전일 고가 − 전일 저가) × K. 장중에 기준가를 "
        "돌파하면 그 가격에 매수하고(시가가 이미 넘어서 시작하면 시가에 매수), 다음 날 시가에 무조건 매도합니다. "
        "K가 작을수록 매매가 잦아집니다. 이평 필터를 켜면 종가가 n일 이평 위일 때만 진입해 하락장 손실을 줄입니다. "
        "매매가 잦아 수수료·슬리피지 영향이 큽니다."
    )
    params = [
        Param("k", "K 값", 0.5, 0.1, 1.5, step=0.05, integer=False),
        Param("ma_filter", "이평 필터 (0=끔)", 0, 0, 200),
    ]

    def warmup(self) -> int:
        return max(2, self.p["ma_filter"])

    def prepare(self, bars: Bars) -> None:
        self.b = bars
        self.ma = ind.sma(bars.close, self.p["ma_filter"]) if self.p["ma_filter"] else None

    def on_bar(self, i: int, ctx: Context) -> list:
        orders = []
        if ctx.position.is_open:
            orders.append(Order("sell", reason="다음 날 시가 청산"))
        rng = self.b.high[i] - self.b.low[i]
        trend_ok = self.ma is None or (self.ma[i] is not None and self.b.close[i] > self.ma[i])
        if rng > 0 and trend_ok:
            orders.append(Order("buy", kind="stop", stop_from_open=self.p["k"] * rng,
                                reason=f"변동성 돌파 (K={self.p['k']:g})"))
        return orders

    def chart_indicators(self) -> list:
        if not self.p["ma_filter"]:
            return []
        return [{"id": "ma", "params": {"p1": self.p["ma_filter"], "p2": 0, "p3": 0, "p4": 0}}]
