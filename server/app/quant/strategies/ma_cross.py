from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class MovingAverageCross(Strategy):
    id = "ma_cross"
    name = "이동평균 크로스"
    category = "swing"
    timeframe = "1d"
    theory = "다우 이론 · 추세추종 (Trend Following)"
    summary = "단기 이평이 장기 이평을 상향 돌파하면 매수, 하향 돌파하면 매도"
    description = (
        "가장 기본적인 추세추종 전략. 단기 이동평균이 장기 이동평균을 위로 뚫으면(골든크로스) 다음 날 시가에 매수하고, "
        "아래로 뚫으면(데드크로스) 다음 날 시가에 매도합니다. 추세가 길게 이어지는 장에서 강하고, 횡보장에서는 잦은 "
        "가짜 신호(휩소)로 손실이 쌓이는 경향이 있습니다."
    )
    guide = {
        "background": (
            "찰스 다우(Charles Dow)의 '주가는 추세를 형성하며 명확한 반전 신호가 나오기 전까지 지속된다'는 추세추종 원리에서 출발했습니다. "
            "단기 이동평균은 최근 시장 참여자의 평균 매입 단가(심리 변화), 장기 이동평균은 중장기 펀더멘털 추세를 반영합니다. "
            "행동재무학적으로는 투자자의 정보 과소반응(underreaction)과 군집 추종(herding)이 가격 추세를 길게 끌고 가는 근거로 설명됩니다."
        ),
        "setup": [
            "단기 이평(fast): 10~20일(민감한 스윙) 또는 50일(중장기 추세)로 설정합니다.",
            "장기 이평(slow): 60~200일 범위에서 단기 이평의 2.5~4배 길이가 일반적입니다(예: 20/60, 50/200).",
            "퀀트 랩 '최적화' 탭에서 fast×slow 히트맵의 이웃 평균이 높은 고원(plateau) 구간을 선택하세요.",
            "종목 선택: 변동성만 크고 방향성이 없는 테마주보다 추세가 뚜렷한 지수 ETF·대형 성장주에 적합합니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤, 전 거래일 종가까지의 일봉으로 단기/장기 SMA를 계산합니다.",
            "미보유 상태에서 어제 단기선이 장기선을 상향 돌파(골든크로스)했으면 당일 시장가 매수 주문을 전송합니다.",
            "보유 상태에서 어제 단기선이 장기선을 하향 돌파(데드크로스)했으면 보유 수량 전량을 당일 시장가 매도합니다.",
            "교차 신호가 없으면 포지션을 그대로 유지하며 다음 거래일까지 대기합니다.",
        ],
        "caution": "박스권·횡보장에서는 골든/데드크로스가 반복되는 휩소(whipsaw)로 작은 손실이 누적됩니다. 승률은 35~45%로 낮은 편이나 손익비로 수익을 내는 구조입니다.",
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
