from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class VolatilityBreakout(Strategy):
    id = "vol_breakout"
    name = "변동성 돌파 (래리 윌리엄스)"
    category = "daytrade"
    timeframe = "1d"
    theory = "래리 윌리엄스 · 장중에 힘 있게 치고 오를 때 올라타서 내일 아침에 팔기"
    summary = "오늘 주가가 '시작가 + 어제 움직인 폭×K'를 뚫고 오르면 사고, 다음 날 아침에 팝니다"
    description = (
        "세계 투자 대회 우승자 래리 윌리엄스의 대표 단타 규칙입니다. 어제 주가가 움직인 폭(최고가 − 최저가)의 절반(K=0.5) 이상 "
        "오늘 강하게 치고 올라오면 '오늘 사려는 힘이 강하다'고 보고 매수한 뒤, 다음 날 장 시작할 때 미련 없이 모두 팝니다."
    )
    guide = {
        "background": (
            "1987년 실제 돈으로 열린 세계 투자 대회에서 1년 만에 100배 넘는 수익을 낸 래리 윌리엄스의 전략입니다. "
            "보통 하루 동안 주가는 어제 움직인 폭 안에서 적당히 오르내리는데, 오늘 시작가보다 '어제 폭의 절반(0.5배)' 이상 강하게 "
            "위로 뚫고 나온다는 것은 큰 매수세가 들어왔다는 신호입니다. 그 탄력이 다음 날 아침까지 이어지는 경우가 많아 하루만 짧게 먹고 나옵니다."
        ),
        "setup": [
            "K 값: 기본값 0.5(어제 폭의 절반). 숫자가 작을수록(0.3~0.4) 자주 사고, 클수록(0.6~0.7) 정말 강하게 뚫을 때만 삽니다.",
            "이평 필터: 처음엔 0(끔)이지만, 5~20일(단기 상승장)이나 60일(중기 상승장)을 넣으면 하락장에 헛되이 사는 걸 크게 줄여줍니다.",
            "매매가 잦은 편이라 퀀트 랩에서 수수료·세금을 꼭 넣고 백테스트해 본 뒤 사용하세요.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제 움직인 폭(최고가 − 최저가)과 오늘 시작가를 확인합니다.",
            "어제 사서 들고 있던 주식이 있다면 오늘 장 시작과 동시에 시장가로 먼저 팔아 현금을 확보합니다.",
            "그다음 '오늘 시작가 + 어제 폭 × K' 가격에 자동 매수 예약(조건주문)을 걸어둡니다(이미 넘었으면 즉시 매수).",
            "장 끝날 때까지 그 가격을 못 뚫으면 예약은 자동 취소되고 그날은 쉬어갑니다.",
        ],
        "caution": "낮에 잠깐 뚫고 올라갔다가 오후에 푹 꺼지는 종목에서는 손실이 쌓이기 쉽습니다. 거래가 자주 일어나므로 수수료와 세금 영향이 큽니다.",
    }
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
