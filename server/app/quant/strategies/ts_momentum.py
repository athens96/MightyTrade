from __future__ import annotations

import math

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class TimeSeriesMomentum(Strategy):
    id = "ts_momentum"
    name = "시계열 모멘텀 (TSMOM)"
    category = "swing"
    timeframe = "1d"
    theory = "모스코위츠·오이·페데르센 (JFE 2012) · 절대 모멘텀 & 변동성 타겟팅"
    summary = "과거 N일 수익률이 양수면 목표 변동성 비중으로 매수, 음수 전환 시 전량 청산"
    description = (
        "Moskowitz, Ooi & Pedersen(2012)의 시계열 모멘텀(TSMOM) 이론을 구현한 중기 스윙 전략. "
        "과거 룩백 기간(기본 60거래일) 수익률이 플러스면 상승 모멘텀이 지속될 확률이 높다고 보고 진입하며, "
        "최근 실현 변동성에 반비례해 투자 비중(목표 변동성 ÷ 실현 변동성, 최대 100%)을 조절해 급변동 장세의 낙폭을 완화합니다."
    )
    guide = {
        "background": (
            "토비아스 모스코위츠·야오 후아 오이·라세 페데르센이 2012년 Journal of Financial Economics에 발표한 "
            "'Time Series Momentum' 논문(58개 선물·주식·채권·원자재 시장 수십 년 실증)에 기반합니다. "
            "자산 자체의 과거 수익률 방향이 미래 수익률과 양의 자기상관을 가지며, 여기에 '변동성 타겟팅(Volatility Scaling)'을 결합하면 "
            "위기 국면의 극단적 손실을 크게 줄이면서 샤프비율을 개선한다는 것이 핵심 결론입니다."
        ),
        "setup": [
            "모멘텀 기간(lookback): 스윙용은 40~60일(2~3개월), 중장기 추세용은 120~252일(6~12개월)을 권장합니다.",
            "변동성 기간(vol_window): 기본 20거래일(최근 1개월 일간 수익률의 연율 변동성).",
            "목표 연변동성(target_vol): 보수적 10~15%, 적극적 20~30%. 종목 변동성이 목표치의 2배이면 자본의 50%만 투입합니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 종가 기준 lookback일 수익률과 최근 vol_window일 연율 변동성을 계산합니다.",
            "미보유 상태에서 lookback일 수익률 > 0이면 비중 = min(100%, 목표변동성 ÷ 실현변동성)으로 수량을 계산해 시장가 매수합니다.",
            "보유 중 lookback일 수익률이 0 이하로 꺾이면 즉시 시장가 전량 매도하고 현금으로 대피합니다.",
        ],
        "caution": "횡보장에서 수익률이 0% 선을 자주 넘나들면 매매가 잦아질 수 있으며, 변동성 축소 시 자금 일부가 현금으로 남아 상승장 절대 수익률은 풀투자 대비 낮을 수 있습니다.",
    }
    params = [
        Param("lookback", "모멘텀 기간 (일)", 60, 10, 252),
        Param("vol_window", "변동성 기간 (일)", 20, 5, 120),
        Param("target_vol", "목표 연변동성 (%)", 15, 5, 60, step=1, integer=False),
    ]

    def warmup(self) -> int:
        return max(self.p["lookback"], self.p["vol_window"]) + 1

    def prepare(self, bars: Bars) -> None:
        self.close = bars.close
        rets = [0.0] + [
            (bars.close[i] / bars.close[i - 1] - 1.0) if bars.close[i - 1] > 0 else 0.0
            for i in range(1, len(bars.close))
        ]
        sd = ind.stdev(rets, self.p["vol_window"])
        self.ann_vol = [None if s is None else s * math.sqrt(252) for s in sd]

    def on_bar(self, i: int, ctx: Context) -> list:
        lb = self.p["lookback"]
        if i < lb or self.close[i - lb] <= 0:
            return []
        mom = self.close[i] / self.close[i - lb] - 1.0
        if ctx.position.is_open:
            if mom <= 0:
                return [Order("sell", reason=f"{lb}일 모멘텀 음수 전환 ({mom * 100:+.1f}%)")]
            return []
        vol = self.ann_vol[i]
        if mom > 0 and vol and vol > 0 and self.close[i] > 0:
            weight = min(1.0, (self.p["target_vol"] / 100.0) / vol)
            qty = math.floor(ctx.equity * weight / self.close[i])
            if qty > 0:
                return [Order("buy", qty=qty, reason=f"{lb}일 모멘텀 {mom * 100:+.1f}% · 비중 {weight * 100:.0f}%")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "ma", "params": {"p1": self.p["lookback"], "p2": 0, "p3": 0, "p4": 0}}]
