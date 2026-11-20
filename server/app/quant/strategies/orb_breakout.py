from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class OpeningRangeBreakout(Strategy):
    id = "orb_breakout"
    name = "오프닝 레인지 돌파 (ORB)"
    category = "daytrade"
    timeframe = "15m"
    theory = "토비 크래벨 · 장 초반 변동성 레인지 돌파 (15분봉 · 당일 청산)"
    summary = "15분봉 장 초반 N개 봉의 고가 돌파 시 매수, 손익비 목표/손절 또는 장 마감 청산"
    description = (
        "토비 크래벨(Toby Crabel)의 고전 단타 이론인 Opening Range Breakout(ORB) 전략. "
        "장 시작 후 초반 N개 15분봉(기본 2봉=30분)의 고가·저가로 오프닝 레인지를 확정한 뒤, "
        "상단 돌파 시 추세 방향으로 진입합니다. 레인지 폭 대비 손절 배수·목표 손익비(R)를 적용하며, "
        "장 마감 전 강제 청산해 오버나이트 갭 리스크를 차단합니다."
    )
    params = [
        Param("open_bars", "오프닝 봉 수 (15m)", 2, 1, 8),
        Param("target_rr", "목표 손익비 (R)", 1.5, 0.5, 5.0, step=0.25, integer=False),
        Param("stop_ratio", "손절 레인지 배수", 1.0, 0.25, 2.0, step=0.25, integer=False),
    ]

    def warmup(self) -> int:
        return self.p["open_bars"] + 1

    def prepare(self, bars: Bars) -> None:
        self.b = bars
        sess = ind.session_buckets(bars.time)
        n = len(bars)
        self.sess = sess
        self.bar_in_sess = [0] * n
        self.is_eod = [False] * n
        self.or_high = [None] * n
        self.or_low = [None] * n
        k = self.p["open_bars"]
        cur_s = -1
        idx = 0
        hi, lo = 0.0, 0.0
        for i in range(n):
            if sess[i] != cur_s:
                if i > 0:
                    self.is_eod[i - 1] = True
                cur_s = sess[i]
                idx = 0
                hi, lo = bars.high[i], bars.low[i]
            else:
                idx += 1
                if idx < k:
                    hi = max(hi, bars.high[i])
                    lo = min(lo, bars.low[i])
            self.bar_in_sess[i] = idx
            if idx >= k - 1:
                self.or_high[i] = hi
                self.or_low[i] = lo

    def on_bar(self, i: int, ctx: Context) -> list:
        pos = ctx.position
        # Flatten right before session roll (no overnight holding).
        next_is_new_sess = i + 1 < len(self.b) and self.sess[i + 1] != self.sess[i]
        if pos.is_open:
            if next_is_new_sess or self.is_eod[i]:
                return [Order("sell", reason="장 마감 전 당일 청산")]
            oh, ol = self.or_high[i], self.or_low[i]
            rng = (oh - ol) if (oh is not None and ol is not None and oh > ol) else pos.entry_price * 0.01
            target = pos.entry_price + rng * self.p["target_rr"]
            stop = pos.entry_price - rng * self.p["stop_ratio"]
            if self.b.close[i] >= target:
                return [Order("sell", reason=f"ORB 목표 익절 ({self.p['target_rr']:g}R)")]
            return [Order("sell", kind="stop", stop=stop, reason=f"ORB 손절 ({self.p['stop_ratio']:g}R)")]

        if next_is_new_sess or self.is_eod[i]:
            return []
        oh, ol = self.or_high[i], self.or_low[i]
        if oh is None or ol is None or oh <= ol:
            return []
        if self.bar_in_sess[i] >= self.p["open_bars"] - 1 and self.b.close[i] < oh:
            return [Order("buy", kind="stop", stop=oh, reason=f"ORB {self.p['open_bars']}봉 고점({oh:,.2f}) 돌파")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "vwap", "params": {}}]
