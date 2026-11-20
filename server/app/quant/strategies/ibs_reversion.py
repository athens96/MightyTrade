from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class IbsMeanReversion(Strategy):
    id = "ibs_reversion"
    name = "IBS 내부봉 평균회귀"
    category = "swing"
    timeframe = "1d"
    theory = "파고니디스 · 단기 종가 위치(Internal Bar Strength) 평균회귀"
    summary = "상승 추세 속 당일 저점 부근 마감(IBS < 기준)에 매수, 고점 부근 반등 시 매도"
    description = (
        "IBS = (종가 − 저가) ÷ (고가 − 저가)로 당일 변동폭 내 종가 위치(0~1)를 측정하는 학술 검증 스윙 전략. "
        "장기 상승 추세(기본 200일 이평 위)에서 일시적 매도 압력으로 당일 저가 부근(IBS < 0.2)에 마감하면 "
        "단기 과잉반응으로 보고 다음 날 시가에 매수하며, 고가 부근(IBS > 0.8) 반등 또는 최대 보유일 도달 시 청산합니다."
    )
    guide = {
        "background": (
            "알렉산더 파고니디스(Alexander Pagonidis, 2013)와 Quantpedia 연구에서 광범위하게 검증된 'Internal Bar Strength(IBS) 효과'입니다. "
            "IBS = (종가 − 저가) / (고가 − 저가)는 장중 매수/매도 압력의 비대칭을 나타내며, 유동성 공급자(마켓메이커)가 장 마감 직전 "
            "쏠린 매도 물량을 받아낸 뒤 다음 며칠간 재고를 해소하며 가격이 반등하는 단기 유동성 프리미엄을 포착합니다."
        ),
        "setup": [
            "매수/청산 IBS: 기본 0.2 / 0.8 (당일 레인지 하위 20% 마감 시 매수, 상위 20% 반등 시 청산).",
            "추세 이평(trend_ma): 기본 200일. 강세장 우량 ETF(SPY, QQQ, KODEX 200 등)에서 가장 안정적입니다.",
            "최대 보유일(max_hold): 기본 5거래일. 반등이 나오지 않더라도 시간 손절로 자금 묶임을 방지합니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전 거래일의 고가·저가·종가로 IBS와 추세 이평을 계산합니다.",
            "미보유 시: 전일 종가 > 추세 이평이고 전일 IBS < 0.2이면 당일 시장가 매수합니다.",
            "보유 시: 전일 IBS > 0.8로 강하게 반등 마감했거나 보유 거래일이 최대 보유일에 도달하면 당일 시장가 매도합니다.",
        ],
        "caution": "단기 매매가 잦은 편이므로 거래세·수수료·슬리피지가 높은 소형주보다는 거래비용이 낮은 ETF·대형주에서 효과가 뚜렷합니다.",
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
