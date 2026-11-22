//! Technical indicators (line-for-line port of `server/app/quant/indicators.py` & `client/src/indicators/calc.ts`).

pub type Series = Vec<Option<f64>>;

pub fn sma_opt(values: &[Option<f64>], n: usize) -> Series {
    let mut out = vec![None; values.len()];
    if n == 0 {
        return out;
    }
    let mut total = 0.0;
    let mut valid = 0usize;
    for (i, v) in values.iter().enumerate() {
        let Some(x) = v else {
            total = 0.0;
            valid = 0;
            continue;
        };
        total += x;
        valid += 1;
        if valid > n {
            if let Some(old) = values[i - n] {
                total -= old;
            }
            valid = n;
        }
        if valid == n {
            out[i] = Some(total / n as f64);
        }
    }
    out
}

pub fn sma(values: &[f64], n: usize) -> Series {
    let opts: Vec<Option<f64>> = values.iter().copied().map(Some).collect();
    sma_opt(&opts, n)
}

fn smoothed_opt(values: &[Option<f64>], n: usize, alpha: f64) -> Series {
    let mut out = vec![None; values.len()];
    if n == 0 {
        return out;
    }
    let mut prev: Option<f64> = None;
    let mut seed_sum = 0.0;
    let mut seed_count = 0usize;
    for (i, v) in values.iter().enumerate() {
        let Some(x) = v else {
            if prev.is_some() {
                prev = None;
                seed_sum = 0.0;
                seed_count = 0;
            }
            continue;
        };
        if let Some(p) = prev {
            let next = alpha * x + (1.0 - alpha) * p;
            prev = Some(next);
            out[i] = Some(next);
        } else {
            seed_sum += x;
            seed_count += 1;
            if seed_count == n {
                let p = seed_sum / n as f64;
                prev = Some(p);
                out[i] = Some(p);
            }
        }
    }
    out
}

pub fn ema_opt(values: &[Option<f64>], n: usize) -> Series {
    smoothed_opt(values, n, 2.0 / (n as f64 + 1.0))
}

pub fn ema(values: &[f64], n: usize) -> Series {
    let opts: Vec<Option<f64>> = values.iter().copied().map(Some).collect();
    ema_opt(&opts, n)
}

pub fn rma_opt(values: &[Option<f64>], n: usize) -> Series {
    smoothed_opt(values, n, 1.0 / n as f64)
}

pub fn rma(values: &[f64], n: usize) -> Series {
    let opts: Vec<Option<f64>> = values.iter().copied().map(Some).collect();
    rma_opt(&opts, n)
}

pub fn stdev(values: &[f64], n: usize) -> Series {
    let mean = sma(values, n);
    let mut out = vec![None; values.len()];
    if n == 0 {
        return out;
    }
    for (i, m) in mean.iter().enumerate() {
        let Some(m) = m else {
            continue;
        };
        let mut ss = 0.0;
        for &v in &values[i + 1 - n..=i] {
            let d = v - m;
            ss += d * d;
        }
        out[i] = Some((ss / n as f64).sqrt());
    }
    out
}

pub fn highest(values: &[f64], n: usize) -> Series {
    let mut out = vec![None; values.len()];
    if n == 0 {
        return out;
    }
    for i in (n - 1)..values.len() {
        let m = values[i + 1 - n..=i]
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        out[i] = Some(m);
    }
    out
}

pub fn lowest(values: &[f64], n: usize) -> Series {
    let mut out = vec![None; values.len()];
    if n == 0 {
        return out;
    }
    for i in (n - 1)..values.len() {
        let m = values[i + 1 - n..=i]
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        out[i] = Some(m);
    }
    out
}

pub fn zip_with<F>(a: &[Option<f64>], b: &[Option<f64>], f: F) -> Series
where
    F: Fn(f64, f64) -> Option<f64>,
{
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| match (x, y) {
            (Some(u), Some(v)) => f(*u, *v),
            _ => None,
        })
        .collect()
}

pub fn true_range(high: &[f64], low: &[f64], close: &[f64]) -> Vec<f64> {
    let n = high.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        if i == 0 {
            out.push(high[i] - low[i]);
        } else {
            let hl = high[i] - low[i];
            let hc = (high[i] - close[i - 1]).abs();
            let lc = (low[i] - close[i - 1]).abs();
            out.push(hl.max(hc).max(lc));
        }
    }
    out
}

pub fn rsi(close: &[f64], n: usize) -> Series {
    let len = close.len();
    if len == 0 {
        return vec![];
    }
    let mut gains = Vec::with_capacity(len);
    let mut losses = Vec::with_capacity(len);
    gains.push(None);
    losses.push(None);
    for i in 1..len {
        let diff = close[i] - close[i - 1];
        gains.push(Some(diff.max(0.0)));
        losses.push(Some((-diff).max(0.0)));
    }
    let rg = rma_opt(&gains, n);
    let rl = rma_opt(&losses, n);
    zip_with(&rg, &rl, |g, l| {
        if l == 0.0 {
            Some(if g == 0.0 { 50.0 } else { 100.0 })
        } else {
            Some(100.0 - 100.0 / (1.0 + g / l))
        }
    })
}

pub fn atr(high: &[f64], low: &[f64], close: &[f64], n: usize) -> Series {
    rma(&true_range(high, low, close), n)
}

pub struct BollingerOut {
    pub upper: Series,
    pub mid: Series,
    pub lower: Series,
}

pub fn bollinger(close: &[f64], n: usize, k: f64) -> BollingerOut {
    let mid = sma(close, n);
    let sd = stdev(close, n);
    let upper = zip_with(&mid, &sd, |m, s| Some(m + k * s));
    let lower = zip_with(&mid, &sd, |m, s| Some(m - k * s));
    BollingerOut { upper, mid, lower }
}

pub struct DonchianOut {
    pub upper: Series,
    pub lower: Series,
}

pub fn donchian(high: &[f64], low: &[f64], upper_n: usize, lower_n: usize) -> DonchianOut {
    DonchianOut {
        upper: highest(high, upper_n),
        lower: lowest(low, lower_n),
    }
}

pub struct MacdOut {
    pub macd: Series,
    pub signal: Series,
    pub hist: Series,
}

pub fn macd(close: &[f64], fast: usize, slow: usize, signal: usize) -> MacdOut {
    let line = zip_with(&ema(close, fast), &ema(close, slow), |f, s| Some(f - s));
    let sig = ema_opt(&line, signal);
    let hist = zip_with(&line, &sig, |m, s| Some(m - s));
    MacdOut {
        macd: line,
        signal: sig,
        hist,
    }
}

pub fn session_buckets(times: &[i64]) -> Vec<i64> {
    let n = times.len();
    if n == 0 {
        return vec![];
    }
    if n == 1 {
        return vec![0];
    }
    let mut diffs: Vec<i64> = (1..n)
        .map(|i| times[i] - times[i - 1])
        .filter(|&d| d > 0)
        .collect();
    diffs.sort_unstable();
    let step = if diffs.is_empty() {
        86400
    } else {
        diffs[diffs.len() / 2]
    };
    if step >= 43200 {
        return (0..n).map(|i| (i / 10) as i64).collect();
    }
    let gap_thresh = (step * 4).max(3600);
    let max_bars = (((6.5 * 3600.0) / step as f64).round() as usize).max(12);
    let mut out = Vec::with_capacity(n);
    out.push(0);
    let mut sid = 0i64;
    let mut in_sess = 1usize;
    for i in 1..n {
        if times[i] - times[i - 1] > gap_thresh || in_sess >= max_bars {
            sid += 1;
            in_sess = 1;
        } else {
            in_sess += 1;
        }
        out.push(sid);
    }
    out
}

pub struct VwapOut {
    pub vwap: Series,
    pub sd: Series,
    pub session: Vec<i64>,
}

pub fn intraday_vwap(
    times: &[i64],
    high: &[f64],
    low: &[f64],
    close: &[f64],
    volume: &[f64],
) -> VwapOut {
    let sessions = session_buckets(times);
    let n = times.len();
    let mut vwap_out = Vec::with_capacity(n);
    let mut sd_out = Vec::with_capacity(n);
    let mut cum_pv = 0.0;
    let mut cum_p2v = 0.0;
    let mut cum_v = 0.0;
    let mut prev_s: Option<i64> = None;

    for i in 0..n {
        let s = sessions[i];
        if prev_s != Some(s) {
            cum_pv = 0.0;
            cum_p2v = 0.0;
            cum_v = 0.0;
            prev_s = Some(s);
        }
        let tp = (high[i] + low[i] + close[i]) / 3.0;
        let vol = volume[i].max(0.0);
        cum_pv += tp * vol;
        cum_p2v += tp * tp * vol;
        cum_v += vol;
        if cum_v > 0.0 {
            let mean = cum_pv / cum_v;
            let var = (cum_p2v / cum_v - mean * mean).max(0.0);
            vwap_out.push(Some(mean));
            sd_out.push(Some(var.sqrt()));
        } else {
            vwap_out.push(Some(tp));
            sd_out.push(Some(0.0));
        }
    }
    VwapOut {
        vwap: vwap_out,
        sd: sd_out,
        session: sessions,
    }
}
