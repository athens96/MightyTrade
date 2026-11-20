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
    guide = {
        "background": (
            "1990년 토비 크래벨(Toby Crabel)의 명저 'Day Trading with Short Term Price Patterns and Opening Range Breakout'에서 "
            "정립된 대표적 장중 돌파 이론입니다. 개장 직후 15~30분은 밤사이 누적된 뉴스와 기관·개인 주문이 충돌하며 당일의 초기 균형 "
            "범위(Opening Range)를 형성하는데, 이 상단을 강하게 뚫어내는 순간 매수세 우위가 확정되며 장중 추세로 확장되는 원리를 활용합니다."
        ),
        "setup": [
            "오프닝 봉 수(open_bars): 15분봉 기준 1봉(15분, 공격적) 또는 2봉(30분, 표준 ORB-30)이 가장 많이 쓰입니다.",
            "목표 손익비(target_rr): 기본 1.5R (오프닝 레인지 폭의 1.5배 상승 시 익절). 추세가 강한 종목은 2.0R로 높입니다.",
            "손절 배수(stop_ratio): 기본 1.0R (레인지 폭만큼 하락 시 손절). 타이트한 손절은 0.5~0.75R로 설정합니다.",
            "거래량이 풍부하고 장중 방향성이 뚜렷한 주도주·나스닥/코스피 대표 ETF에 적합합니다.",
        ],
        "execution": [
            "정규장 중 스케줄러가 30초마다 동작하며 새로운 15분봉이 마감될 때마다 자동 실행됩니다.",
            "장 시작 후 초반 open_bars개 15분봉의 최고가(OR High)와 최저가(OR Low)를 확정합니다.",
            "이후 미보유 상태에서 OR High 돌파 매수 조건주문을 유지하며, 체결되면 진입가 기준 +target_rr×레인지 익절 / −stop_ratio×레인지 손절을 감시합니다.",
            "정규장 마감 10분 전이 되면 남아있는 조건주문을 취소하고 보유 포지션을 시장가로 당일 전량 청산합니다(오버나이트 0).",
        ],
        "caution": "장 초반 레인지가 지나치게 넓거나(급등 출발) 횡보 장세에서는 가짜 돌파가 잦을 수 있습니다. 분봉 매매 특성상 호가 스프레드와 수수료 영향이 큽니다.",
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
