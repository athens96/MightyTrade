use serde_json::{json, Value};

use super::{params_to_json, resolve_params, Param, ResolvedParams, Strategy, StrategySpec};
use crate::quant::engine::{Bars, Context, Order};
use crate::quant::indicators as ind;

// ========================================================================= 1. ma_cross
const MA_PARAMS: &[Param] = &[
    Param::int("fast", "단기 이평", 20.0, 2.0, 200.0),
    Param::int("slow", "장기 이평", 60.0, 3.0, 400.0),
];

pub struct MovingAverageCross {
    p: ResolvedParams,
    fast: ind::Series,
    slow: ind::Series,
}

impl Strategy for MovingAverageCross {
    fn id(&self) -> &'static str { "ma_cross" }
    fn params_json(&self) -> Value { params_to_json(MA_PARAMS, &self.p) }
    fn warmup(&self) -> usize { self.p["slow"] as usize + 1 }
    fn prepare(&mut self, bars: &Bars) {
        self.fast = ind::sma(&bars.close, self.p["fast"] as usize);
        self.slow = ind::sma(&bars.close, self.p["slow"] as usize);
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        if i < 1 { return vec![]; }
        let (Some(f0), Some(s0), Some(f1), Some(s1)) =
            (self.fast[i - 1], self.slow[i - 1], self.fast[i], self.slow[i])
        else { return vec![]; };
        let fn_ = self.p["fast"] as i64;
        let sn_ = self.p["slow"] as i64;
        if !ctx.position.is_open() && f0 <= s0 && f1 > s1 {
            return vec![Order::market("buy", format!("골든크로스 ({fn_}/{sn_})"))];
        }
        if ctx.position.is_open() && f0 >= s0 && f1 < s1 {
            return vec![Order::market("sell", format!("데드크로스 ({fn_}/{sn_})"))];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "ma", "params": {"p1": self.p["fast"] as i64, "p2": self.p["slow"] as i64, "p3": 0, "p4": 0}})]
    }
}

pub static MA_CROSS_SPEC: StrategySpec = StrategySpec {
    id: "ma_cross",
    name: "이동평균 크로스",
    category: "swing",
    timeframe: "1d",
    theory: "다우 이론 · 달리는 기차에 올라타는 추세추종",
    summary: "단기 평균 가격이 장기 평균 가격을 뚫고 올라가면 사고, 내려가면 팝니다",
    description: "가장 오래되고 이해하기 쉬운 추세 전략입니다. 최근 가격 흐름(단기 이동평균)이 오랜 기간의 평균 가격(장기 이동평균)보다 위로 올라서면 '이제 상승 흐름이 시작됐다'고 보고 매수하고, 반대로 아래로 내려가면 '상승이 끝났다'고 보고 매도합니다.",
    guide: || json!({
        "background": "100년 넘게 검증된 '다우 이론'의 핵심은 아주 단순합니다. 한 번 방향을 잡은 주가는 관성처럼 한동안 그 방향으로 계속 움직인다는 것입니다. 최근 20일 평균 가격은 '요즘 사람들의 투자 심리', 60일 평균 가격은 '긴 흐름'을 뜻하므로, 최근 가격이 긴 흐름을 위로 뚫는 순간(골든크로스)을 상승 출발 신호로 봅니다.",
        "setup": [
            "단기 이평(며칠 평균): 보통 20일(약 한 달 거래일)로 둡니다. 더 빠르게 반응하고 싶으면 10일로 줄입니다.",
            "장기 이평: 보통 단기 이평의 3배 정도인 60일(약 석 달)을 씁니다. 아주 묵직하게 보려면 50일/200일 조합도 많이 씁니다.",
            "잘 맞는 종목: 뉴스 한 번에 급등락하는 작은 종목보다, 큰 흐름이 길게 이어지는 지수 ETF나 대형주에 잘 맞습니다."
        ],
        "execution": [
            "매일 장이 열리고 1분 뒤에, 어제 마감 가격까지로 단기 평균과 장기 평균을 계산합니다.",
            "주식을 안 가지고 있는데 어제 단기 평균이 장기 평균을 위로 뚫었다면, 오늘 바로 시장가로 매수합니다.",
            "주식을 가지고 있는데 어제 단기 평균이 장기 평균 아래로 내려갔다면, 가지고 있는 수량을 전부 시장가로 매도합니다."
        ],
        "caution": "주가가 오르지도 내리지도 않고 제자리에서 흔들릴 때(횡보장)는 '샀다 팔았다'를 반복하며 조금씩 손실이 날 수 있습니다. 10번 중 4번 정도만 이겨도, 크게 오를 때 길게 먹어서 수익을 내는 방식입니다."
    }),
    params: MA_PARAMS,
    build: |raw| {
        let p = resolve_params(MA_PARAMS, raw)?;
        if p["fast"] >= p["slow"] {
            return Err("단기 이평은 장기 이평보다 짧아야 합니다".to_string());
        }
        Ok(Box::new(MovingAverageCross { p, fast: vec![], slow: vec![] }))
    },
};

// ========================================================================= 2. turtle
const TURTLE_PARAMS: &[Param] = &[
    Param::int("entry", "진입 기간", 20.0, 5.0, 200.0),
    Param::int("exit", "청산 기간", 10.0, 2.0, 200.0),
    Param::int("atr_n", "ATR(N) 기간", 20.0, 5.0, 100.0),
    Param::float("stop_atr", "손절 배수 (N)", 2.0, 0.5, 10.0, 0.5),
    Param::float("risk_pct", "1N당 위험 (% 자산)", 1.0, 0.1, 10.0, 0.1),
];

pub struct Turtle {
    p: ResolvedParams,
    hh: ind::Series,
    ll: ind::Series,
    atr: ind::Series,
}

impl Strategy for Turtle {
    fn id(&self) -> &'static str { "turtle" }
    fn params_json(&self) -> Value { params_to_json(TURTLE_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["entry"] as usize).max(self.p["atr_n"] as usize) + 1 }
    fn prepare(&mut self, bars: &Bars) {
        let dc = ind::donchian(&bars.high, &bars.low, self.p["entry"] as usize, self.p["exit"] as usize);
        self.hh = dc.upper;
        self.ll = dc.lower;
        self.atr = ind::atr(&bars.high, &bars.low, &bars.close, self.p["atr_n"] as usize);
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let pos = &ctx.position;
        if pos.is_open() {
            let mut n_entry = pos.entry_atr;
            if n_entry.is_none() && pos.entry_index > 0 && (pos.entry_index as usize) <= self.atr.len() {
                n_entry = self.atr[(pos.entry_index - 1) as usize];
            }
            let hard_stop = n_entry.filter(|&v| v > 0.0).map(|n| pos.entry_price - self.p["stop_atr"] * n);
            let channel = self.ll[i];
            if hard_stop.is_none() && channel.is_none() {
                return vec![];
            }
            let (level, why) = match (hard_stop, channel) {
                (None, Some(ch)) => (ch, format!("{}일 신저가", self.p["exit"] as i64)),
                (Some(hs), Some(ch)) if ch >= hs => (ch, format!("{}일 신저가", self.p["exit"] as i64)),
                (Some(hs), _) => (hs, format!("{}N 손절", self.p["stop_atr"])),
                (None, None) => unreachable!(),
            };
            return vec![Order::stop("sell", level, None, why)];
        }
        let (Some(hh), Some(n)) = (self.hh[i], self.atr[i]) else { return vec![]; };
        if n <= 0.0 { return vec![]; }
        let qty = (ctx.equity * self.p["risk_pct"] / 100.0 / n).floor();
        if qty <= 0.0 { return vec![]; }
        vec![Order::stop("buy", hh, Some(qty), format!("{}일 신고가 돌파", self.p["entry"] as i64))]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![
            json!({"id": "donchian", "params": {"upper": self.p["entry"] as i64, "lower": self.p["exit"] as i64}}),
            json!({"id": "atr", "params": {"n": self.p["atr_n"] as i64}}),
        ]
    }
    fn last_atr(&self) -> Option<f64> {
        self.atr.last().copied().flatten()
    }
}

pub static TURTLE_SPEC: StrategySpec = StrategySpec {
    id: "turtle",
    name: "터틀 트레이딩",
    category: "swing",
    timeframe: "1d",
    theory: "리처드 데니스 · 최근 최고가 돌파 + 하루 흔들림 크기에 맞춘 안전 수량 조절",
    summary: "최근 20일 최고가를 뚫으면 사고, 최근 10일 최저가나 손절선 깨지면 팝니다",
    description: "1980년대 일반인들을 훈련시켜 큰 수익을 낸 유명한 '터틀 트레이딩' 규칙입니다. 최근 20일 중 가장 비쌌던 가격을 뚫고 올라가면 강한 상승 시작으로 보고 매수하며, 하루 평균 출렁임(ATR)에 맞춰 수량을 자동 조절합니다.",
    guide: || json!({
        "background": "1983년 리처드 데니스가 공개한 전략으로, 최근 20일 최고가 돌파 진입과 ATR 기반 리스크 수량 조절(1N당 위험 1~2%)을 결합합니다.",
        "setup": [
            "진입/청산 기간: 기본 20일 최고가 매수 / 10일 최저가 매도",
            "손절 배수(N): 기본 2배, 1N당 위험(%): 기본 1~2%"
        ],
        "execution": [
            "매일 장 시작 1분 뒤 최근 20일 최고가·10일 최저가·ATR을 계산합니다.",
            "미보유 시 안전 수량을 계산해 20일 최고가 돌파 조건주문을 겁니다.",
            "보유 시 10일 최저가와 2N 손절선 중 더 높은 가격에 매도 조건주문을 겁니다."
        ],
        "caution": "10번 중 6~7번은 작게 손절하고 3~4번의 큰 추세에서 수익을 몰아 버는 구조입니다."
    }),
    params: TURTLE_PARAMS,
    build: |raw| {
        let p = resolve_params(TURTLE_PARAMS, raw)?;
        if p["exit"] >= p["entry"] {
            return Err("청산 기간은 진입 기간보다 짧아야 합니다".to_string());
        }
        Ok(Box::new(Turtle { p, hh: vec![], ll: vec![], atr: vec![] }))
    },
};

// ========================================================================= 3. macd_trend
const MACD_PARAMS: &[Param] = &[
    Param::int("fast", "단기 EMA", 12.0, 2.0, 50.0),
    Param::int("slow", "장기 EMA", 26.0, 5.0, 100.0),
    Param::int("signal", "시그널", 9.0, 2.0, 50.0),
    Param::int("trend_ma", "추세 이평 (0=끔)", 120.0, 0.0, 300.0),
    Param::float("atr_stop", "ATR 추적 손절 (0=끔)", 2.5, 0.0, 10.0, 0.5),
];

pub struct MacdTrendSwing {
    p: ResolvedParams,
    high: Vec<f64>,
    close: Vec<f64>,
    macd: ind::Series,
    sig: ind::Series,
    trend: Option<ind::Series>,
    atr: ind::Series,
}

impl Strategy for MacdTrendSwing {
    fn id(&self) -> &'static str { "macd_trend" }
    fn params_json(&self) -> Value { params_to_json(MACD_PARAMS, &self.p) }
    fn warmup(&self) -> usize {
        ((self.p["slow"] + self.p["signal"]) as usize)
            .max(self.p["trend_ma"] as usize)
            .max(15)
    }
    fn prepare(&mut self, bars: &Bars) {
        self.high = bars.high.clone();
        self.close = bars.close.clone();
        let m = ind::macd(&bars.close, self.p["fast"] as usize, self.p["slow"] as usize, self.p["signal"] as usize);
        self.macd = m.macd;
        self.sig = m.signal;
        let tma = self.p["trend_ma"] as usize;
        self.trend = if tma > 0 { Some(ind::sma(&bars.close, tma)) } else { None };
        self.atr = ind::atr(&bars.high, &bars.low, &bars.close, 14);
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        if i < 1 { return vec![]; }
        let (Some(m0), Some(s0), Some(m1), Some(s1)) =
            (self.macd[i - 1], self.sig[i - 1], self.macd[i], self.sig[i])
        else { return vec![]; };
        if ctx.position.is_open() {
            if m0 >= s0 && m1 < s1 {
                return vec![Order::market("sell", "MACD 데드크로스")];
            }
            if self.p["atr_stop"] > 0.0 {
                if let Some(a) = self.atr[i].filter(|&v| v > 0.0) {
                    let start = if ctx.position.entry_index >= 0 {
                        ctx.position.entry_index as usize
                    } else {
                        i.saturating_sub(20)
                    };
                    let peak = if start <= i {
                        self.high[start..=i].iter().copied().fold(f64::NEG_INFINITY, f64::max)
                    } else {
                        self.high[i]
                    };
                    let trail = peak - self.p["atr_stop"] * a;
                    return vec![Order::stop("sell", trail, None, format!("ATR 추적 손절 ({}N)", self.p["atr_stop"]))];
                }
            }
            return vec![];
        }
        let trend_ok = match &self.trend {
            None => true,
            Some(ts) => ts[i].is_some_and(|t| self.close[i] > t),
        };
        if trend_ok && m0 <= s0 && m1 > s1 {
            return vec![Order::market(
                "buy",
                format!("MACD 골든크로스 ({}/{}/{})", self.p["fast"] as i64, self.p["slow"] as i64, self.p["signal"] as i64),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        let mut out = vec![json!({"id": "macd", "params": {"fast": self.p["fast"] as i64, "slow": self.p["slow"] as i64, "signal": self.p["signal"] as i64}})];
        if self.p["trend_ma"] as i64 > 0 {
            out.insert(0, json!({"id": "ma", "params": {"p1": self.p["trend_ma"] as i64, "p2": 0, "p3": 0, "p4": 0}}));
        }
        out
    }
}

pub static MACD_TREND_SPEC: StrategySpec = StrategySpec {
    id: "macd_trend",
    name: "MACD 추세 스윙",
    category: "swing",
    timeframe: "1d",
    theory: "제럴드 아펠 · 상승 속도가 붙을 때 사고, 고점에서 밀리면 수익 지키며 매도",
    summary: "큰 흐름이 상승일 때 MACD 상승 신호에 사고, 하락 신호나 추적 손절선 깨지면 팝니다",
    description: "MACD 지표에 120일 장기 추세 필터와 ATR 추적 손절을 결합한 스윙 전략입니다.",
    guide: || json!({
        "background": "MACD 상승 가속 신호에 120일선 상승 필터와 보유 최고가 대비 ATR 추적 손절을 결합해 하락장 함정을 피하고 수익을 지킵니다.",
        "setup": ["표준 12/26/9, 추세 이평 120일, ATR 추적 손절 2.5배 권장"],
        "execution": ["120일선 위에서 MACD 골든크로스 시 매수, 데드크로스 또는 ATR 추적 손절 시 매도"],
        "caution": "급등락 테마주에서는 손절이 잦을 수 있습니다."
    }),
    params: MACD_PARAMS,
    build: |raw| {
        let p = resolve_params(MACD_PARAMS, raw)?;
        if p["fast"] >= p["slow"] {
            return Err("단기 EMA는 장기 EMA보다 짧아야 합니다".to_string());
        }
        Ok(Box::new(MacdTrendSwing { p, high: vec![], close: vec![], macd: vec![], sig: vec![], trend: None, atr: vec![] }))
    },
};

// ========================================================================= 4. bollinger
const BB_PARAMS: &[Param] = &[
    Param::int("n", "기간", 20.0, 5.0, 200.0),
    Param::float("k", "표준편차 배수", 2.0, 0.5, 4.0, 0.1),
];

pub struct BollingerReversion {
    p: ResolvedParams,
    close: Vec<f64>,
    lower: ind::Series,
    mid: ind::Series,
}

impl Strategy for BollingerReversion {
    fn id(&self) -> &'static str { "bollinger" }
    fn params_json(&self) -> Value { params_to_json(BB_PARAMS, &self.p) }
    fn warmup(&self) -> usize { self.p["n"] as usize }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        let bb = ind::bollinger(&bars.close, self.p["n"] as usize, self.p["k"]);
        self.lower = bb.lower;
        self.mid = bb.mid;
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let (Some(lower), Some(mid)) = (self.lower[i], self.mid[i]) else { return vec![]; };
        let c = self.close[i];
        if !ctx.position.is_open() && c < lower {
            return vec![Order::market("buy", "종가 < 하단 밴드")];
        }
        if ctx.position.is_open() && c > mid {
            return vec![Order::market("sell", "종가 > 중심선")];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "bb", "params": {"n": self.p["n"] as i64, "k": self.p["k"]}})]
    }
}

pub static BOLLINGER_SPEC: StrategySpec = StrategySpec {
    id: "bollinger",
    name: "볼린저 밴드 회귀",
    category: "swing",
    timeframe: "1d",
    theory: "존 볼린저 · 고무줄처럼 너무 내려가면 제자리로 돌아오는 원리",
    summary: "주가가 평소 범위(하단선) 밑으로 과하게 떨어지면 사고, 평균 가격으로 돌아오면 팝니다",
    description: "주가가 평소 범위(볼린저 하단선) 아래로 과도하게 밀릴 때 매수하고 중심선(20일 평균)으로 회복하면 매도하는 평균회귀 전략입니다.",
    guide: || json!({
        "background": "일시적 과매도로 볼린저 하단 밴드 아래로 내려간 우량주·지수 ETF의 평균 회귀 성질을 활용합니다.",
        "setup": ["기간 20일, 표준편차 배수 2.0 권장"],
        "execution": ["종가가 하단 밴드 미만이면 매수, 중심선 초과 시 매도"],
        "caution": "구조적 악재로 계속 하락하는 개별 소형주보다 대표 지수 ETF나 우량주에 사용하세요."
    }),
    params: BB_PARAMS,
    build: |raw| {
        let p = resolve_params(BB_PARAMS, raw)?;
        Ok(Box::new(BollingerReversion { p, close: vec![], lower: vec![], mid: vec![] }))
    },
};

// ========================================================================= 5. rsi2
const RSI2_PARAMS: &[Param] = &[
    Param::int("rsi_n", "RSI 기간", 2.0, 2.0, 30.0),
    Param::int("entry", "매수 RSI 기준", 10.0, 1.0, 50.0),
    Param::int("trend", "추세 이평 (0=끔)", 200.0, 0.0, 400.0),
    Param::int("exit_ma", "청산 이평", 5.0, 2.0, 50.0),
];

pub struct Rsi2MeanReversion {
    p: ResolvedParams,
    close: Vec<f64>,
    rsi: ind::Series,
    trend: Option<ind::Series>,
    exit_ma: ind::Series,
}

impl Strategy for Rsi2MeanReversion {
    fn id(&self) -> &'static str { "rsi2" }
    fn params_json(&self) -> Value { params_to_json(RSI2_PARAMS, &self.p) }
    fn warmup(&self) -> usize {
        (self.p["trend"] as usize)
            .max(self.p["exit_ma"] as usize)
            .max(self.p["rsi_n"] as usize + 1)
    }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        self.rsi = ind::rsi(&bars.close, self.p["rsi_n"] as usize);
        let tr = self.p["trend"] as usize;
        self.trend = if tr > 0 { Some(ind::sma(&bars.close, tr)) } else { None };
        self.exit_ma = ind::sma(&bars.close, self.p["exit_ma"] as usize);
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let c = self.close[i];
        if ctx.position.is_open() {
            if self.exit_ma[i].is_some_and(|m| c > m) {
                return vec![Order::market("sell", format!("종가 > {}일 이평", self.p["exit_ma"] as i64))];
            }
            return vec![];
        }
        let trend_ok = match &self.trend {
            None => true,
            Some(ts) => ts[i].is_some_and(|t| c > t),
        };
        if let Some(r) = self.rsi[i] {
            if r < self.p["entry"] && trend_ok {
                return vec![Order::market(
                    "buy",
                    format!("RSI({}) {:.1} < {}", self.p["rsi_n"] as i64, r, self.p["entry"] as i64),
                )];
            }
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![
            json!({"id": "ma", "params": {"p1": self.p["exit_ma"] as i64, "p2": self.p["trend"] as i64, "p3": 0, "p4": 0}}),
            json!({"id": "rsi", "params": {"n": self.p["rsi_n"] as i64}}),
        ]
    }
}

pub static RSI2_SPEC: StrategySpec = StrategySpec {
    id: "rsi2",
    name: "RSI(2) 평균회귀 (래리 코너스)",
    category: "swing",
    timeframe: "1d",
    theory: "래리 코너스 · 잘 나가는 종목이 잠깐 세일할 때 사서 빠르게 되파는 전략",
    summary: "장기 상승 중인 종목이 2~3일 급락하면 사고, 5일 평균 가격을 회복하면 바로 팝니다",
    description: "200일 장기 상승장 종목이 단기 RSI(2) 10 미만으로 과매도될 때 매수하고 5일선 회복 시 청산하는 래리 코너스 전략입니다.",
    guide: || json!({
        "background": "장기 상승 추세(200일선 위) 종목의 단기 급락(RSI(2) < 10) 후 빠른 반등을 노리는 고승률 전략입니다.",
        "setup": ["RSI 기간 2일, 매수 기준 10, 추세 이평 200일, 청산 이평 5일 권장"],
        "execution": ["200일선 위 & RSI(2) < 10 매수, 5일선 회복 시 매도"],
        "caution": "기본 전략에 손절선이 없으므로 공통 손절 보호막을 함께 활용하는 것을 권장합니다."
    }),
    params: RSI2_PARAMS,
    build: |raw| {
        let p = resolve_params(RSI2_PARAMS, raw)?;
        Ok(Box::new(Rsi2MeanReversion { p, close: vec![], rsi: vec![], trend: None, exit_ma: vec![] }))
    },
};

// ========================================================================= 6. ibs_reversion
const IBS_PARAMS: &[Param] = &[
    Param::float("entry_ibs", "매수 IBS 기준", 0.2, 0.05, 0.5, 0.05),
    Param::float("exit_ibs", "청산 IBS 기준", 0.8, 0.5, 0.95, 0.05),
    Param::int("trend_ma", "추세 이평 (0=끔)", 200.0, 0.0, 400.0),
    Param::int("max_hold", "최대 보유일", 5.0, 1.0, 30.0),
];

pub struct IbsMeanReversion {
    p: ResolvedParams,
    close: Vec<f64>,
    ibs: Vec<f64>,
    trend: Option<ind::Series>,
}

impl Strategy for IbsMeanReversion {
    fn id(&self) -> &'static str { "ibs_reversion" }
    fn params_json(&self) -> Value { params_to_json(IBS_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["trend_ma"] as usize).max(2) }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        self.ibs = (0..bars.len())
            .map(|i| {
                let h = bars.high[i];
                let l = bars.low[i];
                if h <= l { 0.5 } else { (bars.close[i] - l) / (h - l) }
            })
            .collect();
        let tr = self.p["trend_ma"] as usize;
        self.trend = if tr > 0 { Some(ind::sma(&bars.close, tr)) } else { None };
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let ibs = self.ibs[i];
        if ctx.position.is_open() {
            let held = if ctx.position.entry_index >= 0 {
                (i as isize - ctx.position.entry_index).max(0) as usize
            } else { 0 };
            if ibs > self.p["exit_ibs"] {
                return vec![Order::market("sell", format!("IBS {ibs:.2} > {}", self.p["exit_ibs"]))];
            }
            if held >= self.p["max_hold"] as usize {
                return vec![Order::market("sell", format!("최대 보유일({}일) 청산", self.p["max_hold"] as i64))];
            }
            return vec![];
        }
        let trend_ok = match &self.trend {
            None => true,
            Some(ts) => ts[i].is_some_and(|t| self.close[i] > t),
        };
        if trend_ok && ibs < self.p["entry_ibs"] {
            return vec![Order::market("buy", format!("IBS {ibs:.2} < {}", self.p["entry_ibs"]))];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        if self.p["trend_ma"] as i64 == 0 {
            vec![]
        } else {
            vec![json!({"id": "ma", "params": {"p1": self.p["trend_ma"] as i64, "p2": 0, "p3": 0, "p4": 0}})]
        }
    }
}

pub static IBS_SPEC: StrategySpec = StrategySpec {
    id: "ibs_reversion",
    name: "IBS 내부봉 평균회귀",
    category: "swing",
    timeframe: "1d",
    theory: "파고니디스 · 오늘 유난히 바닥에서 끝난 종목의 다음 날 반등 노리기",
    summary: "상승장 종목이 오늘 하루 최저가 근처에서 마감하면 사고, 최고가 근처로 반등하면 팝니다",
    description: "하루 변동폭 대비 종가 위치(IBS)가 바닥권(0.2 미만)일 때 매수하고 고점권(0.8 초과) 또는 최대 보유일 도달 시 청산합니다.",
    guide: || json!({
        "background": "지수 ETF에서 장 마감 매도 쏠림으로 바닥권에 끝난 날 다음 거래일 반등하는 통계적 특성을 활용합니다.",
        "setup": ["매수 0.2 / 청산 0.8 / 200일선 필터 / 최대 보유일 5일 권장"],
        "execution": ["200일선 위 & IBS < 0.2 매수, IBS > 0.8 또는 5일 경과 시 매도"],
        "caution": "거래비용이 저렴한 지수 ETF나 대형주에 적합합니다."
    }),
    params: IBS_PARAMS,
    build: |raw| {
        let p = resolve_params(IBS_PARAMS, raw)?;
        if p["entry_ibs"] >= p["exit_ibs"] {
            return Err("매수 IBS 기준은 청산 IBS 기준보다 작아야 합니다".to_string());
        }
        Ok(Box::new(IbsMeanReversion { p, close: vec![], ibs: vec![], trend: None }))
    },
};

// ========================================================================= 7. ts_momentum
const TSMOM_PARAMS: &[Param] = &[
    Param::int("lookback", "모멘텀 기간 (일)", 60.0, 10.0, 252.0),
    Param::int("vol_window", "변동성 기간 (일)", 20.0, 5.0, 120.0),
    Param::float("target_vol", "목표 연변동성 (%)", 15.0, 5.0, 60.0, 1.0),
];

pub struct TimeSeriesMomentum {
    p: ResolvedParams,
    close: Vec<f64>,
    ann_vol: ind::Series,
}

impl Strategy for TimeSeriesMomentum {
    fn id(&self) -> &'static str { "ts_momentum" }
    fn params_json(&self) -> Value { params_to_json(TSMOM_PARAMS, &self.p) }
    fn warmup(&self) -> usize { (self.p["lookback"] as usize).max(self.p["vol_window"] as usize) + 1 }
    fn prepare(&mut self, bars: &Bars) {
        self.close = bars.close.clone();
        let mut rets = Vec::with_capacity(bars.len());
        if !bars.is_empty() {
            rets.push(0.0);
            for i in 1..bars.len() {
                rets.push(if bars.close[i - 1] > 0.0 {
                    bars.close[i] / bars.close[i - 1] - 1.0
                } else { 0.0 });
            }
        }
        let sd = ind::stdev(&rets, self.p["vol_window"] as usize);
        let root252 = 252.0f64.sqrt();
        self.ann_vol = sd.into_iter().map(|s| s.map(|v| v * root252)).collect();
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let lb = self.p["lookback"] as usize;
        if i < lb || self.close[i - lb] <= 0.0 {
            return vec![];
        }
        let mom = self.close[i] / self.close[i - lb] - 1.0;
        if ctx.position.is_open() {
            if mom <= 0.0 {
                return vec![Order::market("sell", format!("{lb}일 모멘텀 음수 전환 ({:+.1}%)", mom * 100.0))];
            }
            return vec![];
        }
        if mom > 0.0 && self.close[i] > 0.0 {
            if let Some(vol) = self.ann_vol[i].filter(|&v| v > 0.0) {
                let weight = ((self.p["target_vol"] / 100.0) / vol).min(1.0);
                let qty = (ctx.equity * weight / self.close[i]).floor();
                if qty > 0.0 {
                    let mut o = Order::market(
                        "buy",
                        format!("{lb}일 모멘텀 {:+.1}% · 비중 {:.0}%", mom * 100.0, weight * 100.0),
                    );
                    o.qty = Some(qty);
                    return vec![o];
                }
            }
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "ma", "params": {"p1": self.p["lookback"] as i64, "p2": 0, "p3": 0, "p4": 0}})]
    }
}

pub static TSMOM_SPEC: StrategySpec = StrategySpec {
    id: "ts_momentum",
    name: "시계열 모멘텀 (TSMOM)",
    category: "swing",
    timeframe: "1d",
    theory: "모스코위츠·오이·페데르센 · 오르는 중이면 타고, 시장이 불안하면 적게 넣기",
    summary: "몇 달 전보다 올랐으면 투자하고 내렸으면 현금 보유, 흔들림이 클수록 비중을 줄입니다",
    description: "과거 수익률이 양수면 상승 추세로 진입하되 최근 변동성에 반비례해 투자 비중을 자동 조절하는 변동성 타겟팅 전략입니다.",
    guide: || json!({
        "background": "모스코위츠·오이·페데르센(2012)의 시계열 모멘텀 논문 기반으로, 변동성이 커질수록 비중을 줄여 하락장 충격을 방어합니다.",
        "setup": ["모멘텀 기간 60일, 변동성 기간 20일, 목표 연변동성 15% 권장"],
        "execution": ["양수 모멘텀 시 변동성 비율만큼 매수, 음수 전환 시 전량 현금화"],
        "caution": "변동성이 큰 급등장에서는 전액 투자보다 수익이 다소 적을 수 있지만 하락장 방어력이 뛰어납니다."
    }),
    params: TSMOM_PARAMS,
    build: |raw| {
        let p = resolve_params(TSMOM_PARAMS, raw)?;
        Ok(Box::new(TimeSeriesMomentum { p, close: vec![], ann_vol: vec![] }))
    },
};
