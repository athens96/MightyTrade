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
    guide = {
        "background": (
            "1980년대 존 볼린저(John Bollinger)가 정립한 변동성 채널 이론입니다. 주가 수익률이 이동평균(μ) 주위로 표준편차(σ) 범위 내에 "
            "분포한다는 통계적 성질에 착안해, 단기 수급 충격으로 하단 밴드(μ − kσ) 밖으로 밀려난 가격은 결국 평균(중심선)으로 "
            "되돌아오려는 평균회귀(Mean Reversion) 압력을 받는다는 원리를 이용합니다."
        ),
        "setup": [
            "기간(n): 기본 20일(약 1개월 거래일). 짧은 스윙은 10~15일, 중기 스윙은 20~30일을 사용합니다.",
            "표준편차 배수(k): 기본 2.0. 신호가 너무 적으면 1.8, 보수적으로 극단적 과매도만 노리면 2.2~2.5로 높입니다.",
            "종목 선택: 구조적 하락 위험이 낮은 우량 대형주·지수 ETF·박스권 성향 종목에 유리합니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 종가 기준 n일 이동평균(중심선)과 하단 밴드(이평 − k×표준편차)를 계산합니다.",
            "미보유 상태에서 전일 종가가 하단 밴드 아래로 내려갔으면 당일 시장가 매수 주문을 보냅니다.",
            "보유 상태에서 전일 종가가 중심선(n일 이평) 위로 회복했으면 당일 시장가 매도로 익절/청산합니다.",
        ],
        "caution": "악재·실적 붕괴로 하단 밴드를 타고 계속 내려가는 '밴드 워킹(하락 추세)'에서는 큰 낙폭이 발생할 수 있습니다. 개별 소형주보다 지수·우량주에 적합합니다.",
    }
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
