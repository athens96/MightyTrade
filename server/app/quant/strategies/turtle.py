from __future__ import annotations

import math

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy, StrategyParamError


class Turtle(Strategy):
    id = "turtle"
    name = "터틀 트레이딩"
    category = "swing"
    timeframe = "1d"
    theory = "리처드 데니스 · 돈치안 채널 돌파 & ATR 리스크 사이징"
    summary = "n일 신고가 돌파 시 매수, m일 신저가 또는 2N 손절 시 매도. ATR로 비중 조절"
    description = (
        "리처드 데니스의 터틀 시스템(시스템 1 기반, 롱 전용·추가매수 없음). 장중 최근 진입 기간(기본 20일) 최고가를 "
        "돌파하면 매수합니다. 최근 청산 기간(기본 10일) 최저가를 깨거나, 진입가 − 손절배수×N 아래로 내려가면 매도합니다. "
        "N은 ATR(평균 진폭)입니다. 매수 수량 = 자산 × 1N당 위험(%) ÷ N 이라서, 변동성이 큰 종목일수록 적게 삽니다. "
        "승률은 낮지만 큰 추세 하나로 수익을 내는 구조입니다."
    )
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
