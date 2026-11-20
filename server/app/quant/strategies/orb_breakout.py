from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class OpeningRangeBreakout(Strategy):
    id = "orb_breakout"
    name = "오프닝 레인지 돌파 (ORB)"
    category = "daytrade"
    timeframe = "15m"
    theory = "토비 크래벨 · 장 초반 30분 힘겨루기에서 이긴 방향으로 따라가기 (당일 청산)"
    summary = "장 시작 후 첫 30분 최고가를 뚫으면 매수, 목표가·손절가 또는 마감 10분 전 전량 청산"
    description = (
        "유명 단타 교과서에 나오는 '오프닝 레인지 돌파(ORB)' 전략입니다. 장이 열리고 첫 30분(15분봉 2개) 동안 주가가 오르내린 "
        "최고가와 최저가를 기준선으로 삼고, 그 위를 강하게 뚫으면 매수합니다. 밤사이 폭락 걱정이 없도록 장 마감 10분 전에 반드시 다 팝니다."
    )
    guide = {
        "background": (
            "장이 열리고 처음 15~30분은 전날 밤 뉴스를 본 투자자들이 한꺼번에 매수·매도를 쏟아내며 가장 치열하게 줄다리기를 하는 시간입니다. "
            "이 첫 30분 동안 만들어진 최고가(천장)를 뚫고 올라간다는 것은 '오늘 줄다리기에서 사려는 쪽이 이겼다'는 뜻입니다. "
            "그 방향으로 따라가서 첫 30분 폭의 1.5배만큼 오르면 익절하고, 장 끝나기 전에 모두 팔아 다음 날 아침 갭하락 걱정을 없앱니다."
        ),
        "setup": [
            "오프닝 봉 수(15분봉): 기본 2봉(장 시작 후 30분). 더 빠르게 들어가려면 1봉(15분)으로 줄입니다.",
            "목표 손익비(R): 기본 1.5배(첫 30분 폭의 1.5배 오르면 익절). 강한 종목은 2.0배로 키울 수 있습니다.",
            "손절 배수: 기본 1.0배(첫 30분 폭만큼 빠지면 손절). 손실을 더 짧게 자르려면 0.5~0.75배로 줄입니다.",
            "거래가 활발하고 하루 안에 방향이 시원하게 나오는 대형 주도주나 대표 지수 ETF에 잘 맞습니다.",
        ],
        "execution": [
            "정규장 동안 봇이 자동으로 대기하다가 새로운 15분봉이 끝날 때마다 계산합니다.",
            "장 시작 후 초반 30분(2봉)의 최고가와 최저가를 확정합니다.",
            "그 이후 안 가지고 있으면 최고가 돌파 자동 매수 예약을 걸고, 사지면 목표가 익절·손절선을 자동으로 감시합니다.",
            "장 마감 10분 전이 되면 남은 예약을 취소하고 들고 있는 주식을 시장가로 전부 팔아 그날 매매를 끝냅니다.",
        ],
        "caution": "장 초반부터 주가가 너무 크게 출렁였거나, 하루 종일 방향 없이 위아래로만 흔들리는 날에는 손절이 나올 수 있습니다. 단타는 거래비용 영향이 크니 소액 모의 테스트로 먼저 확인하세요.",
    }
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
