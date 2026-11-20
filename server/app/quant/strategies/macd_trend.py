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
    guide = {
        "background": (
            "1979년 제럴드 아펠(Gerald Appel)이 고안한 MACD(Moving Average Convergence Divergence)는 단기 EMA(12)와 장기 EMA(26)의 괴리로 "
            "추세의 '가속도(모멘텀)'를 측정합니다. 여기에 장기 추세 필터(120일선)를 결합해 하락장 역추세 진입을 차단하고, "
            "척 르보(Chuck LeBeau)의 샹들리에 출구(Chandelier Exit: 최고가 − k×ATR)로 추세 고점 부근의 수익 반납을 줄인 실전 스윙 모델입니다."
        ),
        "setup": [
            "MACD 기본(12/26/9)이 가장 보편적이며, 빠른 스윙은 8/17/9, 느린 중기 스윙은 19/39/9를 테스트해 보세요.",
            "추세 이평(trend_ma): 기본 120일(약 6개월). 강세장 종목은 60일, 보수적 필터는 200일로 설정합니다(0=필터 해제).",
            "ATR 추적 손절(atr_stop): 2.0~3.0N이 적당합니다. 너무 작으면 정상적인 눌림목에 조기 청산됩니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 종가 기준 MACD·시그널·장기 이평·ATR(14)을 산출합니다.",
            "미보유 상태에서 종가 > 추세 이평이고 어제 MACD가 시그널을 상향 돌파했으면 당일 시장가 매수합니다.",
            "보유 중 MACD 데드크로스 발생 시 시장가로 즉시 청산합니다.",
            "데드크로스가 없더라도 보유 중 최고가 − (ATR × 손절배수)에 당일 유효 매도 조건주문을 매일 갱신(트레일링)합니다.",
        ],
        "caution": "MACD는 후행 지표이므로 급등 후 급락하는 V자 변동성 종목에서는 진입이 늦고 추적 손절에 자주 걸릴 수 있습니다.",
    }
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
