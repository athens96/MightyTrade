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
