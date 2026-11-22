//! KRX / US exchange price tick grid rounding and Toss decimal formatting.

pub fn krx_tick(price: f64) -> f64 {
    if price < 2_000.0 {
        1.0
    } else if price < 5_000.0 {
        5.0
    } else if price < 20_000.0 {
        10.0
    } else if price < 50_000.0 {
        50.0
    } else if price < 200_000.0 {
        100.0
    } else if price < 500_000.0 {
        500.0
    } else {
        1_000.0
    }
}

pub fn round_price(market: &str, price: f64, mode: &str) -> f64 {
    if market == "KR" {
        let tick = krx_tick(price);
        let q = price / tick;
        let n = match mode {
            "down" => (q + 1e-9).floor(),
            "up" => (q - 1e-9).ceil(),
            _ => (q + 0.5).floor(),
        };
        (n * tick).round()
    } else {
        let f: f64 = if price >= 1.0 { 100.0 } else { 10_000.0 };
        let n = match mode {
            "down" => (price * f + 1e-9).floor(),
            "up" => (price * f - 1e-9).ceil(),
            _ => (price * f + 0.5 + 1e-9).floor(),
        };
        n / f
    }
}

pub fn fmt_price(market: &str, price: f64) -> String {
    if market == "KR" {
        format!("{}", price.round() as i64)
    } else {
        let s = format!("{price:.4}");
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_grid_matches_python() {
        assert_eq!(round_price("KR", 71_230.0, "nearest"), 71_200.0);
        assert_eq!(round_price("KR", 71_230.0, "up"), 71_300.0);
        assert_eq!(round_price("KR", 71_230.0, "down"), 71_200.0);
        assert_eq!(round_price("US", 12.345, "nearest"), 12.35);
        assert_eq!(fmt_price("KR", 71_200.0), "71200");
        assert_eq!(fmt_price("US", 12.3000), "12.3");
    }
}
