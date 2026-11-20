from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class MacdTrendSwing(Strategy):
    id = "macd_trend"
    name = "MACD 추세 스윙"
    category = "swing"
    timeframe = "1d"
    theory = "제럴드 아펠 · MACD 모멘텀 수렴확산 + 추세 필터 + ATR 추적 손절"
    summary = "장기 추세 위에서 MACD 골든크로스 매수, 데드크로스 또는 ATR 추적 손절 시 매도"
    description = (
        "제럴드 아펠의 MACD 모멘텀 이론에 추세 필터와 변동성 기반 추적 손절을 결합한 스윙 전략. "
        "종가가 장기 이동평균(기본 120일) 위인 상승 추세에서 MACD선이 시그널선을 상향 돌파하면 매수합니다. "
        "데드크로스가 나오거나 보유 중 최고가 대비 ATR×손절배수만큼 하락하면 청산해 추세 수익을 지키며 손실을 제한합니다."
    )
    params = [
        Param("fast", "단기 EMA", 12, 2, 50),
        Param("slow", "장기 EMA", 26, 5, 100),
        Param("signal", "시그널", 9, 2, 50),
        Param("trend_ma", "추세 이평 (0=끔)", 120, 0, 300),
        Param("atr_stop", "ATR 추적 손절 (0=끔)", 2.5, 0, 10, step=0.5, integer=False),
    ]

    def validate(self) -> None:
        if self.p["fast"] >= self.p["slow"]:
            raise StrategyParamError("단기 EMA는 장기 EMA보다 짧아야 합니다")

    def warmup(self) -> int:
        return max(self.p["slow"] + self.p["signal"], self.p["trend_ma"], 15)

    def prepare(self, bars: Bars) -> None:
        self.b = bars
        m = ind.macd(bars.close, self.p["fast"], self.p["slow"], self.p["signal"])
        self.macd, self.sig = m["macd"], m["signal"]
        self.trend = ind.sma(bars.close, self.p["trend_ma"]) if self.p["trend_ma"] else None
        self.atr = ind.atr(bars.high, bars.low, bars.close, 14)

    def on_bar(self, i: int, ctx: Context) -> list:
        if i < 1:
            return []
        m0, s0, m1, s1 = self.macd[i - 1], self.sig[i - 1], self.macd[i], self.sig[i]
        if None in (m0, s0, m1, s1):
            return []
        if ctx.position.is_open:
            if m0 >= s0 and m1 < s1:
                return [Order("sell", reason="MACD 데드크로스")]
            if self.p["atr_stop"] > 0 and self.atr[i]:
                start = max(ctx.position.entry_index, 0) if ctx.position.entry_index >= 0 else max(i - 20, 0)
                peak = max(self.b.high[start : i + 1]) if start <= i else self.b.high[i]
                trail = peak - self.p["atr_stop"] * self.atr[i]
                return [Order("sell", kind="stop", stop=trail, reason=f"ATR 추적 손절 ({self.p['atr_stop']:g}N)")]
            return []
        trend_ok = self.trend is None or (self.trend[i] is not None and self.b.close[i] > self.trend[i])
        if trend_ok and m0 <= s0 and m1 > s1:
            return [Order("buy", reason=f"MACD 골든크로스 ({self.p['fast']}/{self.p['slow']}/{self.p['signal']})")]
        return []

    def chart_indicators(self) -> list:
        out = [{"id": "macd", "params": {"fast": self.p["fast"], "slow": self.p["slow"], "signal": self.p["signal"]}}]
        if self.p["trend_ma"]:
            out.insert(0, {"id": "ma", "params": {"p1": self.p["trend_ma"], "p2": 0, "p3": 0, "p4": 0}})
        return out
