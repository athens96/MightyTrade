use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::errors::{ApiError, ApiResult};

pub static KR_SYMBOL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9A-Z]{6}$").unwrap());
pub static US_SYMBOL_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[A-Z][A-Z0-9.\-]{0,11}$").unwrap());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Market {
    KR,
    US,
}

impl Market {
    pub fn parse(raw: &str) -> ApiResult<Self> {
        match raw.trim().to_uppercase().as_str() {
            "KR" => Ok(Market::KR),
            "US" => Ok(Market::US),
            _ => Err(ApiError::bad_request(
                "invalid_market",
                "market must be KR or US",
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Market::KR => "KR",
            Market::US => "US",
        }
    }

    pub fn validate_symbol(&self, symbol: &str) -> bool {
        match self {
            Market::KR => KR_SYMBOL_RE.is_match(symbol),
            Market::US => US_SYMBOL_RE.is_match(symbol),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interval {
    M1,
    M5,
    M15,
    H1,
    D1,
    W1,
}

impl Interval {
    pub fn parse(raw: &str) -> ApiResult<Self> {
        match raw.trim() {
            "1m" => Ok(Interval::M1),
            "5m" => Ok(Interval::M5),
            "15m" => Ok(Interval::M15),
            "1h" => Ok(Interval::H1),
            "1d" => Ok(Interval::D1),
            "1w" => Ok(Interval::W1),
            _ => Err(ApiError::bad_request(
                "invalid_interval",
                "one of ['1m', '5m', '15m', '1h', '1d', '1w']",
            )),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Interval::M1 => "1m",
            Interval::M5 => "5m",
            Interval::M15 => "15m",
            Interval::H1 => "1h",
            Interval::D1 => "1d",
            Interval::W1 => "1w",
        }
    }

    pub fn seconds(&self) -> i64 {
        match self {
            Interval::M1 => 60,
            Interval::M5 => 300,
            Interval::M15 => 900,
            Interval::H1 => 3600,
            Interval::D1 => 86400,
            Interval::W1 => 604800,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub market: String,
    pub symbol: String,
    pub price: f64,
    pub change: f64,
    pub change_pct: f64,
    pub currency: String,
    pub time: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instrument {
    pub market: String,
    pub symbol: String,
    pub name: String,
    pub exchange: String,
    pub english_name: String,
}
