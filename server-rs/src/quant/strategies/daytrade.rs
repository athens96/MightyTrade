use serde_json::{json, Value};

use super::{params_to_json, resolve_params, Param, ResolvedParams, Strategy, StrategySpec};
use crate::quant::engine::{Bars, Context, Order};
use crate::quant::indicators as ind;

// ========================================================================= 8. vol_breakout
const VOL_PARAMS: &[Param] = &[
    Param::float("k", "K 값", 0.5, 0.1, 1.5, 0.05),
    Param::int("ma_filter", "이평 필터 (0=끔)", 0.0, 0.0, 200.0),
];

pub struct VolatilityBreakout {
    p: ResolvedParams,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    ma: Option<ind::Series>,
}

impl Strategy for VolatilityBreakout {
    fn id(&self) -> &'static str { "vol_breakout" }
    fn params_json(&self) -> Value { params_to_json(VOL_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["ma_filter"] as usize).max(2) }
    fn prepare(&mut self, bars: &Bars) {
        self.high = bars.high.clone();
        self.low = bars.low.clone();
        self.close = bars.close.clone();
        let mf = self.p["ma_filter"] as usize;
        self.ma = if mf > 0 { Some(ind::sma(&bars.close, mf)) } else { None };
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let mut orders = Vec::new();
        if ctx.position.is_open() {
            orders.push(Order::market("sell", "다음 날 시가 청산"));
        }
        let rng = self.high[i] - self.low[i];
        let trend_ok = match &self.ma {
            None => true,
            Some(m) => m[i].is_some_and(|v| self.close[i] > v),
        };
        if rng > 0.0 && trend_ok {
            orders.push(Order {
                side: "buy".to_string(),
                kind: "stop".to_string(),
                stop: None,
                stop_from_open: Some(self.p["k"] * rng),
                limit: None,
                qty: None,
                reason: format!("변동성 돌파 (K={})", self.p["k"]),
            });
        }
        orders
    }
    fn chart_indicators(&self) -> Vec<Value> {
        if self.p["ma_filter"] as i64 == 0 {
            vec![]
        } else {
            vec![json!({"id": "ma", "params": {"p1": self.p["ma_filter"] as i64, "p2": 0, "p3": 0, "p4": 0}})]
        }
    }
}

pub static VOL_BREAKOUT_SPEC: StrategySpec = StrategySpec {
    id: "vol_breakout",
    name: "변동성 돌파 (래리 윌리엄스)",
    category: "daytrade",
    timeframe: "1d",
    theory: "래리 윌리엄스 · 장중에 힘 있게 치고 오를 때 올라타서 내일 아침에 팔기",
    summary: "오늘 주가가 '시작가 + 어제 움직인 폭×K'를 뚫고 오르면 사고, 다음 날 아침에 팝니다",
    description: "어제 변동폭의 K배(기본 0.5)만큼 시가 위로 돌파 시 매수하고 다음 날 시가에 전량 청산하는 래리 윌리엄스 단타 전략입니다.",
    guide: || json!({
        "background": "전일 변동폭 × K를 당일 시가 위로 돌파할 때 강한 매수세 유입으로 보고 진입해 익일 시가에 청산합니다.",
        "setup": ["K 값 0.5 기본, 하락장 필터로 5~20일 또는 60일 이평 필터 추천"],
        "execution": ["장 시작 1분 뒤 전일 보유분 청산 후 당일 돌파 매수 조건주문 등록"],
        "caution": "매매 빈도가 높아 거래비용 영향을 백테스트로 먼저 확인하세요."
    }),
    params: VOL_PARAMS,
    build: |raw| {
        let p = resolve_params(VOL_PARAMS, raw)?;
        Ok(Box::new(VolatilityBreakout { p, high: vec![], low: vec![], close: vec![], ma: None }))
    },
};

// ========================================================================= 9. gap_reversion
const GAP_PARAMS: &[Param] = &[
    Param::float("gap_pct", "갭하락 기준 (%)", 1.5, 0.3, 10.0, 0.1),
    Param::int("trend_ma", "추세 이평 (0=끔)", 60.0, 0.0, 300.0),
];

pub struct OpeningGapReversion {
    p: ResolvedParams,
    close: Vec<f64>,
    trend: Option<ind::Series>,
}

impl Strategy for OpeningGapReversion {
    fn id(&self) -> &'static str { "gap_reversion" }
    fn params_json(&self) -> Value { params_to_json(GAP_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["trend_ma"] as usize).max(2) }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        let tr = self.p["trend_ma"] as usize;
        self.trend = if tr > 0 { Some(ind::sma(&bars.close, tr)) } else { None };
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let mut orders = Vec::new();
        if ctx.position.is_open() {
            orders.push(Order::market("sell", "다음 날 시가 청산"));
        }
        let trend_ok = match &self.trend {
            None => true,
            Some(ts) => ts[i].is_some_and(|t| self.close[i] > t),
        };
        if trend_ok && self.close[i] > 0.0 {
            let limit_px = self.close[i] * (1.0 - self.p["gap_pct"] / 100.0);
            orders.push(Order::limit(
                "buy",
                limit_px,
                None,
                format!("전일 종가 대비 -{}% 갭 회귀 지정가", self.p["gap_pct"]),
            ));
        }
        orders
    }
    fn chart_indicators(&self) -> Vec<Value> {
        if self.p["trend_ma"] as i64 == 0 {
            vec![]
        } else {
            vec![json!({"id": "ma", "params": {"p1": self.p["trend_ma"] as i64, "p2": 0, "p3": 0, "p4": 0}})]
        }
    }
}

pub static GAP_REVERSION_SPEC: StrategySpec = StrategySpec {
    id: "gap_reversion",
    name: "오프닝 갭 회귀 (단타)",
    category: "daytrade",
    timeframe: "1d",
    theory: "아침에 겁먹고 싸게 던진 물량을 받아서 다음 날 되팔기",
    summary: "어제보다 N% 이상 싸게 떨어질 때만 지정가로 사고, 다음 날 아침에 팝니다",
    description: "60일 상승 흐름 종목이 전일 종가 대비 일정 비율(-1.5%) 아래로 떨어질 때 지정가로 받아 익일 시가에 청산합니다.",
    guide: || json!({
        "background": "아침 장 초반 과잉 매도로 발생한 갭하락이 장중 회복되는 경향을 지정가 매수로 포착합니다.",
        "setup": ["지수 ETF·대형주 1.0~2.0%, 추세 이평 60일 권장"],
        "execution": ["전일 매수분 시가 청산 후 전일 종가 대비 -N% 지정가 매수 주문"],
        "caution": "실적 쇼크 등 구조적 악재 종목보다 대표 지수 ETF에 안전합니다."
    }),
    params: GAP_PARAMS,
    build: |raw| {
        let p = resolve_params(GAP_PARAMS, raw)?;
        Ok(Box::new(OpeningGapReversion { p, close: vec![], trend: None }))
    },
};

// ========================================================================= 10. orb_breakout
const ORB_PARAMS: &[Param] = &[
    Param::int("open_bars", "오프닝 봉 수 (15m)", 2.0, 1.0, 8.0),
    Param::float("target_rr", "목표 손익비 (R)", 1.5, 0.5, 5.0, 0.25),
    Param::float("stop_ratio", "손절 레인지 배수", 1.0, 0.25, 2.0, 0.25),
];

pub struct OpeningRangeBreakout {
    p: ResolvedParams,
    close: Vec<f64>,
    sess: Vec<i64>,
    bar_in_sess: Vec<usize>,
    is_eod: Vec<bool>,
    or_high: ind::Series,
    or_low: ind::Series,
}

impl Strategy for OpeningRangeBreakout {
    fn id(&self) -> &'static str { "orb_breakout" }
    fn params_json(&self) -> Value { params_to_json(ORB_PARAMS, &self.p) }
    fn warmup(&self) -> usize { self.p["open_bars"] as usize + 1 }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        let n = bars.len();
        self.sess = ind::session_buckets(&bars.time);
        self.bar_in_sess = vec![0; n];
        self.is_eod = vec![false; n];
        self.or_high = vec![None; n];
        self.or_low = vec![None; n];
        let k = self.p["open_bars"] as usize;
        let mut cur_s = -1i64;
        let mut idx = 0usize;
        let mut hi = 0.0;
        let mut lo = 0.0;
        for i in 0..n {
            if self.sess[i] != cur_s {
                if i > 0 {
                    self.is_eod[i - 1] = true;
                }
                cur_s = self.sess[i];
                idx = 0;
                hi = bars.high[i];
                lo = bars.low[i];
            } else {
                idx += 1;
                if idx < k {
                    hi = hi.max(bars.high[i]);
                    lo = lo.min(bars.low[i]);
                }
            }
            self.bar_in_sess[i] = idx;
            if idx + 1 >= k {
                self.or_high[i] = Some(hi);
                self.or_low[i] = Some(lo);
            }
        }
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let n = self.close.len();
        let next_is_last = i + 2 < n && self.sess[i + 2] != self.sess[i];
        let next_is_new_sess = i + 1 < n && self.sess[i + 1] != self.sess[i];
        let near_eod = next_is_last || next_is_new_sess || self.is_eod[i];

        if ctx.position.is_open() {
            if near_eod {
                return vec![Order::market("sell", "장 마감 전 당일 청산")];
            }
            let rng = match (self.or_high[i], self.or_low[i]) {
                (Some(oh), Some(ol)) if oh > ol => oh - ol,
                _ => ctx.position.entry_price * 0.01,
            };
            let target = ctx.position.entry_price + rng * self.p["target_rr"];
            let stop = ctx.position.entry_price - rng * self.p["stop_ratio"];
            if self.close[i] >= target {
                return vec![Order::market("sell", format!("ORB 목표 익절 ({}R)", self.p["target_rr"]))];
            }
            return vec![Order::stop("sell", stop, None, format!("ORB 손절 ({}R)", self.p["stop_ratio"]))];
        }

        if near_eod {
            return vec![];
        }
        let (Some(oh), Some(ol)) = (self.or_high[i], self.or_low[i]) else { return vec![]; };
        if oh <= ol {
            return vec![];
        }
        if self.bar_in_sess[i] + 1 >= self.p["open_bars"] as usize && self.close[i] < oh {
            return vec![Order::stop(
                "buy",
                oh,
                None,
                format!("ORB {}봉 고점({oh:.2}) 돌파", self.p["open_bars"] as i64),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "vwap", "params": {}})]
    }
}

pub static ORB_SPEC: StrategySpec = StrategySpec {
    id: "orb_breakout",
    name: "오프닝 레인지 돌파 (ORB)",
    category: "daytrade",
    timeframe: "15m",
    theory: "토비 크래벨 · 장 초반 30분 힘겨루기에서 이긴 방향으로 따라가기 (당일 청산)",
    summary: "장 시작 후 첫 30분 최고가를 뚫으면 매수, 목표가·손절가 또는 마감 10분 전 전량 청산",
    description: "장 초반 첫 30분(15분봉 2개) 고점을 돌파할 때 매수해 목표 손익비 익절·손절 또는 마감 10분 전 당일 전량 청산합니다.",
    guide: || json!({
        "background": "장 시작 직후 30분간의 레인지 고점 돌파 방향으로 진입하고 밤사이 갭 위험 없이 당일 청산합니다.",
        "setup": ["오프닝 2봉(30분), 목표 1.5R, 손절 1.0R 권장"],
        "execution": ["15분봉 마감마다 계산, 마감 10분 전 전량 청산"],
        "caution": "횡보 장세에서는 손절이 발생할 수 있어 모의 테스트로 먼저 검증하세요."
    }),
    params: ORB_PARAMS,
    build: |raw| {
        let p = resolve_params(ORB_PARAMS, raw)?;
        Ok(Box::new(OpeningRangeBreakout {
            p,
            close: vec![],
            sess: vec![],
            bar_in_sess: vec![],
            is_eod: vec![],
            or_high: vec![],
            or_low: vec![],
        }))
    },
};

// ========================================================================= 11. vwap_reversion
const VWAP_PARAMS: &[Param] = &[
    Param::float("band_k", "하단 밴드 배수 (σ)", 1.5, 0.5, 4.0, 0.1),
    Param::int("min_bars", "장 초반 대기 봉 수 (5m)", 4.0, 2.0, 30.0),
    Param::float("stop_pct", "손절 (%)", 1.0, 0.2, 5.0, 0.1),
];

pub struct VwapBandReversion {
    p: ResolvedParams,
    close: Vec<f64>,
    vwap: ind::Series,
    sd: ind::Series,
    sess: Vec<i64>,
    bar_in_sess: Vec<usize>,
}

impl Strategy for VwapBandReversion {
    fn id(&self) -> &'static str { "vwap_reversion" }
    fn params_json(&self) -> Value { params_to_json(VWAP_PARAMS, &self.p) }
    fn warmup(&self) -> usize { self.p["min_bars"] as usize + 1 }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        let v = ind::intraday_vwap(&bars.time, &bars.high, &bars.low, &bars.close, &bars.volume);
        self.vwap = v.vwap;
        self.sd = v.sd;
        self.sess = v.session;
        self.bar_in_sess = Vec::with_capacity(self.sess.len());
        let mut prev_s = -1i64;
        let mut idx = 0usize;
        for &s in &self.sess {
            if s != prev_s {
                idx = 0;
                prev_s = s;
            } else {
                idx += 1;
            }
            self.bar_in_sess.push(idx);
        }
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let n = self.close.len();
        let next_is_last = i + 2 < n && self.sess[i + 2] != self.sess[i];
        let next_is_new_sess = i + 1 < n && self.sess[i + 1] != self.sess[i];
        let near_eod = next_is_last || next_is_new_sess;
        let c = self.close[i];

        if ctx.position.is_open() {
            if near_eod {
                return vec![Order::market("sell", "장 마감 전 당일 청산")];
            }
            if self.vwap[i].is_some_and(|vw| c >= vw) {
                return vec![Order::market("sell", "VWAP 중심선 회복 익절")];
            }
            let stop = ctx.position.entry_price * (1.0 - self.p["stop_pct"] / 100.0);
            return vec![Order::stop("sell", stop, None, format!("VWAP 손절 (-{}%)", self.p["stop_pct"]))];
        }

        if near_eod || self.bar_in_sess[i] < self.p["min_bars"] as usize {
            return vec![];
        }
        let (Some(vw), Some(sd)) = (self.vwap[i], self.sd[i]) else { return vec![]; };
        if sd <= 0.0 { return vec![]; }
        let lower = vw - self.p["band_k"] * sd;
        if c < lower {
            return vec![Order::market(
                "buy",
                format!("VWAP -{}σ 이탈 ({c:.2} < {lower:.2})", self.p["band_k"]),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "vwap", "params": {}})]
    }
}

pub static VWAP_SPEC: StrategySpec = StrategySpec {
    id: "vwap_reversion",
    name: "VWAP 밴드 평균회귀 (단타)",
    category: "daytrade",
    timeframe: "5m",
    theory: "기관 평균 매수가(VWAP)보다 너무 싸졌을 때 샀다가 평균가로 돌아오면 팔기 (당일 청산)",
    summary: "5분봉 가격이 오늘 평균 체결가(VWAP)보다 크게 빠지면 사고, 평균가 회복 시 팝니다",
    description: "5분봉 주가가 당일 거래량 가중 평균가(VWAP) -1.5σ 아래로 과도 이탈 시 매수해 VWAP 회복 시 익절, 마감 10분 전 청산합니다.",
    guide: || json!({
        "background": "기관의 평균 체결 단가(VWAP) 대비 과도한 장중 이탈 후 되돌림을 5분봉으로 포착합니다.",
        "setup": ["하단 밴드 1.5σ, 대기 4봉(20분), 손절 1.0% 권장"],
        "execution": ["5분봉 마감마다 VWAP 계산, 마감 10분 전 당일 전량 청산"],
        "caution": "거래량이 풍부한 초대형주나 대표 지수 ETF에 적합합니다."
    }),
    params: VWAP_PARAMS,
    build: |raw| {
        let p = resolve_params(VWAP_PARAMS, raw)?;
        Ok(Box::new(VwapBandReversion {
            p,
            close: vec![],
            vwap: vec![],
            sd: vec![],
            sess: vec![],
            bar_in_sess: vec![],
        }))
    },
};
