from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class Rsi2MeanReversion(Strategy):
    id = "rsi2"
    name = "RSI(2) 평균회귀 (래리 코너스)"
    category = "swing"
    timeframe = "1d"
    theory = "래리 코너스 · 장기 추세 내 단기 과매도 평균회귀"
    summary = "장기 상승 추세 속 단기 급락(RSI(2) 과매도)에 매수, 단기 이평 회복 시 매도"
    description = (
        "래리 코너스의 단기 평균회귀 전략. 종가가 장기 이평(기본 200일) 위인 상승 추세에서만, 초단기 RSI(2)가 "
        "기준값(기본 10) 아래로 떨어지면 다음 날 시가에 매수합니다. 종가가 단기 이평(기본 5일)을 넘으면 다음 날 시가에 "
        "매도합니다. 승률이 높고 보유 기간이 짧지만, 한 번의 큰 하락에 손실이 커질 수 있습니다."
    )
    guide = {
        "background": (
            "래리 코너스(Larry Connors)의 저서 'Short Term Trading Strategies That Work'에서 제시된 대표적 단기 평균회귀 전략입니다. "
            "장기 상승 추세(200일선 위)에 있는 자산이 단기적인 패닉·차익실현으로 2~3일 급락(초단기 2일 RSI < 10)할 때 "
            "과잉반응이 빠르게 되돌려지는 통계적 우위(높은 승률과 짧은 자금 노출 기간)를 활용합니다."
        ),
        "setup": [
            "RSI 기간(rsi_n): 기본 2일. 2~3일이 핵심이며 길어지면 일반 추세/스윙 성격으로 변합니다.",
            "매수 기준(entry): 기본 10. 더 극단적인 급락만 노릴 때는 5, 진입 빈도를 높이려면 15~20으로 조정합니다.",
            "추세 이평(trend): 기본 200일(강세장 필터). 하락장 진입을 막는 가장 중요한 안전장치이므로 끄지 않는 것을 권장합니다.",
            "청산 이평(exit_ma): 기본 5일. 빠른 회전은 5일, 반등을 더 길게 보려면 7~10일을 사용합니다.",
        ],
        "execution": [
            "매일 정규장 시작 1분 뒤 전일 종가 기준으로 RSI(2), 200일 추세 이평, 5일 청산 이평을 계산합니다.",
            "미보유 시: 전일 종가 > 200일 이평 AND RSI(2) < 10이면 당일 시장가 매수합니다.",
            "보유 시: 전일 종가 > 5일 이평으로 단기 반등에 성공하면 당일 시장가 매도로 청산합니다.",
        ],
        "caution": "승률(보통 65~75%)은 높지만 손절선이 없으므로 200일선을 깨는 급락장이 시작될 때 단 한 번의 큰 손실(꼬리 위험)이 날 수 있습니다. 퀀트 랩 '몬테카를로'로 최악 MDD를 꼭 확인하세요.",
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
