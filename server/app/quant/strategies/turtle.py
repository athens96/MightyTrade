from __future__ import annotations

import math

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class Turtle(Strategy):
    id = "turtle"
    name = "터틀 트레이딩"
    category = "swing"
    timeframe = "1d"
    theory = "리처드 데니스 · 최근 최고가 돌파 + 하루 흔들림 크기에 맞춘 안전 수량 조절"
    summary = "최근 20일 최고가를 뚫으면 사고, 최근 10일 최저가나 손절선 깨지면 팝니다"
    description = (
        "1980년대 일반인들을 훈련시켜 큰 수익을 낸 유명한 '터틀 트레이딩' 규칙입니다. 최근 20일 중 가장 비쌌던 가격을 뚫고 올라가면 "
        "강한 상승 시작으로 보고 매수합니다. 특히 '하루에 주가가 얼마나 출렁이는지(ATR)'를 계산해서, 출렁임이 큰 종목은 적게 사고 "
        "잔잔한 종목은 더 사는 안전장치가 들어 있습니다."
    )
    guide = {
        "background": (
            "1983년 유명 투자자 리처드 데니스가 '누구라도 정해진 규칙만 지키면 성공할 수 있을까?'를 실험하며 공개한 전략입니다. "
            "핵심은 두 가지입니다. ① 최근 20일 최고가를 뚫었다는 건 그만큼 사고 싶어하는 힘이 세다는 뜻이므로 과감히 따라 삽니다. "
            "② 종목마다 하루에 움직이는 폭(평균 진폭, ATR)이 다르므로, 많이 출렁이는 종목은 소량만 사서 한 번 틀려도 전체 돈의 1~2% 이상 잃지 않게 조절합니다."
        ),
        "setup": [
            "진입/청산 기간: 기본값 20일 최고가 매수 / 10일 최저가 매도면 충분합니다. 더 길게 보고 싶으면 55일 / 20일을 씁니다.",
            "손절 배수(N): 기본 2배. 내가 산 가격에서 '하루 평균 출렁임 × 2'만큼 떨어지면 미련 없이 손절합니다.",
            "1N당 위험(%): 기본 1~2% 추천. 이 숫자가 클수록 한 번에 사는 주식 수가 늘어나 수익도 크지만 손실도 커집니다.",
            "운용 자금은 최소 10~20주 이상 살 수 있는 금액으로 넣어야 수량 조절 기능이 제대로 작동합니다.",
        ],
        "execution": [
            "매일 장 시작 1분 뒤, 최근 20일 최고가·10일 최저가·하루 평균 출렁임(ATR)을 계산합니다.",
            "안 가지고 있을 때: 안전 수량을 자동 계산한 뒤, '오늘 가격이 20일 최고가를 넘으면 자동으로 사라'는 예약(조건주문)을 걸어둡니다.",
            "가지고 있을 때: '10일 최저가'와 '내 매수가 − 손절폭' 중 더 높은 가격에 자동 매도 예약(조건주문)을 매일 걸어둡니다.",
            "처음 살 때의 출렁임(ATR) 값을 봇이 기억해 두므로, 몇 달 오래 들고 있어도 원래 정한 손절 기준이 틀어지지 않습니다.",
        ],
        "caution": "10번 중 6~7번은 작게 손절하고, 나머지 3~4번의 큰 상승에서 수익을 몰아 버는 구조입니다. 몇 번 연속 손절했다고 규칙을 중간에 끄면 안 됩니다.",
    }
    params = [
        Param("entry", "진입 기간", 20, 5, 200),
        Param("exit", "청산 기간", 10, 2, 200),
        Param("atr_n", "ATR(N) 기간", 20, 5, 100),
        Param("stop_atr", "손절 배수 (N)", 2, 0.5, 10, step=0.5, integer=False),
        Param("risk_pct", "1N당 위험 (% 자산)", 1, 0.1, 10, step=0.1, integer=False),
    ]

    def validate(self) -> None:
        if self.p["exit"] >= self.p["entry"]:
            raise StrategyParamError("청산 기간은 진입 기간보다 짧아야 합니다")

    def warmup(self) -> int:
        return max(self.p["entry"], self.p["atr_n"]) + 1

    def prepare(self, bars: Bars) -> None:
        dc = ind.donchian(bars.high, bars.low, self.p["entry"], self.p["exit"])
        self.hh, self.ll = dc["upper"], dc["lower"]
        self.atr = ind.atr(bars.high, bars.low, bars.close, self.p["atr_n"])

    def on_bar(self, i: int, ctx: Context) -> list:
        pos = ctx.position
        if pos.is_open:
            n_entry = pos.entry_atr
            if n_entry is None and 0 < pos.entry_index <= len(self.atr):
                n_entry = self.atr[pos.entry_index - 1]
            hard_stop = pos.entry_price - self.p["stop_atr"] * n_entry if n_entry else None
            channel = self.ll[i]
            if hard_stop is None and channel is None:
                return []
            if hard_stop is None or (channel is not None and channel >= hard_stop):
                level, why = channel, f"{self.p['exit']}일 신저가"
            else:
                level, why = hard_stop, f"{self.p['stop_atr']:g}N 손절"
            return [Order("sell", kind="stop", stop=level, reason=why)]
        hh, n = self.hh[i], self.atr[i]
        if hh is None or not n:
            return []
        qty = math.floor(ctx.equity * self.p["risk_pct"] / 100 / n)
        if qty <= 0:
            return []
        return [Order("buy", kind="stop", stop=hh, qty=qty, reason=f"{self.p['entry']}일 신고가 돌파")]

    def chart_indicators(self) -> list:
        return [{"id": "donchian", "params": {"upper": self.p["entry"], "lower": self.p["exit"]}},
                {"id": "atr", "params": {"n": self.p["atr_n"]}}]
