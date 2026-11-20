from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class Rsi2MeanReversion(Strategy):
    id = "rsi2"
    name = "RSI(2) 평균회귀 (래리 코너스)"
    category = "swing"
    timeframe = "1d"
    theory = "래리 코너스 · 잘 나가는 종목이 잠깐 세일할 때 사서 빠르게 되파는 전략"
    summary = "장기 상승 중인 종목이 2~3일 급락하면 사고, 5일 평균 가격을 회복하면 바로 팝니다"
    description = (
        "유명 단기 투자자 래리 코너스의 대표 전략입니다. 1년(200일) 흐름이 탄탄한 상승장인 종목이 며칠 사이 갑자기 크게 눌렸을 때 "
        "'잠깐 세일하는 기회'로 보고 매수한 뒤, 며칠 안에 반등(5일 평균 회복)하면 빠르게 팔아 수익을 챙깁니다."
    )
    guide = {
        "background": (
            "좋은 물건이 잠깐 반짝 세일을 하면 사람들이 몰려들어 가격이 금방 원래대로 돌아옵니다. 주식도 마찬가지입니다. "
            "장기 흐름(200일 평균 위)이 튼튼한 종목이 별다른 악재 없이 2~3일 연속 하락해 초단기 과매도 점수(2일 RSI가 10 미만)에 도달하면, "
            "저가 매수세가 들어와 며칠 안에 반등할 확률이 매우 높다는 점(보통 승률 65~75%)을 활용합니다."
        ),
        "setup": [
            "RSI 기간: 기본 2일 그대로 두세요. 아주 짧은 2~3일 급락을 잡는 것이 이 전략의 핵심입니다.",
            "매수 RSI 기준: 기본 10. 더 싸게 살 때만 들어가려면 5로 낮추고, 기회를 늘리려면 15로 높입니다.",
            "추세 이평: 기본 200일(약 1년 평균). 하락장 종목을 사지 않도록 막아주는 가장 중요한 안전벨트이므로 켜두는 것을 추천합니다.",
            "청산 이평: 기본 5일(일주일 평균). 반등이 나오면 욕심부리지 않고 빠르게 수익을 챙깁니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제 마감 기준으로 2일 RSI 점수, 200일 평균선, 5일 평균선을 계산합니다.",
            "안 가지고 있을 때: 주가가 200일선 위에 있고 RSI(2)가 10보다 낮으면 오늘 시장가로 매수합니다.",
            "가지고 있을 때: 어제 마감 가격이 5일 평균선 위로 올라왔다면 오늘 시장가로 바로 팔아 청산합니다.",
        ],
        "caution": "이기는 횟수는 많지만 별도 손절선이 없어, 큰 하락장이 갑자기 시작되면 한 번에 손실이 커질 수 있습니다. 퀀트 랩 '몬테카를로'에서 최악의 경우 손실 폭을 꼭 먼저 확인해 보세요.",
    }
    params = [
        Param("rsi_n", "RSI 기간", 2, 2, 30),
        Param("entry", "매수 RSI 기준", 10, 1, 50),
        Param("trend", "추세 이평 (0=끔)", 200, 0, 400),
        Param("exit_ma", "청산 이평", 5, 2, 50),
    ]

    def warmup(self) -> int:
        return max(self.p["trend"], self.p["exit_ma"], self.p["rsi_n"] + 1)

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.rsi = ind.rsi(bars.close, self.p["rsi_n"])
        self.trend = ind.sma(bars.close, self.p["trend"]) if self.p["trend"] else None
        self.exit_ma = ind.sma(bars.close, self.p["exit_ma"])

    def on_bar(self, i: int, ctx: Context) -> list:
        c, r = self.close[i], self.rsi[i]
        if ctx.position.is_open:
            if self.exit_ma[i] is not None and c > self.exit_ma[i]:
                return [Order("sell", reason=f"종가 > {self.p['exit_ma']}일 이평")]
            return []
        trend_ok = self.trend is None or (self.trend[i] is not None and c > self.trend[i])
        if r is not None and r < self.p["entry"] and trend_ok:
            return [Order("buy", reason=f"RSI({self.p['rsi_n']}) {r:.1f} < {self.p['entry']}")]
        return []

    def chart_indicators(self) -> list:
        return [
            {"id": "ma", "params": {"p1": self.p["exit_ma"], "p2": self.p["trend"], "p3": 0, "p4": 0}},
            {"id": "rsi", "params": {"n": self.p["rsi_n"]}},
        ]
