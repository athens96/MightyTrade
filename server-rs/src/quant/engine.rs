use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::market::models::Candle;
use crate::quant::metrics::{drawdown_series, performance, trade_stats};
use crate::quant::strategies::Strategy;

#[derive(Debug, Clone)]
pub struct Bars {
    pub time: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
}

impl Bars {
    pub fn from_candles(candles: &[Candle]) -> Self {
        Self {
            time: candles.iter().map(|c| c.time).collect(),
            open: candles.iter().map(|c| c.open).collect(),
            high: candles.iter().map(|c| c.high).collect(),
            low: candles.iter().map(|c| c.low).collect(),
            close: candles.iter().map(|c| c.close).collect(),
            volume: candles.iter().map(|c| c.volume).collect(),
        }
    }

    pub fn to_candles(&self) -> Vec<Candle> {
        (0..self.len())
            .map(|i| Candle {
                time: self.time[i],
                open: self.open[i],
                high: self.high[i],
                low: self.low[i],
                close: self.close[i],
                volume: self.volume[i],
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.close.len()
    }

    pub fn is_empty(&self) -> bool {
        self.close.is_empty()
    }

    pub fn slice(&self, end: usize) -> Self {
        let e = end.min(self.len());
        Self {
            time: self.time[..e].to_vec(),
            open: self.open[..e].to_vec(),
            high: self.high[..e].to_vec(),
            low: self.low[..e].to_vec(),
            close: self.close[..e].to_vec(),
            volume: self.volume[..e].to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Order {
    pub side: String, // "buy" | "sell"
    pub kind: String, // "market" | "stop" | "limit"
    pub stop: Option<f64>,
    pub stop_from_open: Option<f64>,
    pub limit: Option<f64>,
    pub qty: Option<f64>,
    pub reason: String,
}

impl Order {
    pub fn market(side: &str, reason: impl Into<String>) -> Self {
        Self {
            side: side.to_string(),
            kind: "market".to_string(),
            stop: None,
            stop_from_open: None,
            limit: None,
            qty: None,
            reason: reason.into(),
        }
    }

    pub fn stop(side: &str, level: f64, qty: Option<f64>, reason: impl Into<String>) -> Self {
        Self {
            side: side.to_string(),
            kind: "stop".to_string(),
            stop: Some(level),
            stop_from_open: None,
            limit: None,
            qty,
            reason: reason.into(),
        }
    }

    pub fn limit(side: &str, level: f64, qty: Option<f64>, reason: impl Into<String>) -> Self {
        Self {
            side: side.to_string(),
            kind: "limit".to_string(),
            stop: None,
            stop_from_open: None,
            limit: Some(level),
            qty,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Position {
    pub qty: f64,
    pub entry_price: f64,
    pub entry_index: isize,
    pub entry_atr: Option<f64>,
}

impl Position {
    pub fn flat() -> Self {
        Self {
            qty: 0.0,
            entry_price: 0.0,
            entry_index: -1,
            entry_atr: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.qty > 0.0
    }
}

#[derive(Debug, Clone)]
pub struct Context {
    pub position: Position,
    pub cash: f64,
    pub equity: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Costs {
    pub commission_pct: f64,
    pub tax_pct: f64,
    pub slippage_pct: f64,
}

impl Default for Costs {
    fn default() -> Self {
        Self {
            commission_pct: 0.015,
            tax_pct: 0.0,
            slippage_pct: 0.05,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RiskRules {
    pub stop_loss_pct: f64,
    pub take_profit_pct: f64,
    pub trailing_stop_pct: f64,
}

impl RiskRules {
    pub fn enabled(&self) -> bool {
        self.stop_loss_pct > 0.0 || self.take_profit_pct > 0.0 || self.trailing_stop_pct > 0.0
    }

    pub fn from_value(raw: Option<&Value>) -> Result<Self, String> {
        let Some(obj) = raw.and_then(|v| v.as_object()) else {
            return Ok(Self::default());
        };
        let parse_k = |k: &str| -> Result<f64, String> {
            let v = match obj.get(k) {
                None | Some(Value::Null) => 0.0,
                Some(Value::String(s)) if s.is_empty() => 0.0,
                Some(val) => val
                    .as_f64()
                    .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
                    .ok_or_else(|| format!("{k} must be a number"))?,
            };
            if !(0.0..=80.0).contains(&v) {
                return Err(format!("{k} must be between 0 and 80"));
            }
            Ok((v * 10000.0).round() / 10000.0)
        };
        Ok(Self {
            stop_loss_pct: parse_k("stop_loss_pct")?,
            take_profit_pct: parse_k("take_profit_pct")?,
            trailing_stop_pct: parse_k("trailing_stop_pct")?,
        })
    }

    pub fn to_value(&self) -> Value {
        json!({
            "stop_loss_pct": self.stop_loss_pct,
            "take_profit_pct": self.take_profit_pct,
            "trailing_stop_pct": self.trailing_stop_pct,
        })
    }
}

#[derive(Debug, Clone)]
struct Trade {
    entry_time: i64,
    entry_price: f64,
    qty: f64,
    entry_reason: String,
    exit_time: Option<i64>,
    exit_price: Option<f64>,
    exit_reason: String,
    pnl: Option<f64>,
    return_pct: Option<f64>,
    bars: usize,
    open: bool,
    cost_basis: f64,
    entry_index: usize,
}

impl Trade {
    fn public(&self) -> Value {
        json!({
            "entry_time": self.entry_time,
            "entry_price": self.entry_price,
            "qty": self.qty,
            "entry_reason": self.entry_reason,
            "exit_time": self.exit_time,
            "exit_price": self.exit_price,
            "exit_reason": self.exit_reason,
            "pnl": self.pnl,
            "return_pct": self.return_pct,
            "bars": self.bars,
            "open": self.open,
        })
    }
}

pub fn buy_and_hold(
    bars: &Bars,
    start: usize,
    end: usize,
    capital: f64,
    costs: Costs,
    whole_shares: bool,
) -> Vec<f64> {
    let price = bars.close[start] * (1.0 + costs.slippage_pct / 100.0);
    let mut qty = capital / (price * (1.0 + costs.commission_pct / 100.0));
    if whole_shares {
        qty = qty.floor();
    }
    let cash = capital - qty * price * (1.0 + costs.commission_pct / 100.0);
    (start..end).map(|j| cash + qty * bars.close[j]).collect()
}

pub fn run_backtest(
    bars: &Bars,
    strategy: &mut dyn Strategy,
    capital: f64,
    costs: Costs,
    start_index: usize,
    whole_shares: bool,
    risk: RiskRules,
    regimes: Option<&[String]>,
    market_timing_mode: &str,
) -> Result<Value, String> {
    if bars.len() < 2 || start_index >= bars.len() - 1 {
        return Err("not enough bars for the backtest period".to_string());
    }
    strategy.prepare(bars);

    let mut cash = capital;
    let mut pos = Position::flat();
    let mut peak_price = 0.0f64;
    let mut trades: Vec<Trade> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut pending: Vec<Order> = Vec::new();
    let mut equity: Vec<f64> = Vec::with_capacity(bars.len() - start_index);
    let mut in_market = 0usize;
    let slip = costs.slippage_pct / 100.0;
    let comm = costs.commission_pct / 100.0;
    let tax_rate = costs.tax_pct / 100.0;

    let fill_price = |o: &Order, j: usize| -> Option<f64> {
        let raw = match o.kind.as_str() {
            "market" => bars.open[j],
            "stop" => {
                let level = o.stop.unwrap_or_else(|| bars.open[j] + o.stop_from_open.unwrap_or(0.0));
                if o.side == "buy" {
                    if bars.high[j] < level {
                        return None;
                    }
                    bars.open[j].max(level)
                } else {
                    if bars.low[j] > level {
                        return None;
                    }
                    bars.open[j].min(level)
                }
            }
            "limit" => {
                let lim = o.limit?;
                if o.side == "buy" {
                    if bars.low[j] > lim {
                        return None;
                    }
                    bars.open[j].min(lim)
                } else {
                    if bars.high[j] < lim {
                        return None;
                    }
                    bars.open[j].max(lim)
                }
            }
            _ => return None,
        };
        Some(if o.side == "buy" {
            raw * (1.0 + slip)
        } else {
            raw * (1.0 - slip)
        })
    };

    for j in start_index..bars.len() {
        let reg = regimes
            .and_then(|rs| rs.get(j))
            .map(|s| s.as_str())
            .unwrap_or("bull");
        let mt_scale: f64 = if (market_timing_mode == "block_buy" || market_timing_mode == "exit_all")
            && reg == "bear"
        {
            0.0
        } else if market_timing_mode == "half_size" {
            if reg == "bear" {
                0.5
            } else if reg == "caution" {
                0.75
            } else {
                1.0
            }
        } else {
            1.0
        };

        // Risk rules check
        let mut exited_by_risk = false;
        if pos.is_open() && risk.enabled() {
            let entry = pos.entry_price;
            let mut eff_stop: Option<f64> = None;
            let mut stop_reason = String::new();
            if risk.stop_loss_pct > 0.0 {
                eff_stop = Some(entry * (1.0 - risk.stop_loss_pct / 100.0));
                stop_reason = format!("공통 손절 (-{}%)", risk.stop_loss_pct);
            }
            if risk.trailing_stop_pct > 0.0 && peak_price > 0.0 {
                let tr_level = peak_price * (1.0 - risk.trailing_stop_pct / 100.0);
                if eff_stop.is_none_or(|e| tr_level > e) {
                    eff_stop = Some(tr_level);
                    stop_reason = format!("트레일링 스톱 (고점 대비 -{}%)", risk.trailing_stop_pct);
                }
            }
            if let Some(st_lv) = eff_stop {
                if bars.low[j] <= st_lv {
                    let o = Order::stop("sell", st_lv, None, stop_reason);
                    if let Some(px) = fill_price(&o, j) {
                        let val = pos.qty * px;
                        let fee = val * comm;
                        let tax = val * tax_rate;
                        cash += val - fee - tax;
                        if let Some(t) = trades.last_mut() {
                            t.exit_time = Some(bars.time[j]);
                            t.exit_price = Some(px);
                            t.exit_reason = o.reason;
                            t.open = false;
                            let pnl = val - fee - tax - t.cost_basis;
                            t.pnl = Some(pnl);
                            t.return_pct = Some(pnl / t.cost_basis * 100.0);
                            t.bars = j - t.entry_index;
                        }
                        pos = Position::flat();
                        peak_price = 0.0;
                        exited_by_risk = true;
                    }
                }
            }
            if !exited_by_risk && risk.take_profit_pct > 0.0 {
                let tp_level = entry * (1.0 + risk.take_profit_pct / 100.0);
                if bars.high[j] >= tp_level {
                    let o = Order::limit(
                        "sell",
                        tp_level,
                        None,
                        format!("공통 익절 (+{}%)", risk.take_profit_pct),
                    );
                    if let Some(px) = fill_price(&o, j) {
                        let val = pos.qty * px;
                        let fee = val * comm;
                        let tax = val * tax_rate;
                        cash += val - fee - tax;
                        if let Some(t) = trades.last_mut() {
                            t.exit_time = Some(bars.time[j]);
                            t.exit_price = Some(px);
                            t.exit_reason = o.reason;
                            t.open = false;
                            let pnl = val - fee - tax - t.cost_basis;
                            t.pnl = Some(pnl);
                            t.return_pct = Some(pnl / t.cost_basis * 100.0);
                            t.bars = j - t.entry_index;
                        }
                        pos = Position::flat();
                        peak_price = 0.0;
                        exited_by_risk = true;
                    }
                }
            }
        }

        if !exited_by_risk && market_timing_mode == "exit_all" && reg == "bear" && pos.is_open() {
            let o = Order::market("sell", "마켓 타이밍 하락장 현금 대피");
            if let Some(px) = fill_price(&o, j) {
                let val = pos.qty * px;
                let fee = val * comm;
                let tax = val * tax_rate;
                cash += val - fee - tax;
                if let Some(t) = trades.last_mut() {
                    t.exit_time = Some(bars.time[j]);
                    t.exit_price = Some(px);
                    t.exit_reason = o.reason;
                    t.open = false;
                    let pnl = val - fee - tax - t.cost_basis;
                    t.pnl = Some(pnl);
                    t.return_pct = Some(pnl / t.cost_basis * 100.0);
                    t.bars = j - t.entry_index;
                }
                pos = Position::flat();
                peak_price = 0.0;
                exited_by_risk = true;
            }
        }

        for o in pending.drain(..) {
            if o.side == "sell" {
                if exited_by_risk || !pos.is_open() {
                    continue;
                }
                if let Some(px) = fill_price(&o, j) {
                    let val = pos.qty * px;
                    let fee = val * comm;
                    let tax = val * tax_rate;
                    cash += val - fee - tax;
                    if let Some(t) = trades.last_mut() {
                        t.exit_time = Some(bars.time[j]);
                        t.exit_price = Some(px);
                        t.exit_reason = o.reason;
                        t.open = false;
                        let pnl = val - fee - tax - t.cost_basis;
                        t.pnl = Some(pnl);
                        t.return_pct = Some(pnl / t.cost_basis * 100.0);
                        t.bars = j - t.entry_index;
                    }
                    pos = Position::flat();
                    peak_price = 0.0;
                }
            } else {
                if pos.is_open() || mt_scale <= 0.0 {
                    continue;
                }
                if let Some(px) = fill_price(&o, j) {
                    let affordable = (cash * mt_scale.min(1.0)) / (px * (1.0 + comm));
                    let mut qty = o.qty.map(|q| (q * mt_scale).min(affordable)).unwrap_or(affordable);
                    if whole_shares {
                        qty = qty.floor();
                    }
                    if qty <= 0.0 {
                        if !warnings.iter().any(|w| w == "insufficient_capital") {
                            warnings.push("insufficient_capital".to_string());
                        }
                        continue;
                    }
                    let val = qty * px;
                    let fee = val * comm;
                    cash -= val + fee;
                    pos = Position {
                        qty,
                        entry_price: px,
                        entry_index: j as isize,
                        entry_atr: None,
                    };
                    peak_price = px.max(bars.high[j]);
                    trades.push(Trade {
                        entry_time: bars.time[j],
                        entry_price: px,
                        qty,
                        entry_reason: o.reason,
                        exit_time: None,
                        exit_price: None,
                        exit_reason: String::new(),
                        pnl: None,
                        return_pct: None,
                        bars: 0,
                        open: true,
                        cost_basis: val + fee,
                        entry_index: j,
                    });
                }
            }
        }

        if pos.is_open() {
            peak_price = peak_price.max(bars.high[j]);
            in_market += 1;
        }
        let eq = cash + pos.qty * bars.close[j];
        equity.push(eq);
        pending = strategy.on_bar(
            j,
            &Context {
                position: pos.clone(),
                cash,
                equity: eq,
            },
        );
    }

    if pos.is_open() {
        let last = bars.len() - 1;
        if let Some(t) = trades.last_mut() {
            let val = pos.qty * bars.close[last];
            let pnl = val - t.cost_basis;
            t.pnl = Some(pnl);
            t.return_pct = Some(pnl / t.cost_basis * 100.0);
            t.bars = last - t.entry_index;
        }
    }

    let times = &bars.time[start_index..];
    let bench = buy_and_hold(bars, start_index, bars.len(), capital, costs, whole_shares);
    let dd = drawdown_series(&equity);
    let trades_pub: Vec<Value> = trades.iter().map(|t| t.public()).collect();

    let mut metrics = performance(times, &equity);
    let tstats = trade_stats(&trades_pub);
    if let (Some(m), Some(ts)) = (metrics.as_object_mut(), tstats.as_object()) {
        for (k, v) in ts {
            m.insert(k.clone(), v.clone());
        }
        m.insert(
            "exposure_pct".to_string(),
            json!(in_market as f64 / equity.len() as f64 * 100.0),
        );
    }
    let r4 = |x: f64| (x * 10000.0).round() / 10000.0;
    let eq_pts: Vec<Value> = times
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            json!({
                "time": t,
                "equity": r4(equity[i]),
                "benchmark": r4(bench[i]),
                "drawdown": r4(dd[i] * 100.0),
            })
        })
        .collect();

    Ok(json!({
        "equity": eq_pts,
        "metrics": metrics,
        "benchmark": performance(times, &bench),
        "trades": trades_pub,
        "warnings": warnings,
    }))
}
