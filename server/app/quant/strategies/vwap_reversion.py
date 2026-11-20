from __future__ import annotations

from .. import indicators as ind
from .base import Bars, Context, Order, Param, Strategy


class VwapBandReversion(Strategy):
    id = "vwap_reversion"
    name = "VWAP 밴드 평균회귀 (단타)"
    category = "daytrade"
    timeframe = "5m"
    theory = "기관 체결 벤치마크 · 장중 VWAP 표준편차 이탈 회귀 (5분봉 · 당일 청산)"
    summary = "5분봉 가격이 당일 VWAP − K×σ 하단 이탈 시 매수, VWAP 회복 또는 손절·마감 청산"
    description = (
        "기관 알고리즘의 체결 기준가인 당일 VWAP(거래량 가중 평균가) 주변으로 가격이 회귀하는 "
        "장중 평균회귀 이론을 적용한 5분봉 단타 전략. 개장 초반 표본 축적 후 종가가 VWAP 하단 밴드(−Kσ) 아래로 "
        "과매도 이탈하면 매수하고, VWAP 회복 시 익절하거나 손절(%)·장 마감 전 당일 청산합니다."
    )
    guide = {
        "background": (
            "기관 투자자와 브로커의 주문 집행 성과 평가 기준(Execution Benchmark)인 VWAP(Volume Weighted Average Price)에 기반한 "
            "장중 통계적 차익/평균회귀 이론입니다. 대형 기관 알고리즘이 당일 평균 체결단가(VWAP)보다 저렴한 구간에서 매수 집행을 "
            "집중하는 특성 때문에, 거래량 가중 표준편차 하단(VWAP − Kσ) 아래로 과도하게 이탈한 가격은 VWAP 중심으로 되돌아오는 경향이 있습니다."
        ),
        "setup": [
            "하단 밴드 배수(band_k): 기본 1.5σ. 횡보장 빈도를 높이려면 1.2σ, 극단적 과매도만 노리면 2.0σ로 설정합니다.",
            "대기 봉 수(min_bars): 기본 4봉(5분봉×4 = 개장 후 20분). 초반 변동성 노이즈를 거른 뒤부터 진입합니다.",
            "손절(stop_pct): 기본 1.0%. 장중 급락 추세(추세 하락일)에 물리지 않도록 반드시 종목 변동성에 맞게 설정하세요.",
            "거래량이 두꺼워 호가 스프레드가 좁은 초대형주·지수 ETF(삼성전자, SPY, QQQ 등)에 적합합니다.",
        ],
        "execution": [
            "정규장 중 스케줄러가 새로운 5분봉이 마감될 때마다 당일 누적 VWAP과 거래량 가중 표준편차(σ)를 갱신합니다.",
            "개장 후 min_bars 봉 경과 후, 미보유 상태에서 5분봉 종가가 'VWAP − band_k×σ' 아래로 내려가면 시장가 매수합니다.",
            "보유 중 5분봉 종가가 당일 VWAP 이상으로 회복하면 즉시 익절 매도하고, 진입가 대비 −stop_pct% 이하 시 손절합니다.",
            "정규장 마감 10분 전에는 모든 미체결을 취소하고 보유 잔량을 시장가로 당일 강제 청산합니다.",
        ],
        "caution": "강한 악재로 하루 종일 VWAP 아래에서 계속 하락하는 '원웨이 하락장'에서는 손절이 반복될 수 있습니다. 5분봉 백테스트는 최근 수일치 분봉 기준이므로 모의 봇으로 며칠간 검증 후 실거래 전환하세요.",
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
