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
    guide = {
        "background": (
            "시장 미세구조(Market Microstructure)와 행동재무학의 '개장 과잉반응(Opening Overreaction) 및 갭 메우기(Gap Fill)' 이론에 기반합니다. "
            "장 마감 후 쌓인 비이성적 공포 주문이 동시호가·장 초반에 집중 체결되며 일시적으로 공정가치 아래로 급락하면, "
            "유동성 공급자와 저가 매수세가 유입되며 가격이 되돌아오는 경향을 지정가 매수로 포착합니다."
        ),
        "setup": [
            "갭하락 기준(gap_pct): 대형주·지수 ETF는 1.0~2.0%, 변동성이 큰 개별 성장주는 2.5~4.0%를 권장합니다.",
            "추세 이평(trend_ma): 기본 60일(또는 200일). 실적 악화·하락 추세 종목의 갭하락을 피하기 위해 켜두는 것이 안전합니다.",
            "호가 단위에 맞춰 매수 지정가는 자동으로 불리하지 않은 방향(내림)으로 반올림됩니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 종가와 추세 이평을 확인합니다.",
            "어제 체결된 보유 포지션이 있으면 먼저 당일 시장가로 청산하고 체결을 확인합니다.",
            "추세 조건 충족 시 '전일 종가 × (1 − gap_pct%)' 가격으로 당일 유효 지정가(LIMIT) 매수 주문을 걸어둡니다.",
            "장중 해당 가격까지 내려오지 않으면 미체결로 만료되고, 체결되면 다음 거래일 개장 시 청산됩니다.",
        ],
        "caution": "실적 어닝쇼크·유상증자 등 펀더멘털 충격으로 발생한 갭하락은 되돌아오지 않고 추가 하락할 수 있으므로 개별 이벤트성 종목보다 지수 ETF에 적합합니다.",
    }
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
