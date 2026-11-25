//! Three quantitative equity strategies adapted from Zura Kakushadze & Juan Andrés Serur,
//! *151 Trading Strategies* (2018) for KR/US spot (long-only) stock markets (MT-25):
//!
//! 1. `hp_filter` (Section 8.1) — Strictly causal rolling Hodrick–Prescott filter trend crossover.
//! 2. `knn` (Section 3.17) — Strictly causal rolling k-Nearest Neighbors pattern matching.
//! 3. `alpha_combo` (Section 3.20, Kakushadze–Yu 2017) — 36-alpha residual-regression ensemble.

use serde_json::{json, Value};

use super::{params_to_json, resolve_params, Param, ResolvedParams, Strategy, StrategySpec};
use crate::quant::engine::{Bars, Context, Order};
use crate::quant::indicators as ind;

// ========================================================================= Linear algebra helpers

/// Solves a small dense linear system `A x = b` via Gaussian elimination with partial pivoting.
/// Returns `None` if singular or non-finite.
pub fn solve_linear(a_in: &[Vec<f64>], b_in: &[f64]) -> Option<Vec<f64>> {
    let n = b_in.len();
    if n == 0 || a_in.len() != n {
        return None;
    }
    let mut a = a_in.to_vec();
    let mut b = b_in.to_vec();
    for col in 0..n {
        let mut piv = col;
        let mut best = a[col][col].abs();
        for (r, row) in a.iter().enumerate().skip(col + 1) {
            let v = row[col].abs();
            if v > best {
                best = v;
                piv = r;
            }
        }
        if !best.is_finite() || best < 1e-14 {
            return None;
        }
        if piv != col {
            a.swap(col, piv);
            b.swap(col, piv);
        }
        let diag = a[col][col];
        for r in (col + 1)..n {
            let f = a[r][col] / diag;
            if f == 0.0 {
                continue;
            }
            a[r][col] = 0.0;
            for c in (col + 1)..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = b[i];
        for c in (i + 1)..n {
            s -= a[i][c] * x[c];
        }
        let v = s / a[i][i];
        if !v.is_finite() {
            return None;
        }
        x[i] = v;
    }
    Some(x)
}

/// Computes the exact linear weights `w` such that `w · s` equals the final smoothed point
/// `S*(W-1)` of the Hodrick–Prescott filter on any price slice `s` of length `W`:
///
/// `(I + λ DᵀD) w = e_{W-1}`
///
/// Solved in `O(W)` time via symmetric pentadiagonal Cholesky factorization (`A = L Lᵀ`).
/// Because `w` depends only on `(window, lambda)`, computing the strictly causal rolling HP trend
/// over `N` bars takes `O(N · W)` dot products with zero look-ahead bias and machine precision.
pub fn hp_endpoint_weights(window: usize, lambda: f64) -> Vec<f64> {
    if window == 0 {
        return vec![];
    }
    if window < 3 || lambda <= 0.0 {
        let mut w = vec![0.0; window];
        w[window - 1] = 1.0;
        return w;
    }
    let mut d0 = vec![0.0; window];
    let mut d1 = vec![0.0; window.saturating_sub(1)];
    let mut d2 = vec![0.0; window.saturating_sub(2)];

    for i in 0..window {
        let c0 = if window == 3 {
            if i == 1 { 4.0 } else { 1.0 }
        } else if i == 0 || i == window - 1 {
            1.0
        } else if i == 1 || i == window - 2 {
            5.0
        } else {
            6.0
        };
        d0[i] = 1.0 + lambda * c0;
    }
    for i in 0..(window - 1) {
        let c1 = if i == 0 || i == window - 2 { -2.0 } else { -4.0 };
        d1[i] = lambda * c1;
    }
    for v in &mut d2 {
        *v = lambda;
    }

    // Banded Cholesky: L has l0[i] = L_{i,i}, l1[i] = L_{i+1,i}, l2[i] = L_{i+2,i}
    let mut l0 = vec![0.0; window];
    let mut l1 = vec![0.0; window.saturating_sub(1)];
    let mut l2 = vec![0.0; window.saturating_sub(2)];
    for i in 0..window {
        let mut s = d0[i];
        if i >= 1 {
            s -= l1[i - 1] * l1[i - 1];
        }
        if i >= 2 {
            s -= l2[i - 2] * l2[i - 2];
        }
        l0[i] = s.max(1e-18).sqrt();
        if i + 1 < window {
            let mut off1 = d1[i];
            if i >= 1 {
                off1 -= l1[i - 1] * l2[i - 1];
            }
            l1[i] = off1 / l0[i];
        }
        if i + 2 < window {
            l2[i] = d2[i] / l0[i];
        }
    }

    // Forward solve L y = e_{W-1}
    let mut y = vec![0.0; window];
    y[window - 1] = 1.0 / l0[window - 1];

    // Backward solve Lᵀ w = y
    let mut w = vec![0.0; window];
    for i in (0..window).rev() {
        let mut s = y[i];
        if i + 1 < window {
            s -= l1[i] * w[i + 1];
        }
        if i + 2 < window {
            s -= l2[i] * w[i + 2];
        }
        w[i] = s / l0[i];
    }
    w
}

/// Strictly causal rolling HP filter trend: `trend[t]` uses only `prices[t+1-window ..= t]`.
pub fn causal_hp_trend(prices: &[f64], window: usize, lambda: f64) -> ind::Series {
    let n = prices.len();
    let mut out = vec![None; n];
    if window < 3 || n < window {
        return out;
    }
    let w = hp_endpoint_weights(window, lambda);
    for t in (window - 1)..n {
        let slice = &prices[(t + 1 - window)..=t];
        let mut acc = 0.0;
        for (wk, pk) in w.iter().zip(slice.iter()) {
            acc += wk * pk;
        }
        if acc.is_finite() {
            out[t] = Some(acc);
        }
    }
    out
}

fn sma_opt(series: &[Option<f64>], n: usize) -> ind::Series {
    let len = series.len();
    let mut out = vec![None; len];
    if n == 0 || len < n {
        return out;
    }
    let mut sum = 0.0;
    let mut valid = 0usize;
    for i in 0..len {
        if let Some(v) = series[i] {
            sum += v;
            valid += 1;
        }
        if i >= n {
            if let Some(old) = series[i - n] {
                sum -= old;
                valid -= 1;
            }
        }
        if i + 1 >= n && valid == n {
            out[i] = Some(sum / n as f64);
        }
    }
    out
}

// ========================================================================= 1. hp_filter (Sec 8.1)

const HP_PARAMS: &[Param] = &[
    Param::int("window", "롤링 윈도우 (일)", 120.0, 30.0, 365.0),
    Param::int("fast", "추세 단기 이평", 10.0, 2.0, 60.0),
    Param::int("slow", "추세 장기 이평", 30.0, 5.0, 150.0),
    Param::int("lambda", "부드러움 계수 λ", 100000.0, 100.0, 10000000.0),
];

pub struct HpFilterTrend {
    p: ResolvedParams,
    fast_ma: ind::Series,
    slow_ma: ind::Series,
}

impl Strategy for HpFilterTrend {
    fn id(&self) -> &'static str {
        "hp_filter"
    }
    fn params_json(&self) -> Value {
        params_to_json(HP_PARAMS, &self.p)
    }
    fn warmup(&self) -> usize {
        self.p["window"] as usize + self.p["slow"] as usize + 1
    }
    fn prepare(&mut self, bars: &Bars) {
        let trend = causal_hp_trend(&bars.close, self.p["window"] as usize, self.p["lambda"]);
        self.fast_ma = sma_opt(&trend, self.p["fast"] as usize);
        self.slow_ma = sma_opt(&trend, self.p["slow"] as usize);
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        if i < 1 {
            return vec![];
        }
        let (Some(f0), Some(s0), Some(f1), Some(s1)) = (
            self.fast_ma[i - 1],
            self.slow_ma[i - 1],
            self.fast_ma[i],
            self.slow_ma[i],
        ) else {
            return vec![];
        };
        let fn_ = self.p["fast"] as i64;
        let sn_ = self.p["slow"] as i64;
        if !ctx.position.is_open() && f0 <= s0 && f1 > s1 {
            return vec![Order::market(
                "buy",
                format!("HP 추세 골든크로스 ({fn_}/{sn_})"),
            )];
        }
        if ctx.position.is_open() && f1 < s1 {
            return vec![Order::market(
                "sell",
                format!("HP 추세 하락 전환 ({fn_}/{sn_})"),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({
            "id": "ma",
            "params": {"p1": self.p["fast"] as i64, "p2": self.p["slow"] as i64, "p3": 0, "p4": 0}
        })]
    }
}

pub static HP_FILTER_SPEC: StrategySpec = StrategySpec {
    id: "hp_filter",
    name: "[151전략] HP 필터 추세추종",
    category: "swing",
    timeframe: "1d",
    theory: "《151 Trading Strategies》 8.1절 · 거시경제 노이즈 제거 필터로 잔파도 속임수 차단",
    summary: "주가의 잔파도(노이즈)를 HP 필터로 먼저 지운 뒤, 매끄러운 추세선의 단기/장기 교차로 매매합니다",
    description: "노이즈 캔슬링 헤드폰처럼 거시경제학의 Hodrick–Prescott(HP) 필터로 주가 흔들림을 먼저 걸러내고 부드러운 추세선(S*) 위에서 이동평균 교차를 계산합니다. 매 봉마다 과거 구간만 롤링 계산해 미래 엿보기(Look-ahead)를 원천 차단했습니다.",
    guide: || json!({
        "background": "전 WorldQuant 매니징 디렉터 Zura Kakushadze의 《151 Trading Strategies》 8.1절 전략을 현물 주식(롱-온리)용으로 이식했습니다. 일반 이동평균 교차는 잔파도에 속아 잦은 손절이 나기 쉽지만, HP 필터로 고주파 노이즈를 먼저 걷어내면 휩소(속임수 신호)가 줄어듭니다.",
        "setup": [
            "롤링 윈도우(기본 120일): 매일 과거 120일치만 잘라 HP 추세를 새로 계산합니다(미래 참조 0%).",
            "추세 단기/장기 이평(기본 10일/30일): 노이즈가 제거된 추세선 위의 골든·데드크로스 기준입니다.",
            "부드러움 계수 λ(기본 100,000): 클수록 추세선이 완만해져 속임수는 줄고 반응은 묵직해집니다."
        ],
        "execution": [
            "매일 장 마감 뒤 과거 윈도우만 사용해 HP 추세선 끝점을 계산합니다.",
            "미보유 상태에서 HP 추세 단기 이평이 장기 이평을 위로 돌파하면 다음 시초가에 매수합니다.",
            "보유 중 HP 추세 단기 이평이 장기 이평 아래로 꺾이면 전량 매도해 현금으로 대피합니다."
        ],
        "caution": "양방향 급등락이 심한 횡보장에서는 크로스가 잦아질 수 있고, λ를 지나치게 크게 잡으면 추세 전환 반응이 늦어질 수 있습니다."
    }),
    params: HP_PARAMS,
    build: |raw| {
        let p = resolve_params(HP_PARAMS, raw)?;
        if p["fast"] >= p["slow"] {
            return Err("추세 단기 이평은 장기 이평보다 짧아야 합니다".to_string());
        }
        Ok(Box::new(HpFilterTrend {
            p,
            fast_ma: vec![],
            slow_ma: vec![],
        }))
    },
};

// ========================================================================= 2. knn (Sec 3.17)

const KNN_PARAMS: &[Param] = &[
    Param::int("horizon", "예측 기간 T (일)", 5.0, 1.0, 20.0),
    Param::int("train_window", "과거 학습 기간 (일)", 250.0, 60.0, 750.0),
    Param::float("entry_pct", "매수 예측 기준 z1 (%)", 1.0, 0.1, 5.0, 0.1),
    Param::float("exit_pct", "청산 예측 기준 z2 (%)", 0.0, -3.0, 3.0, 0.1),
];

pub struct KnnPattern {
    p: ResolvedParams,
    preds: ind::Series,
    k_used: usize,
}

impl KnnPattern {
    fn compute_features(bars: &Bars) -> Vec<Option<[f64; 5]>> {
        let n = bars.len();
        let p5 = ind::sma(&bars.close, 5);
        let p20 = ind::sma(&bars.close, 20);
        let p60 = ind::sma(&bars.close, 60);
        let v5 = ind::sma(&bars.volume, 5);
        let v20 = ind::sma(&bars.volume, 20);
        let v60 = ind::sma(&bars.volume, 60);
        let mut out = vec![None; n];
        for i in 0..n {
            let c = bars.close[i];
            if c <= 0.0 {
                continue;
            }
            let (Some(a), Some(b), Some(d), Some(va), Some(vb), Some(vc)) =
                (p5[i], p20[i], p60[i], v5[i], v20[i], v60[i])
            else {
                continue;
            };
            let vr5 = if vc > 0.0 { (va / vc).clamp(0.0, 10.0) } else { 1.0 };
            let vr20 = if vc > 0.0 { (vb / vc).clamp(0.0, 10.0) } else { 1.0 };
            out[i] = Some([a / c - 1.0, b / c - 1.0, d / c - 1.0, vr5, vr20]);
        }
        out
    }
}

impl Strategy for KnnPattern {
    fn id(&self) -> &'static str {
        "knn"
    }
    fn params_json(&self) -> Value {
        params_to_json(KNN_PARAMS, &self.p)
    }
    fn warmup(&self) -> usize {
        60 + self.p["train_window"] as usize + self.p["horizon"] as usize + 1
    }
    fn prepare(&mut self, bars: &Bars) {
        let n = bars.len();
        let horizon = self.p["horizon"] as usize;
        let train_w = self.p["train_window"] as usize;
        let feats = Self::compute_features(bars);
        let mut preds = vec![None; n];
        let mut last_k = 0usize;

        for i in (horizon + 20)..n {
            let Some(cur_f) = feats[i] else {
                continue;
            };
            // Strictly causal training samples: j + horizon <= i so target Y(j) uses ONLY bars <= i
            let max_j = i - horizon;
            let min_j = max_j.saturating_sub(train_w.saturating_sub(1));

            let mut samples: Vec<([f64; 5], f64)> = Vec::with_capacity(train_w);
            for j in min_j..=max_j {
                if let Some(fj) = feats[j] {
                    let c0 = bars.close[j];
                    let c1 = bars.close[j + horizon];
                    if c0 > 0.0 && c1 > 0.0 {
                        samples.push((fj, c1 / c0 - 1.0));
                    }
                }
            }
            if samples.len() < 16 {
                continue;
            }
            // Min-max normalization fitted ONLY on training samples
            let mut lo = [f64::INFINITY; 5];
            let mut hi = [f64::NEG_INFINITY; 5];
            for (f, _) in &samples {
                for a in 0..5 {
                    if f[a] < lo[a] {
                        lo[a] = f[a];
                    }
                    if f[a] > hi[a] {
                        hi[a] = f[a];
                    }
                }
            }
            let norm = |f: &[f64; 5]| -> [f64; 5] {
                let mut z = [0.0; 5];
                for a in 0..5 {
                    let span = hi[a] - lo[a];
                    if span > 1e-12 {
                        z[a] = ((f[a] - lo[a]) / span).clamp(-0.5, 1.5);
                    }
                }
                z
            };
            let cur_z = norm(&cur_f);
            let mut dists: Vec<(f64, f64)> = samples
                .iter()
                .map(|(f, y)| {
                    let z = norm(f);
                    let mut d2 = 0.0;
                    for a in 0..5 {
                        let diff = cur_z[a] - z[a];
                        d2 += diff * diff;
                    }
                    (d2, *y)
                })
                .collect();

            let k = (samples.len() as f64).sqrt().floor() as usize;
            let k = k.clamp(3, samples.len());
            last_k = k;
            dists.select_nth_unstable_by(k - 1, |a, b| a.0.total_cmp(&b.0));
            let mean_y = dists[..k].iter().map(|(_, y)| *y).sum::<f64>() / k as f64;
            if mean_y.is_finite() {
                preds[i] = Some(mean_y);
            }
        }
        self.preds = preds;
        self.k_used = last_k;
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let Some(yhat) = self.preds[i] else {
            return vec![];
        };
        let z1 = self.p["entry_pct"] / 100.0;
        let z2 = self.p["exit_pct"] / 100.0;
        let k = self.k_used.max(3);
        if !ctx.position.is_open() && yhat > z1 {
            return vec![Order::market(
                "buy",
                format!("kNN 예측 {:+.2}% > {:+.1}% (유사 {k}일 평균)", yhat * 100.0, self.p["entry_pct"]),
            )];
        }
        if ctx.position.is_open() && yhat <= z2 {
            return vec![Order::market(
                "sell",
                format!("kNN 예측 {:+.2}% ≤ {:+.1}% 청산", yhat * 100.0, self.p["exit_pct"]),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "ma", "params": {"p1": 5, "p2": 20, "p3": 60, "p4": 0}})]
    }
}

pub static KNN_SPEC: StrategySpec = StrategySpec {
    id: "knn",
    name: "[151전략] kNN 과거 유사 패턴 매칭",
    category: "swing",
    timeframe: "1d",
    theory: "《151 Trading Strategies》 3.17절 · 오늘과 가장 닮은 과거 k일의 다음 흐름으로 예측",
    summary: "오늘의 주가·거래량 모양과 가장 비슷했던 과거 날짜들을 찾아, 그날 이후 평균 수익률이 높으면 삽니다",
    description: "복잡한 블랙박스 신경망 대신, 과거 기록에서 오늘과 가장 비슷한 k일(k=⌊√학습일⌋)을 유클리드 거리로 찾아 그 이후 T일 수익률을 평균 내는 투명한 머신러닝 전략입니다. 가격 수준 변화에 흔들리지 않도록 이동평균 비율과 거래량 비율 피처를 사용하며 미래 누수를 원천 차단했습니다.",
    guide: || json!({
        "background": "《151 Trading Strategies》 3.17절의 k-최근접 이웃(kNN) 회귀 전략을 현물 주식(롱-온리)으로 구현했습니다. 베테랑 기상 예보관이 과거 유사한 기압 배치 날들을 찾아 다음 날 날씨를 예측하듯, 5·20·60일 가격 이격과 5·20일 거래량 비율이 오늘과 가장 닮은 과거 k일을 찾아 평균 수익률을 계산합니다.",
        "setup": [
            "예측 기간 T(기본 5일): 유사한 과거 날짜 이후 며칠 뒤 수익률을 학습·예측할지 정합니다.",
            "과거 학습 기간(기본 250일 ≈ 1년): 최근 1년 기록 중 가장 비슷한 k=15일을 자동으로 찾습니다.",
            "매수 기준 z1(기본 +1.0%) · 청산 기준 z2(기본 0.0%): 진입과 청산 문턱을 다르게 두어 잦은 왕복 매매를 막습니다."
        ],
        "execution": [
            "매일 장 마감 후, 미래 수익률이 이미 확정된 과거 구간(최근 T일 제외)에서만 학습 정규화를 수행합니다.",
            "유사 k일의 평균 T일 뒤 수익률 예측치가 +z1%를 넘으면 매수합니다.",
            "보유 중 예측치가 z2% 이하로 내려오면 매도하여 현금으로 돌아옵니다."
        ],
        "caution": "과거 학습 기간에 한 번도 없던 완전히 새로운 시장 국면(급격한 체제 전환)에서는 과거 이웃의 설명력이 떨어질 수 있습니다."
    }),
    params: KNN_PARAMS,
    build: |raw| {
        let p = resolve_params(KNN_PARAMS, raw)?;
        if p["entry_pct"] <= p["exit_pct"] {
            return Err("매수 예측 기준(z1)은 청산 예측 기준(z2)보다 커야 합니다".to_string());
        }
        Ok(Box::new(KnnPattern {
            p,
            preds: vec![],
            k_used: 0,
        }))
    },
};

// ========================================================================= 3. alpha_combo (Sec 3.20)

/// Exact 11-step Kakushadze–Yu (2017) Alpha Combo weight procedure (*151 Trading Strategies*, Sec 3.20).
///
/// - `r`: `[N_alphas][M + 1]` matrix where column `0` is the most recent period's return.
/// - `d`: recent expected-return window (`1 <= d <= M + 1`).
/// - Returns normalized weights `w` with `Σ |w_i| = 1.0` (or all zeros if degenerate).
pub fn alpha_combo_weights(r: &[Vec<f64>], d: usize) -> Vec<f64> {
    let n_total = r.len();
    if n_total < 3 {
        return vec![0.0; n_total];
    }
    let m1 = r[0].len();
    if m1 < 3 || r.iter().any(|row| row.len() != m1) {
        return vec![0.0; n_total];
    }
    let m = m1 - 1;
    let d_eff = d.clamp(1, m1);

    // Filter out flat/dead alphas whose variance is ~0 so division by sigma never blows up
    let mut active_idx = Vec::with_capacity(n_total);
    let mut sigmas = Vec::with_capacity(n_total);
    let mut y_rows: Vec<Vec<f64>> = Vec::with_capacity(n_total);
    let mut e_tilde = Vec::with_capacity(n_total);

    for (idx, row) in r.iter().enumerate() {
        let mean = row.iter().sum::<f64>() / m1 as f64;
        let x: Vec<f64> = row.iter().map(|&v| v - mean).collect();
        let var = x.iter().map(|v| v * v).sum::<f64>() / m as f64;
        let sigma = var.sqrt();
        if !sigma.is_finite() || sigma < 1e-10 {
            continue;
        }
        let y: Vec<f64> = x[..m].iter().map(|&v| v / sigma).collect();
        let e_i = row[..d_eff].iter().sum::<f64>() / d_eff as f64;
        active_idx.push(idx);
        sigmas.push(sigma);
        y_rows.push(y);
        e_tilde.push(e_i / sigma);
    }

    let n = active_idx.len();
    let cols = m - 1;
    if n <= cols || cols == 0 {
        return vec![0.0; n_total];
    }

    // Step 6-7: Cross-sectional demean across alphas, keep first M-1 columns -> Lambda (n x cols)
    let mut lam = vec![vec![0.0; cols]; n];
    for c in 0..cols {
        let col_mean = y_rows.iter().map(|row| row[c]).sum::<f64>() / n as f64;
        for i in 0..n {
            lam[i][c] = y_rows[i][c] - col_mean;
        }
    }

    // Step 9: OLS without intercept: (Λᵀ Λ + eps I) β = Λᵀ E_tilde -> residuals ε = E_tilde - Λ β
    let mut ata = vec![vec![0.0; cols]; cols];
    let mut atb = vec![0.0; cols];
    for c1 in 0..cols {
        for c2 in c1..cols {
            let mut s = 0.0;
            for i in 0..n {
                s += lam[i][c1] * lam[i][c2];
            }
            if c1 == c2 {
                s += 1e-10; // tiny Tikhonov guard against collinearity
            }
            ata[c1][c2] = s;
            ata[c2][c1] = s;
        }
        let mut sb = 0.0;
        for i in 0..n {
            sb += lam[i][c1] * e_tilde[i];
        }
        atb[c1] = sb;
    }

    let beta = solve_linear(&ata, &atb).unwrap_or_else(|| vec![0.0; cols]);

    // Step 10-11: w_i = eps_i / sigma_i, normalized so Σ |w_i| = 1
    let mut out = vec![0.0; n_total];
    let mut abs_sum = 0.0;
    for i in 0..n {
        let mut pred = 0.0;
        for c in 0..cols {
            pred += lam[i][c] * beta[c];
        }
        let eps = e_tilde[i] - pred;
        let w = eps / sigmas[i];
        if w.is_finite() {
            out[active_idx[i]] = w;
            abs_sum += w.abs();
        }
    }
    if abs_sum > 1e-12 {
        for w in &mut out {
            *w /= abs_sum;
        }
    }
    out
}

/// Builds a diverse pool of 36 strictly causal directional alphas (`-1.0` / `0.0` / `+1.0`)
/// across 5 orthogonal signal families (MA cross, TSMOM, Donchian, Mean-Reversion, EMA/Volume).
fn build_alpha_signals(bars: &Bars) -> Vec<Vec<f64>> {
    let n = bars.len();
    let close = &bars.close;
    let high = &bars.high;
    let low = &bars.low;
    let vol = &bars.volume;

    let mut alphas: Vec<Vec<f64>> = Vec::with_capacity(36);

    // Precompute shared SMAs/EMAs
    let sma5 = ind::sma(close, 5);
    let sma10 = ind::sma(close, 10);
    let sma15 = ind::sma(close, 15);
    let sma20 = ind::sma(close, 20);
    let sma30 = ind::sma(close, 30);
    let sma45 = ind::sma(close, 45);
    let sma50 = ind::sma(close, 50);
    let sma60 = ind::sma(close, 60);
    let sma120 = ind::sma(close, 120);
    let sma200 = ind::sma(close, 200);

    // Family 1: Moving Average Crossovers (8 alphas)
    for (fast, slow) in [
        (&sma5, &sma20),
        (&sma10, &sma30),
        (&sma10, &sma50),
        (&sma20, &sma60),
        (&sma20, &sma120),
        (&sma50, &sma200),
        (&sma5, &sma60),
        (&sma15, &sma45),
    ] {
        let mut s = vec![0.0; n];
        for i in 0..n {
            if let (Some(f), Some(sl)) = (fast[i], slow[i]) {
                s[i] = if f > sl { 1.0 } else { -1.0 };
            }
        }
        alphas.push(s);
    }

    // Family 2: Time-Series Momentum (8 alphas)
    for lb in [5usize, 10, 20, 40, 60, 90, 120, 200] {
        let mut s = vec![0.0; n];
        for i in lb..n {
            if close[i - lb] > 0.0 {
                s[i] = if close[i] > close[i - lb] { 1.0 } else { -1.0 };
            }
        }
        alphas.push(s);
    }

    // Family 3: Donchian Channel Midpoint Breakout (6 alphas)
    for lb in [10usize, 20, 30, 55, 80, 120] {
        let dc = ind::donchian(high, low, lb, lb);
        let mut s = vec![0.0; n];
        for i in 0..n {
            if let (Some(u), Some(l)) = (dc.upper[i], dc.lower[i]) {
                let mid = 0.5 * (u + l);
                s[i] = if close[i] >= mid { 1.0 } else { -1.0 };
            }
        }
        alphas.push(s);
    }

    // Family 4: Mean-Reversion / Oscillator (8 alphas)
    let rsi2 = ind::rsi(close, 2);
    let rsi5 = ind::rsi(close, 5);
    let rsi14 = ind::rsi(close, 14);
    for (rs, lo_th, hi_th, invert) in [
        (&rsi2, 25.0, 75.0, true),
        (&rsi5, 35.0, 65.0, true),
        (&rsi14, 40.0, 60.0, true),
        (&rsi14, 50.0, 50.0, false), // trend RSI
    ] {
        let mut s = vec![0.0; n];
        let mut state = 0.0;
        for i in 0..n {
            if let Some(v) = rs[i] {
                if invert {
                    if v < lo_th {
                        state = 1.0;
                    } else if v > hi_th {
                        state = -1.0;
                    }
                } else {
                    state = if v >= 50.0 { 1.0 } else { -1.0 };
                }
                s[i] = state;
            }
        }
        alphas.push(s);
    }
    for (bn, bk) in [(10usize, 1.5), (20, 2.0)] {
        let bb = ind::bollinger(close, bn, bk);
        let mut s = vec![0.0; n];
        let mut state = 0.0;
        for i in 0..n {
            if let (Some(l), Some(u), Some(m)) = (bb.lower[i], bb.upper[i], bb.mid[i]) {
                if close[i] < l {
                    state = 1.0;
                } else if close[i] > u {
                    state = -1.0;
                } else if (state > 0.0 && close[i] > m) || (state < 0.0 && close[i] < m) {
                    state = if close[i] >= m { 0.5 } else { -0.5 };
                }
                s[i] = state;
            }
        }
        alphas.push(s);
    }
    // IBS mean-reversion alpha
    {
        let mut s = vec![0.0; n];
        for i in 0..n {
            let rng = high[i] - low[i];
            if rng > 0.0 {
                let ibs = (close[i] - low[i]) / rng;
                s[i] = if ibs < 0.3 { 1.0 } else if ibs > 0.7 { -1.0 } else { 0.0 };
            }
        }
        alphas.push(s);
    }
    // 5-day short-term reversal alpha
    {
        let mut s = vec![0.0; n];
        for i in 5..n {
            if close[i - 5] > 0.0 {
                s[i] = if close[i] < close[i - 5] { 1.0 } else { -1.0 };
            }
        }
        alphas.push(s);
    }

    // Family 5: EMA & Volume-Confirmed Trend (6 alphas)
    let ema8 = ind::ema(close, 8);
    let ema12 = ind::ema(close, 12);
    let ema20 = ind::ema(close, 20);
    let ema21 = ind::ema(close, 21);
    let ema26 = ind::ema(close, 26);
    let ema50 = ind::ema(close, 50);
    let ema120 = ind::ema(close, 120);
    let v20 = ind::sma(vol, 20);

    for (f, sl) in [(&ema8, &ema21), (&ema12, &ema26), (&ema20, &ema50)] {
        let mut s = vec![0.0; n];
        for i in 0..n {
            if let (Some(a), Some(b)) = (f[i], sl[i]) {
                s[i] = if a > b { 1.0 } else { -1.0 };
            }
        }
        alphas.push(s);
    }
    for em in [&ema20, &ema50, &ema120] {
        let mut s = vec![0.0; n];
        for i in 0..n {
            if let (Some(e), Some(va)) = (em[i], v20[i]) {
                let mult = if va > 0.0 && vol[i] > va { 1.0 } else { 0.5 };
                s[i] = if close[i] > e { mult } else { -mult };
            }
        }
        alphas.push(s);
    }

    alphas
}

const ALPHA_COMBO_PARAMS: &[Param] = &[
    Param::int("lookback_m", "평가 기간 M (일)", 20.0, 10.0, 30.0),
    Param::int("recent_d", "최근 성과 기간 d (일)", 5.0, 2.0, 15.0),
    Param::float("entry_th", "매수 확신도 기준", 0.15, 0.02, 0.60, 0.05),
    Param::float("exit_th", "청산 기준", -0.05, -0.50, 0.30, 0.05),
];

pub struct AlphaComboEnsemble {
    p: ResolvedParams,
    scores: ind::Series,
}

impl Strategy for AlphaComboEnsemble {
    fn id(&self) -> &'static str {
        "alpha_combo"
    }
    fn params_json(&self) -> Value {
        params_to_json(ALPHA_COMBO_PARAMS, &self.p)
    }
    fn warmup(&self) -> usize {
        200 + self.p["lookback_m"] as usize + 3
    }
    fn prepare(&mut self, bars: &Bars) {
        let n = bars.len();
        let m = self.p["lookback_m"] as usize;
        let m1 = m + 1;
        let d = (self.p["recent_d"] as usize).min(m1);
        let alphas = build_alpha_signals(bars);
        let na = alphas.len();

        // Daily bar returns
        let mut bar_ret = vec![0.0; n];
        for t in 1..n {
            if bars.close[t - 1] > 0.0 {
                bar_ret[t] = bars.close[t] / bars.close[t - 1] - 1.0;
            }
        }
        // Causal daily return of alpha a on day t: signal[a][t-1] * bar_ret[t]
        let mut alpha_ret = vec![vec![0.0; n]; na];
        for a in 0..na {
            for t in 1..n {
                alpha_ret[a][t] = alphas[a][t - 1] * bar_ret[t];
            }
        }

        let mut scores = vec![None; n];
        let start_t = (200 + m1 + 1).min(n);
        let mut r_mat = vec![vec![0.0; m1]; na];

        for t in start_t..n {
            // Fill R[a][s] where s = 0 is most recent realized return (day t)
            for a in 0..na {
                for s in 0..m1 {
                    r_mat[a][s] = alpha_ret[a][t - s];
                }
            }
            let w = alpha_combo_weights(&r_mat, d);
            let mut mega = 0.0;
            for a in 0..na {
                mega += w[a] * alphas[a][t];
            }
            if mega.is_finite() {
                scores[t] = Some(mega);
            }
        }
        self.scores = scores;
    }
    fn on_bar(&self, i: usize, ctx: &Context) -> Vec<Order> {
        let Some(score) = self.scores[i] else {
            return vec![];
        };
        let entry_th = self.p["entry_th"];
        let exit_th = self.p["exit_th"];
        if !ctx.position.is_open() && score > entry_th {
            return vec![Order::market(
                "buy",
                format!("알파 콤보 매수 신호 (확신도 {score:+.2} > {entry_th:+.2})"),
            )];
        }
        if ctx.position.is_open() && score < exit_th {
            return vec![Order::market(
                "sell",
                format!("알파 콤보 청산 신호 (확신도 {score:+.2} < {exit_th:+.2})"),
            )];
        }
        vec![]
    }
    fn chart_indicators(&self) -> Vec<Value> {
        vec![json!({"id": "ma", "params": {"p1": 20, "p2": 60, "p3": 200, "p4": 0}})]
    }
}

pub static ALPHA_COMBO_SPEC: StrategySpec = StrategySpec {
    id: "alpha_combo",
    name: "[151전략] WorldQuant 알파 콤보 앙상블",
    category: "swing",
    timeframe: "1d",
    theory: "《151 Trading Strategies》 3.20절 (Kakushadze–Yu) · 36개 약한 신호에서 중복을 빼고 고유 실력만 합치기",
    summary: "내부 36개 세부 전략의 최근 성적을 매일 평가해 서로 겹치는 신호는 빼고 요즘 진짜 먹히는 전략에 자동으로 가중치를 실어 매매합니다",
    description: "WorldQuant에서 수백 개의 약한 알파를 하나로 결합할 때 쓰는 Kakushadze–Yu(2017) 절차를 국내·미국 현물 주식에 맞게 구현했습니다. 이평 교차·모멘텀·채널 돌파·평균회귀·거래량 추세 등 36개 세부 전략의 최근 M일 성적에서 공통 움직임을 회귀분석으로 제거한 뒤, 고유 실력(잔차)이 높은 알파에만 자동으로 비중을 배분합니다.",
    guide: || json!({
        "background": "《151 Trading Strategies》 3.20절(Kakushadze–Yu 2017)의 핵심 비밀은 '약한 신호 하나로는 수수료를 이기기 어렵지만, 서로 다른 수십 개 신호에서 중복을 빼고 합치면 강한 메가 신호가 된다'는 점입니다.",
        "setup": [
            "평가 기간 M(기본 20일): 최근 20거래일 동안 36개 하위 전략의 수익률 패턴을 비교합니다 (36 > M 조건 자동 충족).",
            "최근 성과 기간 d(기본 5일): 최근 5일간 특히 잘 맞고 있는 전략에 점수를 부여합니다.",
            "매수 확신도 기준(기본 0.15) · 청산 기준(기본 -0.05): 합산 메가 신호(-1~+1)가 기준을 넘을 때만 현물 매수, 꺾이면 현금으로 대피합니다."
        ],
        "execution": [
            "매일 장 마감 후 36개 내부 알파의 실현 수익률 행렬을 표준화하고 공통 요인을 회귀분석으로 제거합니다.",
            "고유 실력(잔차)이 큰 알파에 자동으로 높은 가중치를 부여해 최종 확신도 점수를 계산합니다.",
            "점수가 매수 기준을 넘으면 매수하고, 청산 기준 아래로 떨어지면 전량 현금화합니다."
        ],
        "caution": "최근 M일 성과에 적응하는 구조이므로 급격한 방향 전환 직후에는 잠시 엇박자가 날 수 있습니다. 200일 이상 충분한 주가 이력이 있는 종목에서 가장 안정적입니다."
    }),
    params: ALPHA_COMBO_PARAMS,
    build: |raw| {
        let p = resolve_params(ALPHA_COMBO_PARAMS, raw)?;
        if p["recent_d"] > p["lookback_m"] {
            return Err("최근 성과 기간(d)은 평가 기간(M) 이하여야 합니다".to_string());
        }
        if p["entry_th"] <= p["exit_th"] {
            return Err("매수 확신도 기준은 청산 기준보다 커야 합니다".to_string());
        }
        Ok(Box::new(AlphaComboEnsemble { p, scores: vec![] }))
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hp_filter_preserves_linear_trend_and_is_strictly_causal() {
        let w = 60;
        let lam = 1600.0;
        let weights = hp_endpoint_weights(w, lam);
        // Constant and linear series must reproduce exact endpoint value
        let linear: Vec<f64> = (0..w).map(|i| 100.0 + 1.5 * i as f64).collect();
        let est: f64 = weights.iter().zip(linear.iter()).map(|(a, b)| a * b).sum();
        assert!((est - linear[w - 1]).abs() < 1e-7, "linear trend mismatch: {est} vs {}", linear[w - 1]);

        // Causality check: changing future prices MUST NOT alter earlier HP trend points
        let mut p1: Vec<f64> = (0..120)
            .map(|i| 100.0 + 0.4 * i as f64 + 5.0 * (i as f64 * 0.3).sin())
            .collect();
        let mut p2 = p1.clone();
        for x in &mut p2[100..] {
            *x *= 0.4; // sudden crash after bar 100
        }
        let t1 = causal_hp_trend(&p1, 50, 10000.0);
        let t2 = causal_hp_trend(&p2, 50, 10000.0);
        for i in 0..100 {
            assert_eq!(t1[i], t2[i], "HP trend leaked future data at bar {i}");
        }
        p1.clear();
    }

    #[test]
    fn alpha_combo_weights_normalizes_and_rewards_orthogonal_edge() {
        // 60 alphas over 21 periods (M = 20, N = 60 > M - 1):
        // Alphas 0..50 are zero-drift random noise; alphas 50..60 have a small persistent positive drift (+0.006/day).
        // Because step 1 demeans R over time, persistent drift does not enter Lambda and survives regression in eps!
        let n = 60;
        let m1 = 21;
        let mut r = vec![vec![0.0; m1]; n];
        let mut seed: u64 = 0x517cc1b727220a95;
        let mut next_u = || -> f64 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = ((seed >> 33) as f64) / ((1u64 << 31) as f64); // [0, 1)
            (x - 0.5) * 0.02 // [-0.01, +0.01] zero-mean noise
        };
        for i in 0..n {
            for s in 0..m1 {
                let edge = if i >= 50 { 0.006 } else { 0.0 };
                r[i][s] = next_u() + edge;
            }
        }
        let w = alpha_combo_weights(&r, 10);
        assert_eq!(w.len(), n);
        let abs_sum: f64 = w.iter().map(|x| x.abs()).sum();
        assert!((abs_sum - 1.0).abs() < 1e-9, "abs weight sum should be 1.0, got {abs_sum}");
        let edge_avg: f64 = w[50..].iter().sum::<f64>() / 10.0;
        let noise_avg: f64 = w[..50].iter().sum::<f64>() / 50.0;
        assert!(
            edge_avg > noise_avg + 0.01,
            "edge alphas should receive distinctly higher avg weight: edge={edge_avg:.4} vs noise={noise_avg:.4}"
        );
    }

    #[test]
    fn knn_is_strictly_causal() {
        let mk_bars = |crash_after: bool| -> Bars {
            let n = 360;
            let mut close = Vec::with_capacity(n);
            let mut volume = Vec::with_capacity(n);
            for i in 0..n {
                let mut px = 100.0 + 0.2 * i as f64 + 8.0 * (i as f64 * 0.15).sin();
                if crash_after && i >= 330 {
                    px *= 0.3;
                }
                close.push(px);
                volume.push(1000.0 + 300.0 * (i as f64 * 0.2).cos());
            }
            Bars {
                time: (0..n).map(|i| 1_700_000_000 + i as i64 * 86400).collect(),
                open: close.clone(),
                high: close.iter().map(|c| c * 1.01).collect(),
                low: close.iter().map(|c| c * 0.99).collect(),
                close,
                volume,
            }
        };
        let b1 = mk_bars(false);
        let b2 = mk_bars(true);
        let p = resolve_params(KNN_PARAMS, &json!({})).unwrap();
        let mut k1 = KnnPattern { p: p.clone(), preds: vec![], k_used: 0 };
        let mut k2 = KnnPattern { p, preds: vec![], k_used: 0 };
        k1.prepare(&b1);
        k2.prepare(&b2);
        for i in 0..330 {
            assert_eq!(k1.preds[i], k2.preds[i], "kNN leaked future data at bar {i}");
        }
        assert!(k1.preds[325].is_some(), "kNN should produce predictions after warmup");
    }
}
