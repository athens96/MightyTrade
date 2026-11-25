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

// ========================================================================= 12. vwap_pullback
const VWAP_PULLBACK_PARAMS: &[Param] = &[
    Param::int("ema_n", "단기 추세 EMA", 9.0, 3.0, 30.0),
    Param::int("pullback_bars", "눌림 음봉 수", 2.0, 1.0, 5.0),
    Param::float("vol_shrink", "눌림 거래량 감소율", 0.85, 0.3, 1.5, 0.05),
    Param::float("stop_pct", "최대 손절 (%)", 1.5, 0.3, 5.0, 0.1),
];

pub struct VwapEmaPullback {
    p: ResolvedParams,
    open: Vec<f64>,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
    vwap: ind::Series,
    ema_fast: ind::Series,
    vol_sma: ind::Series,
    sess: Vec<i64>,
    bar_in_sess: Vec<usize>,
    sess_open: Vec<f64>,
}

impl Strategy for VwapEmaPullback {
    fn id(&self) -> &'static str { "vwap_pullback" }
    fn params_json(&self) -> Value { params_to_json(VWAP_PULLBACK_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["ema_n"] as usize).max(10) + 2 }
    fn prepare(&mut self, bars: &Bars) {
        self.open = bars.open.clone();
        self.high = bars.high.clone();
        self.low = bars.low.clone();
        self.close = bars.close.clone();
        self.volume = bars.volume.clone();
        let v = ind::intraday_vwap(&bars.time, &bars.high, &bars.low, &bars.close, &bars.volume);
        self.vwap = v.vwap;
        self.sess = v.session;
        self.ema_fast = ind::ema(&bars.close, self.p["ema_n"] as usize);
        self.vol_sma = ind::sma(&bars.volume, 10);
        let n = bars.len();
        self.bar_in_sess = vec![0; n];
        self.sess_open = vec![0.0; n];
        let mut prev_s = -1i64;
        let mut idx = 0usize;
        let mut s_op = 0.0;
        for i in 0..n {
            if self.sess[i] != prev_s {
                prev_s = self.sess[i];
                idx = 0;
                s_op = bars.open[i];
            } else {
                idx += 1;
            }
            self.bar_in_sess[i] = idx;
            self.sess_open[i] = s_op;
        }
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let n = self.close.len();
        let next_is_last = i + 2 < n && self.sess[i + 2] != self.sess[i];
        let next_is_new_sess = i + 1 < n && self.sess[i + 1] != self.sess[i];
        let near_eod = next_is_last || next_is_new_sess;
        let c = self.close[i];
        let ema_n = self.p["ema_n"] as i64;

        if ctx.position.is_open() {
            if near_eod {
                return vec![Order::market("sell", "장 마감 전 당일 청산")];
            }
            let held_bars = i as isize - ctx.position.entry_index;
            if held_bars >= 1 {
                if self.ema_fast[i].is_some_and(|e| c < e) {
                    return vec![Order::market("sell", format!("{ema_n} EMA 추세 이탈 트레일링 청산"))];
                }
                if self.vwap[i].is_some_and(|vw| c < vw) {
                    return vec![Order::market("sell", "VWAP 중심선 이탈 정리")];
                }
            }
            let stop = ctx.position.entry_price * (1.0 - self.p["stop_pct"] / 100.0);
            return vec![Order::stop(
                "sell",
                stop,
                None,
                format!("눌림목 손절 (-{}%)", self.p["stop_pct"]),
            )];
        }

        if near_eod || self.bar_in_sess[i] < 3 {
            return vec![];
        }
        let (Some(vw), Some(ef)) = (self.vwap[i], self.ema_fast[i]) else {
            return vec![];
        };
        // 1파 상승 추세 확인: 당일 시가·VWAP 위, 단기 EMA가 VWAP 이상
        if c < vw * 0.997 || ef < vw * 0.998 || c <= self.sess_open[i] {
            return vec![];
        }
        let pb = self.p["pullback_bars"] as usize;
        if i < pb + 1 {
            return vec![];
        }
        // 직전 1~pb 봉이 숨 고르기(음봉/단봉)이며 같은 세션인지 확인
        let mut pull_ok = false;
        let mut red_cnt = 0usize;
        for k in 0..pb {
            let idx = i - k;
            if self.sess[idx] != self.sess[i] {
                break;
            }
            if self.close[idx] <= self.open[idx] || self.high[idx] <= self.high[idx.saturating_sub(1)] {
                red_cnt += 1;
            }
        }
        if red_cnt >= 1 && self.close[i] <= self.open[i] {
            // 눌림 저점이 9 EMA 또는 VWAP 부근에 근접해 지지받는지 확인
            let near_support = self.low[i] <= ef * 1.012 || self.low[i] <= vw * 1.012;
            let vol_ok = match self.vol_sma[i] {
                Some(vs) if vs > 0.0 => self.volume[i] <= vs * self.p["vol_shrink"],
                _ => true,
            };
            // 눌림 전에 실제 상승 파동(양봉)이 있었는지 확인
            let prior_idx = i - pb;
            let had_impulse = self.sess[prior_idx] == self.sess[i]
                && self.close[prior_idx] > self.open[prior_idx];
            pull_ok = near_support && vol_ok && had_impulse;
        }
        if pull_ok && self.high[i] > 0.0 {
            return vec![Order::stop(
                "buy",
                self.high[i],
                None,
                format!("VWAP+{ema_n}EMA 첫 눌림목 고점({:.2}) 돌파", self.high[i]),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![
            json!({"id": "vwap", "params": {}}),
            json!({"id": "ma", "params": {"p1": self.p["ema_n"] as i64, "p2": 0, "p3": 0, "p4": 0}}),
        ]
    }
}

pub static VWAP_PULLBACK_SPEC: StrategySpec = StrategySpec {
    id: "vwap_pullback",
    name: "[실전] VWAP + 9 EMA 첫 눌림목 (불플래그)",
    category: "daytrade",
    timeframe: "5m",
    theory: "홍인기 · Ross Cameron · Andrew Aziz · 1파 상승 뒤 거래량 줄어든 첫 눌림 고점 돌파 (당일 청산)",
    summary: "VWAP 위 주도주가 9 EMA 부근에서 거래량이 줄며 숨을 고른 뒤 직전 음봉 고점을 뚫을 때 매수하고, 9 EMA 이탈 시 익절합니다",
    description: "실전 단타 고수들의 핵심 타점인 '첫 눌림목(불플래그)'을 5분봉으로 구현했습니다. 상승 1파 후 거래량이 줄어든 눌림봉 고점에 돌파 매수를 걸어 떨어지는 칼날을 피하고, 9 EMA 트레일링으로 수익을 끌고 갑니다.",
    guide: || json!({
        "background": "추격 매수 대신 VWAP 위 상승 1파 후 단기 차익 매물이 소화되는 첫 눌림목에서 '반등이 확인되는 고점 돌파' 시점에만 진입합니다.",
        "setup": ["단기 EMA 9, 눌림 음봉 2봉, 거래량 감소율 0.85, 손절 1.5% 권장", "거래대금이 풍부한 주도주·대형주·대표 ETF에 효과적입니다"],
        "execution": ["5분봉 마감마다 눌림 고점 돌파 조건 감시, 9 EMA 또는 VWAP 이탈 시 청산, 장 마감 10분 전 전량 정리"],
        "caution": "재료나 거래량이 없는 횡보 종목보다 당일 주목받는 거래대금 상위 종목에서 승률이 높습니다."
    }),
    params: VWAP_PULLBACK_PARAMS,
    build: |raw| {
        let p = resolve_params(VWAP_PULLBACK_PARAMS, raw)?;
        Ok(Box::new(VwapEmaPullback {
            p,
            open: vec![],
            high: vec![],
            low: vec![],
            close: vec![],
            volume: vec![],
            vwap: vec![],
            ema_fast: vec![],
            vol_sma: vec![],
            sess: vec![],
            bar_in_sess: vec![],
            sess_open: vec![],
        }))
    },
};

// ========================================================================= 13. hod_breakout
const HOD_PARAMS: &[Param] = &[
    Param::int("wait_bars", "초반 관망 봉 수 (5m)", 3.0, 1.0, 12.0),
    Param::float("vol_mult", "돌파 거래량 배수", 1.5, 1.0, 5.0, 0.1),
    Param::float("tight_pct", "고가놀이 허용폭 (%)", 1.5, 0.3, 5.0, 0.1),
    Param::int("trail_ema", "익절 트레일링 EMA", 9.0, 3.0, 30.0),
    Param::float("stop_pct", "가짜돌파 칼손절 (%)", 1.2, 0.3, 5.0, 0.1),
];

pub struct HodBreakout {
    p: ResolvedParams,
    high: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
    vwap: ind::Series,
    ema_trail: ind::Series,
    vol_sma: ind::Series,
    sess: Vec<i64>,
    bar_in_sess: Vec<usize>,
    hod_before: Vec<f64>,
}

impl Strategy for HodBreakout {
    fn id(&self) -> &'static str { "hod_breakout" }
    fn params_json(&self) -> Value { params_to_json(HOD_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["trail_ema"] as usize).max(12) + 2 }
    fn prepare(&mut self, bars: &Bars) {
        self.high = bars.high.clone();
        self.close = bars.close.clone();
        self.volume = bars.volume.clone();
        let v = ind::intraday_vwap(&bars.time, &bars.high, &bars.low, &bars.close, &bars.volume);
        self.vwap = v.vwap;
        self.sess = v.session;
        self.ema_trail = ind::ema(&bars.close, self.p["trail_ema"] as usize);
        self.vol_sma = ind::sma(&bars.volume, 12);
        let n = bars.len();
        self.bar_in_sess = vec![0; n];
        self.hod_before = vec![0.0; n];
        let mut prev_s = -1i64;
        let mut idx = 0usize;
        let mut hi = 0.0f64;
        for i in 0..n {
            if self.sess[i] != prev_s {
                prev_s = self.sess[i];
                idx = 0;
                hi = bars.high[i];
                self.hod_before[i] = 0.0;
            } else {
                idx += 1;
                self.hod_before[i] = hi;
                hi = hi.max(bars.high[i]);
            }
            self.bar_in_sess[i] = idx;
        }
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let n = self.close.len();
        let next_is_last = i + 2 < n && self.sess[i + 2] != self.sess[i];
        let next_is_new_sess = i + 1 < n && self.sess[i + 1] != self.sess[i];
        let near_eod = next_is_last || next_is_new_sess;
        let c = self.close[i];
        let te = self.p["trail_ema"] as i64;

        if ctx.position.is_open() {
            if near_eod {
                return vec![Order::market("sell", "장 마감 전 당일 청산")];
            }
            let held_bars = i as isize - ctx.position.entry_index;
            if held_bars >= 1 && self.ema_trail[i].is_some_and(|e| c < e) {
                return vec![Order::market("sell", format!("{te} EMA 추세 이탈 트레일링 청산"))];
            }
            let stop = ctx.position.entry_price * (1.0 - self.p["stop_pct"] / 100.0);
            return vec![Order::stop(
                "sell",
                stop,
                None,
                format!("가짜돌파 칼손절 (-{}%)", self.p["stop_pct"]),
            )];
        }

        if near_eod || self.bar_in_sess[i] < self.p["wait_bars"] as usize {
            return vec![];
        }
        let hod = self.hod_before[i];
        if hod <= 0.0 {
            return vec![];
        }
        let Some(vw) = self.vwap[i] else {
            return vec![];
        };
        if c < vw {
            return vec![];
        }
        let vol_ratio = match self.vol_sma[i] {
            Some(vs) if vs > 0.0 => self.volume[i] / vs,
            _ => self.p["vol_mult"],
        };
        // 케이스 1: 이미 거래량을 동반해 당일고가를 강하게 돌파 마감
        if c >= hod && vol_ratio >= self.p["vol_mult"] {
            return vec![Order::market(
                "buy",
                format!("당일고가({hod:.2}) 거래량 {vol_ratio:.1}배 돌파"),
            )];
        }
        // 케이스 2: 고가놀이 — 당일고가 바로 아래(tight_pct 이내)에서 밀리지 않고 응축
        let dist_pct = (hod - c) / hod * 100.0;
        if dist_pct > 0.0
            && dist_pct <= self.p["tight_pct"]
            && vol_ratio >= self.p["vol_mult"] * 0.65
        {
            return vec![Order::stop(
                "buy",
                hod,
                None,
                format!("고가놀이 당일고가({hod:.2}) 재돌파 대기"),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![
            json!({"id": "vwap", "params": {}}),
            json!({"id": "ma", "params": {"p1": self.p["trail_ema"] as i64, "p2": 0, "p3": 0, "p4": 0}}),
        ]
    }
}

pub static HOD_BREAKOUT_SPEC: StrategySpec = StrategySpec {
    id: "hod_breakout",
    name: "[실전] 당일고가 · 고가놀이 거래량 돌파",
    category: "daytrade",
    timeframe: "5m",
    theory: "불개미 · 김형준(보컬) · CIS · Qullamaggie · 고점 부근 버티기 후 거래량 돌파 (당일 청산)",
    summary: "VWAP 위에서 당일 최고가 바로 아래 좁게 버티다 거래량이 터지며 당일고가를 뚫을 때 매수하고, 밀리면 즉시 칼손절합니다",
    description: "기존 ORB에 실전 고수들의 핵심인 '고가놀이(고점 밀집) + 거래량 배수 + VWAP 위 필터 + 9 EMA 트레일링'을 결합한 돌파 단타 전략입니다.",
    guide: || json!({
        "background": "약한 종목의 저점을 예측해 사지 않고, 이미 강한 주도주가 당일 고점 근처에서 안 빠지고 버티다 힘(거래량)이 붙어 고점을 뚫는 순간만 공략합니다.",
        "setup": ["초반 관망 3봉(15분), 돌파 거래량 1.5배, 고가놀이 폭 1.5%, 칼손절 1.2% 권장"],
        "execution": ["5분봉 마감마다 당일고가·고가놀이 감시, 가짜돌파 시 칼손절, 추세 지속 시 9 EMA 이탈까지 보유 후 마감 10분 전 정리"],
        "caution": "돌파 매매는 손절을 미루면 손실이 커지므로 칼손절(1~1.5%)과 서킷브레이커를 함께 쓰는 것을 권장합니다."
    }),
    params: HOD_PARAMS,
    build: |raw| {
        let p = resolve_params(HOD_PARAMS, raw)?;
        Ok(Box::new(HodBreakout {
            p,
            high: vec![],
            close: vec![],
            volume: vec![],
            vwap: vec![],
            ema_trail: vec![],
            vol_sma: vec![],
            sess: vec![],
            bar_in_sess: vec![],
            hod_before: vec![],
        }))
    },
};

// ========================================================================= 14. red_to_green
const R2G_PARAMS: &[Param] = &[
    Param::float("min_dip_pct", "초반 음봉 눌림 최소 (%)", 0.5, 0.1, 5.0, 0.1),
    Param::float("vol_mult", "양전 돌파 거래량 배수", 1.3, 0.8, 5.0, 0.1),
    Param::float("target_pct", "목표 익절 (%)", 2.5, 0.5, 10.0, 0.25),
    Param::float("stop_pct", "손절 기준 (%)", 1.2, 0.3, 5.0, 0.1),
];

pub struct RedToGreen {
    p: ResolvedParams,
    open: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
    vwap: ind::Series,
    vol_sma: ind::Series,
    sess: Vec<i64>,
    bar_in_sess: Vec<usize>,
    prev_close: ind::Series,
    had_red: Vec<bool>,
}

impl Strategy for RedToGreen {
    fn id(&self) -> &'static str { "red_to_green" }
    fn params_json(&self) -> Value { params_to_json(R2G_PARAMS, &self.p) }
    fn warmup(&self) -> usize { 15 }
    fn prepare(&mut self, bars: &Bars) {
        self.open = bars.open.clone();
        self.close = bars.close.clone();
        self.volume = bars.volume.clone();
        let v = ind::intraday_vwap(&bars.time, &bars.high, &bars.low, &bars.close, &bars.volume);
        self.vwap = v.vwap;
        self.sess = v.session;
        self.vol_sma = ind::sma(&bars.volume, 10);
        let n = bars.len();
        self.bar_in_sess = vec![0; n];
        self.prev_close = vec![None; n];
        self.had_red = vec![false; n];
        let min_dip = self.p["min_dip_pct"] / 100.0;
        let mut prev_s = -1i64;
        let mut idx = 0usize;
        let mut last_sess_close: Option<f64> = None;
        let mut seen_red = false;
        for i in 0..n {
            if self.sess[i] != prev_s {
                if i > 0 {
                    last_sess_close = Some(bars.close[i - 1]);
                }
                prev_s = self.sess[i];
                idx = 0;
                seen_red = false;
            } else {
                idx += 1;
            }
            self.bar_in_sess[i] = idx;
            self.prev_close[i] = last_sess_close;
            if let Some(pc) = last_sess_close {
                if bars.low[i] <= pc * (1.0 - min_dip) {
                    seen_red = true;
                }
            }
            self.had_red[i] = seen_red;
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
            let target = ctx.position.entry_price * (1.0 + self.p["target_pct"] / 100.0);
            if c >= target {
                return vec![Order::market(
                    "sell",
                    format!("음봉→양전 목표 익절 (+{}%)", self.p["target_pct"]),
                )];
            }
            let held_bars = i as isize - ctx.position.entry_index;
            if held_bars >= 1
                && self.vwap[i].is_some_and(|vw| c < vw)
                && self.prev_close[i].is_some_and(|pc| c < pc)
            {
                return vec![Order::market("sell", "전일종가·VWAP 재이탈 손절")];
            }
            let stop = ctx.position.entry_price * (1.0 - self.p["stop_pct"] / 100.0);
            return vec![Order::stop(
                "sell",
                stop,
                None,
                format!("양전 손절 (-{}%)", self.p["stop_pct"]),
            )];
        }

        if near_eod || self.bar_in_sess[i] < 2 || !self.had_red[i] || i == 0 {
            return vec![];
        }
        let (Some(pc), Some(vw)) = (self.prev_close[i], self.vwap[i]) else {
            return vec![];
        };
        let prev_below = self.sess[i - 1] == self.sess[i]
            && (self.close[i - 1] <= pc || self.close[i - 1] <= vw);
        let crossed_green = c > pc && c > vw && c > self.open[i];
        let vol_ok = match self.vol_sma[i] {
            Some(vs) if vs > 0.0 => self.volume[i] >= vs * self.p["vol_mult"],
            _ => true,
        };
        if prev_below && crossed_green && vol_ok {
            return vec![Order::market(
                "buy",
                format!("음봉→양전 돌파 (전일종가 {pc:.2} + VWAP 상향)"),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "vwap", "params": {}})]
    }
}

pub static RED_TO_GREEN_SPEC: StrategySpec = StrategySpec {
    id: "red_to_green",
    name: "[실전] 음봉→양전(Red-to-Green) VWAP 돌파",
    category: "daytrade",
    timeframe: "5m",
    theory: "SMB Capital · Lance Breitstein · 장 초반 약세 후 전일종가+VWAP 동시 탈환 (당일 청산)",
    summary: "장 초반 어제 종가 아래(파란불)로 눌렸다가 강한 거래량으로 전일 종가와 VWAP을 동시에 뚫고 양전할 때 매수합니다",
    description: "뉴욕 프롭 트레이딩(SMB Capital)의 대표 실전 셋업입니다. 아침 약세에 물린 공매도·매도세가 되사기 시작하는 '전일종가(빨간불 전환) + VWAP 탈환' 시점을 포착합니다.",
    guide: || json!({
        "background": "전일 종가 아래로 출발해 겁먹은 매도 물량이 쏟아진 뒤, 거래량과 함께 전일 종가와 VWAP을 위로 되찾으면 강한 반전 추세가 나오는 경우가 많습니다.",
        "setup": ["초반 눌림 0.5% 이상, 양전 거래량 1.3배, 익절 2.5%, 손절 1.2% 권장"],
        "execution": ["5분봉 마감마다 전일종가·VWAP 상향 돌파 감시, 재이탈 시 즉시 손절, 장 마감 10분 전 당일 청산"],
        "caution": "실적·뉴스 이슈가 있는 대형주(엔비디아·테슬라·삼성전자·하이닉스 등)에서 가장 깔끔하게 작동합니다."
    }),
    params: R2G_PARAMS,
    build: |raw| {
        let p = resolve_params(R2G_PARAMS, raw)?;
        Ok(Box::new(RedToGreen {
            p,
            open: vec![],
            close: vec![],
            volume: vec![],
            vwap: vec![],
            vol_sma: vec![],
            sess: vec![],
            bar_in_sess: vec![],
            prev_close: vec![],
            had_red: vec![],
        }))
    },
};

// ========================================================================= 15. closing_bet
const CLOSING_BET_PARAMS: &[Param] = &[
    Param::float("min_gain_pct", "당일 최소 상승률 (%)", 3.0, 0.5, 20.0, 0.5),
    Param::float("max_gain_pct", "당일 최대 상승률 (%)", 24.0, 5.0, 29.5, 0.5),
    Param::float("close_strength", "고가 근접도 (0~1)", 0.75, 0.5, 0.98, 0.05),
    Param::float("vol_mult", "거래량 급증 배수", 1.5, 0.5, 5.0, 0.1),
    Param::int("ma_filter", "추세 이평 (0=끔)", 20.0, 0.0, 200.0),
];

pub struct ClosingBet {
    p: ResolvedParams,
    high: Vec<f64>,
    low: Vec<f64>,
    close: Vec<f64>,
    volume: Vec<f64>,
    vol_sma: ind::Series,
    ma: Option<ind::Series>,
}

impl Strategy for ClosingBet {
    fn id(&self) -> &'static str { "closing_bet" }
    fn params_json(&self) -> Value { params_to_json(CLOSING_BET_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["ma_filter"] as usize).max(20) + 1 }
    fn prepare(&mut self, bars: &Bars) {
        self.high = bars.high.clone();
        self.low = bars.low.clone();
        self.close = bars.close.clone();
        self.volume = bars.volume.clone();
        self.vol_sma = ind::sma(&bars.volume, 20);
        let mf = self.p["ma_filter"] as usize;
        self.ma = if mf > 0 { Some(ind::sma(&bars.close, mf)) } else { None };
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let mut orders = Vec::new();
        if ctx.position.is_open() {
            orders.push(Order::market("sell", "익일 시초가 종가베팅 청산"));
            return orders;
        }
        if i == 0 || self.close[i - 1] <= 0.0 {
            return orders;
        }
        let gain_pct = (self.close[i] / self.close[i - 1] - 1.0) * 100.0;
        let rng = self.high[i] - self.low[i];
        if rng <= 0.0 {
            return orders;
        }
        let strength = (self.close[i] - self.low[i]) / rng;
        let vol_ok = match self.vol_sma[i] {
            Some(vs) if vs > 0.0 => self.volume[i] >= vs * self.p["vol_mult"],
            _ => true,
        };
        let trend_ok = match &self.ma {
            None => true,
            Some(m) => m[i].is_some_and(|v| self.close[i] > v),
        };
        if gain_pct >= self.p["min_gain_pct"]
            && gain_pct <= self.p["max_gain_pct"]
            && strength >= self.p["close_strength"]
            && vol_ok
            && trend_ok
        {
            orders.push(Order::market(
                "buy",
                format!(
                    "종가베팅 진입 (+{gain_pct:.1}%, 고가근접 {:.0}%)",
                    strength * 100.0
                ),
            ));
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

pub static CLOSING_BET_SPEC: StrategySpec = StrategySpec {
    id: "closing_bet",
    name: "[실전] 주도주 종가베팅 (오버나잇 단타)",
    category: "daytrade",
    timeframe: "1d",
    theory: "김형준(보컬) · 홍인기 · 장 마감까지 강한 고가 마감 주도주 진입 → 다음 날 시초가 청산",
    summary: "거래량이 터지며 강하게 올라 윗꼬리 없이 고가 부근으로 마감하는 날 장 마감 전 매수해, 다음 날 아침 시초가에 청산합니다",
    description: "국내 실전 고수들과 직장인 트레이더들이 가장 선호하는 '종가베팅' 전략입니다. 자동매매 봇 실행 시 장 마감 20분 전~3분 전에 당일 강세·고가 근접·거래량 조건을 검사해 매수하고, 다음 날 장 시작 1분 뒤 시초가에 자동 청산합니다.",
    guide: || json!({
        "background": "장 막판까지 무너지지 않고 고가 부근에서 강하게 끝난 주도주는 다음 날 아침 추격 매수세로 시가 갭상승할 확률이 높습니다.",
        "setup": ["최소 상승률 3%, 최대 24%(상한가 잠김 제외), 고가 근접도 0.75(상위 25% 마감), 거래량 1.5배 권장"],
        "execution": ["봇 스케줄러가 매일 장 마감 20분 전(한국 15:10 / 미국 15:40 ET) 조건 충족 시 매수하고, 다음 거래일 개장 1분 뒤 전량 청산합니다"],
        "caution": "오버나잇 특성상 밤사이 악재 갭하락 위험이 있으므로 공통 손절·운용 자금 한도를 꼭 설정하세요."
    }),
    params: CLOSING_BET_PARAMS,
    build: |raw| {
        let p = resolve_params(CLOSING_BET_PARAMS, raw)?;
        Ok(Box::new(ClosingBet {
            p,
            high: vec![],
            low: vec![],
            close: vec![],
            volume: vec![],
            vol_sma: vec![],
            ma: None,
        }))
    },
};

#[cfg(test)]
mod pro_daytrade_tests {
    use super::*;
    use crate::quant::engine::{run_backtest, Costs, RiskRules};

    fn synthetic_intraday_bars() -> Bars {
        // Two 5m trading sessions separated by overnight gap
        let mut time = Vec::new();
        let mut open = Vec::new();
        let mut high = Vec::new();
        let mut low = Vec::new();
        let mut close = Vec::new();
        let mut volume = Vec::new();

        // Session 0 (baseline): 20 bars ending at 100.0
        let t0 = 1_700_000_000i64;
        for k in 0..20 {
            time.push(t0 + k * 300);
            open.push(99.5);
            high.push(100.5);
            low.push(99.0);
            close.push(100.0);
            volume.push(10_000.0);
        }
        // Session 1 (next day, +18h): dip red -> green reclaim -> impulse -> bull flag pullback -> HOD breakout -> rally
        let t1 = t0 + 86400;
        let pattern: &[(f64, f64, f64, f64, f64)] = &[
            (99.4, 99.6, 98.8, 99.1, 15_000.0),   // 0: red open below prev_close(100)
            (99.1, 99.5, 98.9, 99.4, 14_000.0),   // 1: base
            (99.5, 101.2, 99.4, 101.0, 28_000.0), // 2: Red-to-Green breakout above 100 + VWAP
            (101.0, 102.5, 100.9, 102.3, 30_000.0), // 3: impulse green bar
            (102.3, 102.4, 101.4, 101.7, 9_000.0),  // 4: quiet pullback bar 1 near EMA
            (101.7, 101.9, 101.1, 101.4, 7_000.0),  // 5: quiet pullback bar 2 near EMA/VWAP
            (101.5, 103.2, 101.4, 102.9, 32_000.0), // 6: breaks pullback high (101.9) AND HOD (102.5)!
            (102.9, 103.8, 102.7, 103.6, 25_000.0), // 7: follow through
            (103.6, 104.2, 103.2, 103.9, 22_000.0), // 8: rally
            (103.9, 104.0, 101.8, 102.0, 20_000.0), // 9: EMA break
            (102.0, 102.2, 101.5, 101.8, 12_000.0), // 10: end
            (101.8, 102.0, 101.6, 101.9, 10_000.0), // 11: eod
        ];
        for (k, &(o, h, l, c, v)) in pattern.iter().enumerate() {
            time.push(t1 + (k as i64) * 300);
            open.push(o);
            high.push(h);
            low.push(l);
            close.push(c);
            volume.push(v);
        }
        Bars { time, open, high, low, close, volume }
    }

    #[test]
    fn pro_intraday_strategies_trigger_clean_entries_and_exits() {
        let bars = synthetic_intraday_bars();
        for sid in ["vwap_pullback", "hod_breakout", "red_to_green"] {
            let spec = crate::quant::strategies::get_spec(sid).unwrap();
            let mut strat = (spec.build)(&json!({})).unwrap();
            let res = run_backtest(
                &bars,
                strat.as_mut(),
                10_000_000.0,
                Costs::default(),
                15,
                true,
                RiskRules::default(),
                None,
                "off",
            )
            .unwrap();
            let trades = res["trades"].as_array().unwrap();
            assert!(!trades.is_empty(), "{sid} should produce at least one trade on synthetic setup");
        }
    }

    #[test]
    fn closing_bet_buys_strong_leader_and_exits_next_open() {
        let mut time = Vec::new();
        let mut open = Vec::new();
        let mut high = Vec::new();
        let mut low = Vec::new();
        let mut close = Vec::new();
        let mut volume = Vec::new();
        for d in 0..30 {
            time.push(1_700_000_000 + d * 86400);
            let px = 100.0 + (d as f64) * 0.3;
            if d == 24 {
                // Strong leader day: +7% gain, near high close, 2.5x volume
                open.push(px);
                low.push(px * 0.995);
                high.push(px * 1.075);
                close.push(px * 1.07);
                volume.push(300_000.0);
            } else if d == 25 {
                // Next morning gap up!
                open.push(px * 1.09);
                high.push(px * 1.10);
                low.push(px * 1.06);
                close.push(px * 1.07);
                volume.push(150_000.0);
            } else {
                open.push(px);
                high.push(px * 1.01);
                low.push(px * 0.99);
                close.push(px);
                volume.push(100_000.0);
            }
        }
        let bars = Bars { time, open, high, low, close, volume };
        let mut strat = (CLOSING_BET_SPEC.build)(&json!({})).unwrap();
        let res = run_backtest(
            &bars,
            strat.as_mut(),
            10_000_000.0,
            Costs::default(),
            21,
            true,
            RiskRules::default(),
            None,
            "off",
        )
        .unwrap();
        let trades = res["trades"].as_array().unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0]["open"], json!(false));
    }
}
