from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class IbsMeanReversion(Strategy):
    id = "ibs_reversion"
    name = "IBS 내부봉 평균회귀"
    category = "swing"
    timeframe = "1d"
    theory = "파고니디스 · 오늘 유난히 바닥에서 끝난 종목의 다음 날 반등 노리기"
    summary = "상승장 종목이 오늘 하루 최저가 근처에서 마감하면 사고, 최고가 근처로 반등하면 팝니다"
    description = (
        "하루 동안 주가가 움직인 폭 중에서 '마감 가격이 바닥 쪽인지, 꼭대기 쪽인지'를 0~1 점수로 재는 간단한 전략입니다. "
        "전체 흐름이 좋은 종목이 장 마감 때 팔자 주문이 몰려 최저가 근처(0.2 미만)에서 끝났을 때 사서, 반등하면 며칠 안에 팝니다."
    )
    guide = {
        "background": (
            "IBS(내부봉 강도)는 계산이 아주 쉽습니다. 오늘 가장 낮았던 가격에서 끝나면 0점, 가장 높았던 가격에서 끝나면 1점입니다. "
            "장 마감 직전에 급하게 팔고 나가는 사람들이 몰려 0.2점 아래(바닥 근처)로 밀린 날에는, 다음 날부터 '너무 싸게 팔렸네' 하고 "
            "되사는 매수세가 들어와 반등하는 현상이 미국·한국 지수 ETF에서 오랜 기간 통계적으로 확인되었습니다."
        ),
        "setup": [
            "매수/청산 기준: 기본 0.2 / 0.8 (하루 움직임의 바닥 20% 안에서 끝나면 사고, 꼭대기 20% 안에서 끝나면 팝니다).",
            "추세 이평: 기본 200일. 1년 흐름이 상승 중인 지수 ETF(SPY, QQQ, KODEX 200 등)에 가장 잘 맞습니다.",
            "최대 보유일: 기본 5일. 5거래일이 지나도 반등이 안 나오면 돈이 오래 묶이지 않도록 자동으로 정리합니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제의 최고가·최저가·마감가로 IBS 점수(0~1)와 200일 평균을 계산합니다.",
            "안 가지고 있을 때: 어제 주가가 200일선 위에 있고 IBS가 0.2 미만이면 오늘 시장가로 매수합니다.",
            "가지고 있을 때: 어제 IBS가 0.8을 넘겨 강하게 반등했거나, 산 지 최대 보유일(5일)이 지나면 오늘 시장가로 매도합니다.",
        ],
        "caution": "며칠 간격으로 매매가 자주 일어나므로 세금·수수료가 부담되는 개별 소형주보다 거래비용이 저렴한 ETF나 대형주에 적합합니다.",
    }
    params = [
        Param("entry_ibs", "매수 IBS 기준", 0.2, 0.05, 0.5, step=0.05, integer=False),
        Param("exit_ibs", "청산 IBS 기준", 0.8, 0.5, 0.95, step=0.05, integer=False),
        Param("trend_ma", "추세 이평 (0=끔)", 200, 0, 400),
        Param("max_hold", "최대 보유일", 5, 1, 30),
    ]

    def validate(self) -> None:
        if self.p["entry_ibs"] >= self.p["exit_ibs"]:
            raise StrategyParamError("매수 IBS 기준은 청산 IBS 기준보다 작아야 합니다")

    def warmup(self) -> int:
        return max(self.p["trend_ma"], 2)

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        self.ibs = [
            0.5 if h <= l else (c - l) / (h - l)
            for h, l, c in zip(bars.high, bars.low, bars.close)
        ]
        self.trend = ind.sma(bars.close, self.p["trend_ma"]) if self.p["trend_ma"] else None

    def on_bar(self, i: int, ctx: Context) -> list:
        ibs = self.ibs[i]
        if ctx.position.is_open:
            held = i - ctx.position.entry_index if ctx.position.entry_index >= 0 else 0
            if ibs > self.p["exit_ibs"]:
                return [Order("sell", reason=f"IBS {ibs:.2f} > {self.p['exit_ibs']:g}")]
            if held >= self.p["max_hold"]:
                return [Order("sell", reason=f"최대 보유일({self.p['max_hold']}일) 청산")]
            return []
        trend_ok = self.trend is None or (self.trend[i] is not None and self.close[i] > self.trend[i])
        if trend_ok and ibs < self.p["entry_ibs"]:
            return [Order("buy", reason=f"IBS {ibs:.2f} < {self.p['entry_ibs']:g}")]
        return []

    def chart_indicators(self) -> list:
        if not self.p["trend_ma"]:
            return []
        return [{"id": "ma", "params": {"p1": self.p["trend_ma"], "p2": 0, "p3": 0, "p4": 0}}]
