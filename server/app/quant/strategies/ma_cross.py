from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class MovingAverageCross(Strategy):
    id = "ma_cross"
    name = "이동평균 크로스"
    category = "swing"
    timeframe = "1d"
    theory = "다우 이론 · 달리는 기차에 올라타는 추세추종"
    summary = "단기 평균 가격이 장기 평균 가격을 뚫고 올라가면 사고, 내려가면 팝니다"
    description = (
        "가장 오래되고 이해하기 쉬운 추세 전략입니다. 최근 가격 흐름(단기 이동평균)이 오랜 기간의 평균 가격(장기 이동평균)보다 "
        "위로 올라서면 '이제 상승 흐름이 시작됐다'고 보고 매수하고, 반대로 아래로 내려가면 '상승이 끝났다'고 보고 매도합니다."
    )
    guide = {
        "background": (
            "100년 넘게 검증된 '다우 이론'의 핵심은 아주 단순합니다. 한 번 방향을 잡은 주가는 관성처럼 한동안 그 방향으로 계속 움직인다는 것입니다. "
            "마치 달리기 시작한 기차가 금방 멈추지 않는 것과 같습니다. 최근 20일 평균 가격은 '요즘 사람들의 투자 심리', 60일 평균 가격은 '긴 흐름'을 뜻하므로, "
            "최근 가격이 긴 흐름을 위로 뚫는 순간(골든크로스)을 상승 출발 신호로 봅니다."
        ),
        "setup": [
            "단기 이평(며칠 평균): 보통 20일(약 한 달 거래일)로 둡니다. 더 빠르게 반응하고 싶으면 10일로 줄입니다.",
            "장기 이평: 보통 단기 이평의 3배 정도인 60일(약 석 달)을 씁니다. 아주 묵직하게 보려면 50일/200일 조합도 많이 씁니다.",
            "어떤 숫자가 좋은지 모르겠다면 퀀트 랩의 '최적화'에서 숫자 조합 표를 보고, 한 칸만 튀는 값보다 주변 칸들도 골고루 수익이 좋은 값을 고르세요.",
            "잘 맞는 종목: 뉴스 한 번에 급등락하는 작은 종목보다, 큰 흐름이 길게 이어지는 지수 ETF나 대형주에 잘 맞습니다.",
        ],
        "execution": [
            "매일 장이 열리고 1분 뒤에, 어제 마감 가격까지로 단기 평균과 장기 평균을 계산합니다.",
            "주식을 안 가지고 있는데 어제 단기 평균이 장기 평균을 위로 뚫었다면, 오늘 바로 시장가로 매수합니다.",
            "주식을 가지고 있는데 어제 단기 평균이 장기 평균 아래로 내려갔다면, 가지고 있는 수량을 전부 시장가로 매도합니다.",
            "두 선이 엇갈리지 않았다면 아무것도 하지 않고 그대로 유지합니다.",
        ],
        "caution": "주가가 오르지도 내리지도 않고 제자리에서 흔들릴 때(횡보장)는 '샀다 팔았다'를 반복하며 조금씩 손실이 날 수 있습니다. 10번 중 4번 정도만 이겨도, 크게 오를 때 길게 먹어서 수익을 내는 방식입니다.",
    }
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
