from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class MacdTrendSwing(Strategy):
    id = "macd_trend"
    name = "MACD 추세 스윙"
    category = "swing"
    timeframe = "1d"
    theory = "제럴드 아펠 · 상승 속도가 붙을 때 사고, 고점에서 밀리면 수익 지키며 매도"
    summary = "큰 흐름이 상승일 때 MACD 상승 신호에 사고, 하락 신호나 추적 손절선 깨지면 팝니다"
    description = (
        "차트에서 가장 많이 쓰이는 MACD 지표에 두 가지 안전장치를 더한 스윙 전략입니다. ① 주가가 120일 평균 위에 있는 좋은 장세에서만 "
        "매수해서 하락장 함정을 피하고, ② 주가가 오를수록 손절선도 같이 따라 올라가(추적 손절) 벌어 둔 수익을 지키며 빠져나옵니다."
    )
    guide = {
        "background": (
            "MACD는 '주가가 얼마나 빠른 속도로 오르고 있는지'를 재는 속도계 같은 지표입니다. 최근 12일 흐름과 26일 흐름의 차이가 커지면 "
            "가속도가 붙었다고 봅니다. 여기에 '큰 흐름이 좋을 때만 들어간다(120일선 필터)'와 '주가가 최고점을 찍고 일정 폭 이상 꺾이면 바로 수익을 챙긴다(추적 손절)' "
            "두 가지 실전 규칙을 합쳐서 초보자도 안정적으로 추세 수익을 노릴 수 있게 만들었습니다."
        ),
        "setup": [
            "단기/장기/시그널: 전 세계 표준인 기본값 12 / 26 / 9를 그대로 쓰시면 됩니다.",
            "추세 이평: 기본 120일(약 6개월 평균). 이 선 아래에 있을 때는 아예 사지 않아 하락장을 피해 갑니다.",
            "ATR 추적 손절: 기본 2.5배. 주가가 오르면 손절선도 최고가 밑으로 따라 올라가며, 너무 작게(예: 1.0) 하면 살짝 흔들릴 때도 팔려버립니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제 마감 가격으로 MACD, 120일 평균, 하루 평균 출렁임(ATR)을 계산합니다.",
            "안 가지고 있을 때: 주가가 120일 평균 위에 있고 어제 MACD 상승 신호가 떴다면 오늘 시장가로 매수합니다.",
            "가지고 있을 때: MACD 하락 신호가 뜨면 오늘 바로 시장가로 팔아 수익을 확정합니다.",
            "하락 신호가 없더라도 '보유 중 최고가 − (출렁임 × 2.5)' 가격에 자동 매도 예약(조건주문)을 매일 올려가며 수익을 지킵니다.",
        ],
        "caution": "MACD는 주가가 이미 조금 오른 뒤에 신호가 나오는 편입니다. 갑자기 급등했다가 하루 만에 급락하는 테마주에서는 손절이 잦을 수 있습니다.",
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
