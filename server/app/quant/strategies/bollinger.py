from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class BollingerReversion(Strategy):
    id = "bollinger"
    name = "볼린저 밴드 회귀"
    category = "swing"
    timeframe = "1d"
    theory = "존 볼린저 · 고무줄처럼 너무 내려가면 제자리로 돌아오는 원리"
    summary = "주가가 평소 범위(하단선) 밑으로 과하게 떨어지면 사고, 평균 가격으로 돌아오면 팝니다"
    description = (
        "주가가 평소 움직이는 길(볼린저 밴드)을 고무줄에 비유한 전략입니다. 특별한 이유 없이 평소 바닥선 아래로 너무 과하게 떨어지면 "
        "'고무줄이 팽팽하게 당겨졌다'고 보고 매수하고, 다시 평소 평균 가격(중심선)으로 튕겨 올라오면 팔아서 수익을 챙깁니다."
    )
    guide = {
        "background": (
            "1980년대 존 볼린저가 만든 이론입니다. 주가는 95% 이상의 시간 동안 '최근 20일 평균 가격 ± 평소 흔들림의 2배' 범위 안에서 움직입니다. "
            "마치 고무줄을 아래로 세게 당겼다가 놓으면 가운데로 튕겨 돌아오듯, 일시적인 공포나 매도 쏠림으로 이 범위 아래로 뚝 떨어진 주가는 "
            "며칠 안에 다시 평균 가격으로 돌아오려는 성질(평균회귀)이 강합니다."
        ),
        "setup": [
            "기간: 기본 20일(약 한 달). 더 짧은 반등을 노리면 10~15일로 줄일 수 있습니다.",
            "배수(k): 기본 2.0. 숫자를 2.2~2.5로 키우면 '정말 심하게 떨어진 날'에만 조심스럽게 매수합니다.",
            "잘 맞는 종목: 망할 걱정이 적은 대형 우량주나 지수 ETF처럼 일정 범위 안에서 오르내리는 종목에 잘 맞습니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 최근 20일 평균 가격(중심선)과 하단선(평소보다 훨씬 싼 가격 기준)을 계산합니다.",
            "안 가지고 있을 때: 어제 마감 가격이 하단선보다 아래로 내려갔다면 오늘 시장가로 매수합니다.",
            "가지고 있을 때: 어제 마감 가격이 다시 중심선(20일 평균) 위로 올라왔다면 오늘 시장가로 팔아 수익을 챙깁니다.",
        ],
        "caution": "회사에 큰 악재가 터져 주가가 끝없이 계속 떨어지는 상황에서는 '싸다'고 샀다가 더 떨어질 수 있습니다. 개별 소형주보다 대표 지수 ETF나 우량주에 사용하세요.",
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
