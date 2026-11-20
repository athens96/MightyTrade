from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class VwapBandReversion(Strategy):
    id = "vwap_reversion"
    name = "VWAP 밴드 평균회귀 (단타)"
    category = "daytrade"
    timeframe = "5m"
    theory = "기관 평균 매수가(VWAP)보다 너무 싸졌을 때 샀다가 평균가로 돌아오면 팔기 (당일 청산)"
    summary = "5분봉 가격이 오늘 평균 체결가(VWAP)보다 크게 빠지면 사고, 평균가 회복 시 팝니다"
    description = (
        "VWAP은 '오늘 지금까지 거래된 모든 주식의 평균 가격'입니다. 기관 투자자들이 이 평균가보다 쌀 때 사려는 성질이 있어, "
        "5분봉 주가가 오늘 평균가보다 지나치게 아래로 빠지면 매수하고 평균가로 돌아오면 팝니다. 장 마감 10분 전에 당일 전량 청산합니다."
    )
    guide = {
        "background": (
            "대형 기관·펀드는 주식을 살 때 '오늘 시장의 전체 평균 체결가(VWAP)보다 비싸게 사지 않는 것'을 중요한 기준으로 삼습니다. "
            "그래서 장중에 일시적인 매도 물량으로 주가가 오늘 평균가(VWAP)보다 평소 흔들림의 1.5배 이상 아래로 떨어지면, "
            "기관과 저가 매수세가 들어오며 다시 오늘 평균가 근처로 돌아오는 경우가 많습니다. 이 짧은 되돌림을 5분봉으로 포착합니다."
        ),
        "setup": [
            "하단 밴드 배수: 기본 1.5. 숫자를 2.0으로 올리면 '정말 많이 빠졌을 때만' 조심스럽게 사고, 1.2로 낮추면 더 자주 삽니다.",
            "대기 봉 수(5분봉): 기본 4봉(장 시작 후 20분). 장 초반 어수선한 20분은 건너뛰고 평균가가 안정된 뒤부터 매매합니다.",
            "손절(%): 기본 1.0%. 반등하지 않고 계속 빠질 때를 대비해 손실을 1% 안팎으로 막아줍니다.",
            "거래량이 많아서 사고팔 때 가격 차이가 적은 초대형주(삼성전자, 애플)나 지수 ETF(SPY, KODEX 200)에 적합합니다.",
        ],
        "execution": [
            "정규장 동안 새로운 5분봉이 끝날 때마다 '오늘 지금까지의 평균 체결가(VWAP)'와 흔들림 폭을 다시 계산합니다.",
            "장 시작 20분 뒤부터, 주가가 평균가보다 크게 아래로 내려가면 시장가로 매수합니다.",
            "보유 중에 주가가 오늘 평균가(VWAP)까지 회복하면 바로 팔아 익절하고, 1% 이상 빠지면 손절합니다.",
            "장 마감 10분 전이 되면 남은 주식을 시장가로 전부 팔아 다음 날로 넘기지 않습니다.",
        ],
        "caution": "큰 악재로 하루 종일 반등 없이 계속 내려가는 날에는 손절이 반복될 수 있습니다. 5분봉은 최근 며칠 데이터만 백테스트되므로 반드시 모의 봇으로 먼저 돌려보세요.",
    }
    params = [
        Param("band_k", "하단 밴드 배수 (σ)", 1.5, 0.5, 4.0, step=0.1, integer=False),
        Param("min_bars", "장 초반 대기 봉 수 (5m)", 4, 2, 30),
        Param("stop_pct", "손절 (%)", 1.0, 0.2, 5.0, step=0.1, integer=False),
    ]

    def warmup(self) -> int:
        return self.p["min_bars"] + 1

    def prepare(self, bars: Bars) -> None:
        self.b = bars
        v = ind.intraday_vwap(bars.time, bars.high, bars.low, bars.close, bars.volume)
        self.vwap, self.sd, self.sess = v["vwap"], v["sd"], v["session"]
        self.bar_in_sess = []
        prev_s = -1
        idx = 0
        for s in self.sess:
            idx = 0 if s != prev_s else idx + 1
            prev_s = s
            self.bar_in_sess.append(idx)

    def on_bar(self, i: int, ctx: Context) -> list:
        pos = ctx.position
        next_is_new_sess = i + 1 < len(self.b) and self.sess[i + 1] != self.sess[i]
        vwap, sd = self.vwap[i], self.sd[i]
        c = self.b.close[i]

        if pos.is_open:
            if next_is_new_sess:
                return [Order("sell", reason="장 마감 전 당일 청산")]
            if vwap is not None and c >= vwap:
                return [Order("sell", reason="VWAP 중심선 회복 익절")]
            stop = pos.entry_price * (1.0 - self.p["stop_pct"] / 100.0)
            return [Order("sell", kind="stop", stop=stop, reason=f"VWAP 손절 (-{self.p['stop_pct']:g}%)")]

        if next_is_new_sess or self.bar_in_sess[i] < self.p["min_bars"]:
            return []
        if vwap is None or sd is None or sd <= 0:
            return []
        lower = vwap - self.p["band_k"] * sd
        if c < lower:
            return [Order("buy", reason=f"VWAP -{self.p['band_k']:g}σ 이탈 ({c:,.2f} < {lower:,.2f})")]
        return []

    def chart_indicators(self) -> list:
        return [{"id": "vwap", "params": {}}]
