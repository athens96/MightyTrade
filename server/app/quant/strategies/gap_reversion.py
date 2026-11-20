from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class OpeningGapReversion(Strategy):
    id = "gap_reversion"
    name = "오프닝 갭 회귀 (단타)"
    category = "daytrade"
    timeframe = "1d"
    theory = "아침에 겁먹고 싸게 던진 물량을 받아서 다음 날 되팔기"
    summary = "어제보다 N% 이상 싸게 떨어질 때만 지정가로 사고, 다음 날 아침에 팝니다"
    description = (
        "밤사이 뉴스에 놀란 사람들이 아침에 급하게 싸게 팔아치울 때, 오히려 그 가격에 받아두었다가 가격이 진정되면 되파는 단타 전략입니다. "
        "어제 마감가보다 일정 비율(기본 -1.5%) 아래 가격에 매수 주문을 미리 걸어두고, 체결되면 다음 날 아침에 바로 팝니다."
    )
    guide = {
        "background": (
            "밤사이에 조금만 불안한 뉴스가 나와도 아침 장이 열리자마자 겁먹은 매도 주문이 한꺼번에 쏟아져 주가가 실제 가치보다 과하게 "
            "떨어져 출발(갭하락)하는 경우가 많습니다. 하지만 장이 진행되며 흥분이 가라앉으면 원래 가격 쪽으로 되돌아오는 경향이 강한데, "
            "이 '아침 과잉반응 되돌림'을 낮은 가격에 미리 기다렸다가(지정가) 잡는 전략입니다."
        ),
        "setup": [
            "갭하락 기준(%): 대형주·지수 ETF는 1.0~2.0%, 평소 출렁임이 큰 종목은 2.5~4.0%로 넉넉하게 잡으세요.",
            "추세 이평: 기본 60일(석 달 평균). 원래부터 망가져서 계속 떨어지는 종목은 피하고, 평소 흐름이 좋은 종목이 잠깐 눌릴 때만 삽니다.",
            "주문 가격은 한국·미국 주식 호가 단위에 맞춰 자동으로 깔끔하게 반올림됩니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제 마감가와 60일 평균선을 확인합니다.",
            "어제 싸게 사서 들고 있던 주식이 있다면 오늘 장 시작 시 시장가로 먼저 팝니다.",
            "흐름이 괜찮다면 '어제 마감가보다 N% 싼 가격'에 오늘 하루만 유효한 지정가 매수 주문을 걸어둡니다.",
            "오늘 그 가격까지 안 내려오면 사지 않고 넘어가며, 체결되면 다음 거래일 아침에 자동 청산됩니다.",
        ],
        "caution": "회사 실적 쇼크나 유상증자 같은 진짜 큰 악재로 급락한 날에는 반등 없이 더 떨어질 수 있습니다. 개별 종목보다 지수 ETF에 훨씬 안전합니다.",
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
