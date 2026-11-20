from __future__ import annotations

import math

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class TimeSeriesMomentum(Strategy):
    id = "ts_momentum"
    name = "시계열 모멘텀 (TSMOM)"
    category = "swing"
    timeframe = "1d"
    theory = "모스코위츠·오이·페데르센 · 오르는 중이면 타고, 시장이 불안하면 적게 넣기"
    summary = "몇 달 전보다 올랐으면 투자하고 내렸으면 현금 보유, 흔들림이 클수록 비중을 줄입니다"
    description = (
        "세계 주요 주식·채권·원자재 시장 수십 년 데이터를 분석해 입증된 전략입니다. '석 달 전보다 가격이 높으면 상승 중'이라고 보고 "
        "매수하되, 주가가 평소보다 심하게 출렁일 때는 자동으로 돈의 일부만 넣고 나머지는 현금으로 남겨 큰 손실을 막습니다."
    )
    guide = {
        "background": (
            "2012년 세계적 금융 학술지에 발표된 '시계열 모멘텀' 연구에 바탕을 둡니다. 비유하자면 바다 날씨를 보고 배에 짐을 싣는 것과 같습니다. "
            "① 순풍이 불고 있으면(과거 60일 수익률이 플러스) 배를 띄우고, 역풍(마이너스)이면 항구(현금)로 돌아옵니다. "
            "② 파도(가격 출렁임)가 잔잔하면 짐을 100% 싣지만, 파도가 2배로 거칠어지면 짐을 50%만 실어 배가 뒤집히지 않게 지킵니다."
        ),
        "setup": [
            "모멘텀 기간: 기본 60일(약 석 달 전과 비교). 더 묵직한 장기 추세를 보려면 120일~252일(6개월~1년)로 늘립니다.",
            "변동성 기간: 기본 20일(최근 한 달간 주가가 얼마나 출렁였는지 측정).",
            "목표 연변동성(%): 기본 15%(안정형). 좀 더 공격적으로 비중을 크게 넣고 싶으면 20~30%로 높입니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤 어제 마감 가격을 60일 전 가격과 비교하고, 최근 한 달간의 출렁임 크기를 계산합니다.",
            "안 가지고 있을 때: 60일 전보다 가격이 높다면, '목표 출렁임 ÷ 실제 출렁임' 비율만큼만(최대 100%) 시장가로 매수합니다.",
            "가지고 있을 때: 60일 전보다 가격이 낮아지면(마이너스 전환) 전량 시장가로 팔아 현금으로 피신합니다.",
        ],
        "caution": "출렁임이 클 때 일부 현금을 남겨두므로, 폭등장에서는 전액 몰빵했을 때보다 수익이 조금 덜할 수 있습니다. 대신 하락장에서 훨씬 덜 잃습니다.",
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
