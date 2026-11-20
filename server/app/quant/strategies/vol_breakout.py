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
    guide = {
        "background": (
            "1987년 로빈스 월드컵 트레이딩 대회에서 1년 만에 11,376% 수익을 기록한 래리 윌리엄스(Larry Williams)의 대표 단기 돌파 이론입니다. "
            "장중 가격이 당일 시가에서 전일 변동폭(Range = 고가 − 저가)의 일정 비율(K) 이상을 상향 돌파하면 '단기 매수 세력의 "
            "불균형(모멘텀 확장)'이 시작된 것으로 보고, 그 추진력이 익일 시가까지 이어지는 확률적 우위를 노립니다."
        ),
        "setup": [
            "K 값: 전통 기본값은 0.5. 변동성이 큰 종목은 0.5~0.7(노이즈 필터), 지수 ETF는 0.3~0.5가 자주 쓰입니다.",
            "이평 필터(ma_filter): 0(끔) 대신 5~20일(단기 상승장) 또는 60일(중기 상승장)을 켜면 하락장 손실을 크게 줄일 수 있습니다.",
            "수수료·거래세·슬리피지 비중이 크므로 퀀트 랩에서 반드시 실제 비용을 넣고 백테스트·워크포워드를 확인하세요.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 변동폭(고가 − 저가)과 오늘 시가를 확인합니다.",
            "전날 매수해 보유 중인 포지션이 있다면 먼저 시장가로 전량 청산하고, 짧은 체결 확인 후 매수 자금을 산정합니다.",
            "추세 필터를 통과하면 '오늘 시가 + 전일 변동폭 × K' 가격에 당일 유효 매수 조건주문(SINGLE)을 등록합니다(이미 돌파 시 시장가).",
            "장중 돌파하지 않으면 조건주문은 장 마감 시 자동 만료되어 현금을 유지합니다.",
        ],
        "caution": "장중 윗꼬리(돌파 직후 급락)가 잦은 종목에서는 손실이 누적되며, 하루 1회 이상 매매가 발생할 수 있어 거래비용과 슬리피지에 매우 민감합니다.",
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
