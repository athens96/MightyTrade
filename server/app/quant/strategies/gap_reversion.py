from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class OpeningGapReversion(Strategy):
    id = "gap_reversion"
    name = "오프닝 갭 회귀 (단타)"
    category = "daytrade"
    timeframe = "1d"
    theory = "시장 미세구조 과잉반응 · 개장 갭하락 평균회귀 (Gap Fill)"
    summary = "전일 종가 대비 N% 이상 갭하락·눌림 지정가 매수, 다음 날 시가에 청산"
    description = (
        "개장 직후 패닉·오버나이트 뉴스로 인한 단기 과잉반응(갭하락)이 장중·익일 되돌림(Gap Fill)을 보이는 "
        "시장 미세구조 현상을 활용하는 단타 전략. 전일 종가 대비 지정 비율(기본 -1.5%) 아래에 지정가 매수 주문을 걸어두고, "
        "체결되면 다음 날 시가에 일괄 청산합니다. 추세 필터를 켜면 장기 상승 추세 종목에서만 갭하락을 매수합니다."
    )
    params = [
        Param("gap_pct", "갭하락 기준 (%)", 1.5, 0.3, 10.0, step=0.1, integer=False),
        Param("trend_ma", "추세 이평 (0=끔)", 60, 0, 300),
    ]

    def warmup(self) -> int:
        return max(2, self.p["trend_ma"])

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.trend = ind.sma(bars.close, self.p["trend_ma"]) if self.p["trend_ma"] else None

    def on_bar(self, i: int, ctx: Context) -> list:
        orders = []
        if ctx.position.is_open:
            orders.append(Order("sell", reason="다음 날 시가 청산"))
        trend_ok = self.trend is None or (self.trend[i] is not None and self.close[i] > self.trend[i])
        if trend_ok and self.close[i] > 0:
            limit_px = self.close[i] * (1.0 - self.p["gap_pct"] / 100.0)
            orders.append(Order("buy", kind="limit", limit=limit_px,
                                reason=f"전일 종가 대비 -{self.p['gap_pct']:g}% 갭 회귀 지정가"))
        return orders

    def chart_indicators(self) -> list:
        if not self.p["trend_ma"]:
            return []
        return [{"id": "ma", "params": {"p1": self.p["trend_ma"], "p2": 0, "p3": 0, "p4": 0}}]
