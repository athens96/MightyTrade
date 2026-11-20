from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class VwapBandReversion(Strategy):
    id = "vwap_reversion"
    name = "VWAP 밴드 평균회귀 (단타)"
    category = "daytrade"
    timeframe = "5m"
    theory = "기관 체결 벤치마크 · 장중 VWAP 표준편차 이탈 회귀 (5분봉 · 당일 청산)"
    summary = "5분봉 가격이 당일 VWAP − K×σ 하단 이탈 시 매수, VWAP 회복 또는 손절·마감 청산"
    description = (
        "기관 알고리즘의 체결 기준가인 당일 VWAP(거래량 가중 평균가) 주변으로 가격이 회귀하는 "
        "장중 평균회귀 이론을 적용한 5분봉 단타 전략. 개장 초반 표본 축적 후 종가가 VWAP 하단 밴드(−Kσ) 아래로 "
        "과매도 이탈하면 매수하고, VWAP 회복 시 익절하거나 손절(%)·장 마감 전 당일 청산합니다."
    )
    params = [
        Param("band_k", "하단 밴드 배수 (σ)", 1.5, 0.5, 4.0, step=0.1, integer=False),
        Param("min_bars", "장 초반 대기 봉 수 (5m)", 4, 2, 30),
        Param("stop_pct", "손절 (%)", 1.0, 0.2, 5.0, step=0.1, integer=False),
    ]

    def warmup(self) -> int:
        return self.p["min_bars"] + 1

    def prepare(self, bars: Bars) -> None:
        self.b = bars
        v = ind.intraday_vwap(bars.time, bars.high, bars.low, bars.close, bars.volume)
        self.vwap, self.sd, self.sess = v["vwap"], v["sd"], v["session"]
        self.bar_in_sess = []
        prev_s = -1
        idx = 0
        for s in self.sess:
            idx = 0 if s != prev_s else idx + 1
            prev_s = s
            self.bar_in_sess.append(idx)

    def on_bar(self, i: int, ctx: Context) -> list:
        pos = ctx.position
        next_is_new_sess = i + 1 < len(self.b) and self.sess[i + 1] != self.sess[i]
        vwap, sd = self.vwap[i], self.sd[i]
        c = self.b.close[i]

        if pos.is_open:
            if next_is_new_sess:
                return [Order("sell", reason="장 마감 전 당일 청산")]
            if vwap is not None and c >= vwap:
                return [Order("sell", reason="VWAP 중심선 회복 익절")]
            stop = pos.entry_price * (1.0 - self.p["stop_pct"] / 100.0)
            return [Order("sell", kind="stop", stop=stop, reason=f"VWAP 손절 (-{self.p['stop_pct']:g}%)")]

        if next_is_new_sess or self.bar_in_sess[i] < self.p["min_bars"]:
            return []
        if vwap is None or sd is None or sd <= 0:
            return []
        lower = vwap - self.p["band_k"] * sd
        if c < lower:
            return [Order("buy", reason=f"VWAP -{self.p['band_k']:g}σ 이탈 ({c:,.2f} < {lower:,.2f})")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "vwap", "params": {}}]
