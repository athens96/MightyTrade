use chrono::Utc;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use sha2::{Digest, Sha256};

use super::models::{Candle, Instrument, Interval, Market, Quote};

fn sample_instruments() -> Vec<Instrument> {
    let rows: &[(&str, &str, &str, &str, &str)] = &[
        ("KR", "005930", "삼성전자", "KOSPI", "Samsung Electronics"),
        ("KR", "000660", "SK하이닉스", "KOSPI", "SK hynix"),
        ("KR", "035420", "NAVER", "KOSPI", "NAVER"),
        ("KR", "069500", "KODEX 200", "KOSPI", "KODEX 200 ETF"),
        ("KR", "229200", "KODEX 코스닥150", "KOSPI", "KODEX KOSDAQ150 ETF"),
        ("KR", "360750", "TIGER 미국S&P500", "KOSPI", "TIGER US S&P500 ETF"),
        ("KR", "148070", "KOSEF 국고채10년", "KOSPI", "KOSEF KTB 10Y ETF"),
        ("KR", "114260", "KODEX 국고채3년", "KOSPI", "KODEX KTB 3Y ETF"),
        ("KR", "153130", "KODEX 단기채권", "KOSPI", "KODEX Short-Term Bond ETF"),
        ("KR", "132030", "KODEX 골드선물(H)", "KOSPI", "KODEX Gold Futures ETF"),
        ("KR", "261220", "KODEX WTI원유선물(H)", "KOSPI", "KODEX WTI Crude Oil ETF"),
        ("US", "AAPL", "애플", "NASDAQ", "Apple Inc."),
        ("US", "NVDA", "엔비디아", "NASDAQ", "NVIDIA Corp."),
        ("US", "TSLA", "테슬라", "NASDAQ", "Tesla Inc."),
        ("US", "SPY", "SPDR S&P 500 ETF", "NYSE", "SPDR S&P 500 ETF Trust"),
        ("US", "QQQ", "Invesco QQQ", "NASDAQ", "Invesco QQQ Trust"),
        ("US", "TLT", "iShares 미국 장기국채 20년+", "NASDAQ", "iShares 20+ Year Treasury Bond ETF"),
        ("US", "IEF", "iShares 미국 중기국채 7-10년", "NASDAQ", "iShares 7-10 Year Treasury Bond ETF"),
        ("US", "SHY", "iShares 미국 단기국채 1-3년", "NASDAQ", "iShares 1-3 Year Treasury Bond ETF"),
        ("US", "AGG", "iShares 미국 종합채권", "NYSE", "iShares Core U.S. Aggregate Bond ETF"),
        ("US", "LQD", "iShares 투자등급 회사채", "NYSE", "iShares iBoxx Investment Grade Corporate Bond ETF"),
        ("US", "GLD", "SPDR 골드 트러스트", "NYSE", "SPDR Gold Shares"),
        ("US", "DBC", "Invesco 원자재 지수", "NYSE", "Invesco DB Commodity Index Tracking Fund"),
        ("US", "VEA", "Vanguard 선진국 주식", "NYSE", "Vanguard FTSE Developed Markets ETF"),
        ("US", "VWO", "Vanguard 신흥국 주식", "NYSE", "Vanguard FTSE Emerging Markets ETF"),
    ];
    rows.iter()
        .map(|(m, s, n, e, en)| Instrument {
            market: (*m).to_string(),
            symbol: (*s).to_string(),
            name: (*n).to_string(),
            exchange: (*e).to_string(),
            english_name: (*en).to_string(),
        })
        .collect()
}

fn seeded_rng(market: Market, symbol: &str, interval: Interval) -> ChaCha8Rng {
    let key = format!("{}:{}:{}", market.as_str(), symbol, interval.as_str());
    let hash = Sha256::digest(key.as_bytes());
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);
    ChaCha8Rng::from_seed(seed)
}

fn gauss(rng: &mut ChaCha8Rng, sigma: f64) -> f64 {
    let u1: f64 = rng.gen_range(1e-12..1.0);
    let u2: f64 = rng.gen_range(0.0..1.0);
    let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
    z * sigma
}

pub fn get_candles(market: Market, symbol: &str, interval: Interval, limit: usize) -> Vec<Candle> {
    let mut rng = seeded_rng(market, symbol, interval);
    let step = interval.seconds();
    let end = Utc::now().timestamp().div_euclid(step) * step;
    let mut price = if market == Market::KR { 70_000.0 } else { 150.0 };
    let mut out = Vec::with_capacity(limit);

    for i in 0..limit {
        let t = end - ((limit - 1 - i) as i64) * step;
        let o = price;
        let c = (o * (1.0 + gauss(&mut rng, 0.015))).max(0.01);
        let h = o.max(c) * (1.0 + gauss(&mut rng, 0.006).abs());
        let l = o.min(c) * (1.0 - gauss(&mut rng, 0.006).abs());
        let v = rng.gen_range(10_000..=1_000_000) as f64;
        let (o, h, l, c) = if market == Market::KR {
            (o.round(), h.round(), l.round(), c.round())
        } else {
            (
                (o * 100.0).round() / 100.0,
                (h * 100.0).round() / 100.0,
                (l * 100.0).round() / 100.0,
                (c * 100.0).round() / 100.0,
            )
        };
        out.push(Candle {
            time: t,
            open: o,
            high: h,
            low: l,
            close: c,
            volume: v,
        });
        price = c;
    }
    out
}

pub fn get_history(market: Market, symbol: &str, interval: Interval, start_ts: i64) -> Vec<Candle> {
    let step = interval.seconds();
    let now = Utc::now().timestamp();
    let span = if start_ts > 0 && now > start_ts {
        ((now - start_ts) / step) as usize + 50
    } else {
        1000
    };
    let limit = span.clamp(250, 2000);
    get_candles(market, symbol, interval, limit)
}

pub fn get_quote(market: Market, symbol: &str) -> Quote {
    let candles = get_candles(market, symbol, Interval::D1, 2);
    let prev = &candles[0];
    let last = &candles[1];
    let change = last.close - prev.close;
    let change_pct = if prev.close != 0.0 {
        (change / prev.close * 100.0 * 100.0).round() / 100.0
    } else {
        0.0
    };
    Quote {
        market: market.as_str().to_string(),
        symbol: symbol.to_string(),
        price: last.close,
        change: (change * 100.0).round() / 100.0,
        change_pct,
        currency: if market == Market::KR { "KRW" } else { "USD" }.to_string(),
        time: last.time,
    }
}

pub fn search(market: Market, query: &str) -> Vec<Instrument> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let m = market.as_str();
    sample_instruments()
        .into_iter()
        .filter(|i| {
            i.market == m
                && (i.symbol.to_lowercase().contains(&q)
                    || i.name.to_lowercase().contains(&q)
                    || i.english_name.to_lowercase().contains(&q))
        })
        .collect()
}
