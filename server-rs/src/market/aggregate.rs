use chrono::{Datelike, NaiveDate, TimeZone, Utc};

use super::models::Candle;

fn merge(group: &[Candle], time: i64) -> Candle {
    let open = group[0].open;
    let close = group[group.len() - 1].close;
    let mut high = f64::NEG_INFINITY;
    let mut low = f64::INFINITY;
    let mut volume = 0.0;
    for c in group {
        if c.high > high {
            high = c.high;
        }
        if c.low < low {
            low = c.low;
        }
        volume += c.volume;
    }
    Candle {
        time,
        open,
        high,
        low,
        close,
        volume,
    }
}

fn bucket<F>(candles: &[Candle], key_fn: F) -> Vec<Candle>
where
    F: Fn(&Candle) -> i64,
{
    let mut out = Vec::new();
    let mut group: Vec<Candle> = Vec::new();
    let mut current: Option<i64> = None;

    for c in candles {
        let k = key_fn(c);
        if !group.is_empty() && current != Some(k) {
            let t = key_fn(&group[0]);
            out.push(merge(&group, t));
            group.clear();
        }
        current = Some(k);
        group.push(c.clone());
    }
    if !group.is_empty() {
        let t = key_fn(&group[0]);
        out.push(merge(&group, t));
    }
    out
}

pub fn resample_intraday(candles: &[Candle], step_sec: i64) -> Vec<Candle> {
    bucket(candles, |c| c.time.div_euclid(step_sec) * step_sec)
}

pub fn resample_weekly(daily: &[Candle]) -> Vec<Candle> {
    bucket(daily, |c| {
        let days = c.time.div_euclid(86400);
        let base = NaiveDate::from_ymd_opt(1970, 1, 1).unwrap();
        let d = base + chrono::Duration::days(days);
        let monday = d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64);
        Utc.from_utc_datetime(&monday.and_hms_opt(0, 0, 0).unwrap())
            .timestamp()
    })
}
