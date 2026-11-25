//! Bot-level circuit breakers, consecutive-loss guards, cooldowns, and daily loss limits (MT-4).

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::{json, Value};

use crate::autotrade::{bots, executor};
use crate::errors::{ApiError, ApiResult};
use crate::market::models::{Candle, Interval};
use crate::state::AppState;
use crate::users::repository::now_iso;

pub fn default_guard() -> Value {
    json!({
        "max_drawdown_pct": 0.0,
        "max_consecutive_losses": 0,
        "cooldown_days": 0,
        "daily_loss_limit_pct": 0.0,
    })
}

fn parse_num_or_zero(v: Option<&Value>) -> ApiResult<f64> {
    match v {
        None | Some(Value::Null) => Ok(0.0),
        Some(x) => x
            .as_f64()
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| ApiError::bad_request("invalid_guard", "서킷브레이커 설정값은 숫자여야 합니다")),
    }
}

pub fn validate_guard(raw: Option<&Value>) -> ApiResult<Value> {
    let Some(obj) = raw.and_then(|v| v.as_object()) else {
        return Ok(default_guard());
    };
    let mdd = parse_num_or_zero(obj.get("max_drawdown_pct"))?;
    let streak = parse_num_or_zero(obj.get("max_consecutive_losses"))? as i64;
    let cooldown = parse_num_or_zero(obj.get("cooldown_days"))? as i64;
    let daily = parse_num_or_zero(obj.get("daily_loss_limit_pct"))?;

    if !(0.0..=80.0).contains(&mdd) {
        return Err(ApiError::bad_request(
            "invalid_guard",
            "최대 허용 낙폭은 0 ~ 80% 범위여야 합니다",
        ));
    }
    if !(0..=20).contains(&streak) {
        return Err(ApiError::bad_request(
            "invalid_guard",
            "연속 손절 한도는 0 ~ 20회 범위여야 합니다",
        ));
    }
    if !(0..=60).contains(&cooldown) {
        return Err(ApiError::bad_request(
            "invalid_guard",
            "재진입 쿨다운 일수는 0 ~ 60일 범위여야 합니다",
        ));
    }
    if !(0.0..=50.0).contains(&daily) {
        return Err(ApiError::bad_request(
            "invalid_guard",
            "일일 손실 한도는 0 ~ 50% 범위여야 합니다",
        ));
    }
    Ok(json!({
        "max_drawdown_pct": (mdd * 10000.0).round() / 10000.0,
        "max_consecutive_losses": streak,
        "cooldown_days": cooldown,
        "daily_loss_limit_pct": (daily * 10000.0).round() / 10000.0,
    }))
}

pub struct GuardCfg {
    pub max_drawdown_pct: f64,
    pub max_consecutive_losses: usize,
    pub cooldown_days: i64,
    pub daily_loss_limit_pct: f64,
}

pub fn guard_of(bot: &Value) -> GuardCfg {
    let g = bot.get("guard").and_then(|v| v.as_object());
    GuardCfg {
        max_drawdown_pct: g
            .and_then(|m| m.get("max_drawdown_pct"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
        max_consecutive_losses: g
            .and_then(|m| m.get("max_consecutive_losses"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        cooldown_days: g
            .and_then(|m| m.get("cooldown_days"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        daily_loss_limit_pct: g
            .and_then(|m| m.get("daily_loss_limit_pct"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    }
}

pub fn acknowledge_on_enable(bot: &mut Value) {
    let capital = bot["capital"].as_f64().unwrap_or(0.0);
    for m in ["paper", "live"] {
        if let Some(st) = bot
            .get_mut("state")
            .and_then(|s| s.get_mut(m))
            .and_then(|s| s.as_object_mut())
        {
            if st.remove("tripped").is_some() {
                let eq_len = st
                    .get("equity")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                let baseline = st
                    .get("equity")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.last())
                    .and_then(|e| e.get("value"))
                    .and_then(|v| v.as_f64())
                    .unwrap_or(capital);
                let tr_len = st
                    .get("trades")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                st.insert("guard_reset_equity_idx".to_string(), json!(eq_len));
                st.insert("guard_reset_baseline".to_string(), json!(baseline));
                st.insert("guard_reset_trade_idx".to_string(), json!(tr_len));
            }
        }
    }
}

pub fn check_buy_allowed(
    bot: &Value,
    st: &Value,
    today: &str,
    current_equity: Option<f64>,
) -> Option<String> {
    if let Some(tripped) = st.get("tripped").filter(|v| !v.is_null()) {
        let reason = tripped.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        return Some(format!("서킷브레이커 보호 잠금 중 ({reason})"));
    }
    let g = guard_of(bot);
    if g.cooldown_days > 0 {
        if let Some(cd_until) = st.get("cooldown_until").and_then(|v| v.as_str()) {
            if !cd_until.is_empty() && today <= cd_until {
                return Some(format!("손절 후 쿨다운 휴식 중 (~{cd_until})"));
            }
        }
    }
    if g.daily_loss_limit_pct > 0.0 {
        let eqs = st
            .get("equity")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut prev_eq = eqs
            .iter()
            .rev()
            .find(|e| e.get("date").and_then(|d| d.as_str()).unwrap_or("") < today)
            .and_then(|e| e.get("value"))
            .and_then(|v| v.as_f64());
        if prev_eq.is_none() && !eqs.is_empty() {
            prev_eq = Some(bot["capital"].as_f64().unwrap_or(0.0));
        }
        let cur = current_equity.or_else(|| {
            eqs.last()
                .and_then(|e| e.get("value"))
                .and_then(|v| v.as_f64())
        });
        if let (Some(pe), Some(ce)) = (prev_eq, cur) {
            if pe > 0.0 {
                let drop_pct = (1.0 - ce / pe) * 100.0;
                if drop_pct >= g.daily_loss_limit_pct {
                    return Some(format!(
                        "일일 손실 한도 도달 (-{drop_pct:.1}% ≤ -{}%)",
                        fmt_g(g.daily_loss_limit_pct)
                    ));
                }
            }
        }
    }
    None
}

pub fn fmt_g(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

pub async fn trip_bot(state: &AppState, bot: &mut Value, today: &str, reason: &str) {
    bot["enabled"] = json!(false);
    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();
    let has_pending = bot["state"][&sk]["pending"]
        .as_array()
        .is_some_and(|a| !a.is_empty());
    bot["state"][&sk]["tripped"] = json!({
        "at": now_iso(),
        "date": today,
        "reason": reason,
    });
    if (mode == "live" || mode == "semi") && has_pending {
        let _ = executor::cancel_live_pending(state, bot).await;
    }
    bot["state"][&sk]["pending"] = json!([]);
    bots::log(
        state,
        bot,
        "protection",
        &format!("🛑 서킷브레이커 발동 — 봇 자동 일시정지: {reason}"),
        "warn",
        None,
    )
    .await;
}

pub async fn evaluate_after_step(
    state: &AppState,
    bot: &mut Value,
    today: &str,
) -> Option<String> {
    let g = guard_of(bot);
    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();

    // 1. Post-loss cooldown
    let trades = bot["state"][&sk]["trades"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let closed: Vec<&Value> = trades
        .iter()
        .filter(|t| {
            t.get("exit_date").is_some_and(|v| !v.is_null())
                && t.get("pnl").is_some_and(|v| !v.is_null())
        })
        .collect();
    if g.cooldown_days > 0 {
        if let Some(last_t) = closed.last() {
            let pnl = last_t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let exit_date = last_t.get("exit_date").and_then(|v| v.as_str()).unwrap_or("");
            let last_cd = bot["state"][&sk]
                .get("last_cooldown_exit")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if pnl < 0.0 && exit_date > last_cd {
                if let Ok(d) = NaiveDate::parse_from_str(&exit_date[..exit_date.len().min(10)], "%Y-%m-%d") {
                    let until = (d + Duration::days(g.cooldown_days)).to_string();
                    bot["state"][&sk]["cooldown_until"] = json!(until);
                    bot["state"][&sk]["last_cooldown_exit"] = json!(exit_date);
                }
            }
        }
    }

    // 2. Max drawdown circuit breaker
    let eqs = bot["state"][&sk]["equity"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if g.max_drawdown_pct > 0.0 && !eqs.is_empty() {
        let reset_idx = bot["state"][&sk]
            .get("guard_reset_equity_idx")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        if reset_idx < eqs.len() {
            let window = &eqs[reset_idx..];
            let capital = bot["capital"].as_f64().unwrap_or(0.0);
            let baseline = if reset_idx == 0 {
                capital
            } else {
                bot["state"][&sk]
                    .get("guard_reset_baseline")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(capital)
            };
            let mut peak = baseline;
            for e in window {
                let val = e.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if val > peak {
                    peak = val;
                }
            }
            let cur = window
                .last()
                .and_then(|e| e.get("value"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if peak > 0.0 {
                let dd_pct = (1.0 - cur / peak) * 100.0;
                if dd_pct >= g.max_drawdown_pct {
                    let reason = format!(
                        "최대 허용 낙폭 도달 (-{dd_pct:.1}% / 한도 -{}%)",
                        fmt_g(g.max_drawdown_pct)
                    );
                    trip_bot(state, bot, today, &reason).await;
                    return Some(reason);
                }
            }
        }
    }

    // 3. Consecutive losing trades circuit breaker
    if g.max_consecutive_losses > 0 && !closed.is_empty() {
        let reset_t_idx = bot["state"][&sk]
            .get("guard_reset_trade_idx")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        let recent_slice = if reset_t_idx <= trades.len() {
            &trades[reset_t_idx..]
        } else {
            &[][..]
        };
        let mut streak = 0usize;
        for t in recent_slice.iter().rev() {
            if t.get("exit_date").is_some_and(|v| !v.is_null())
                && t.get("pnl").is_some_and(|v| !v.is_null())
            {
                let pnl = t.get("pnl").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if pnl < 0.0 {
                    streak += 1;
                } else {
                    break;
                }
            }
        }
        if streak >= g.max_consecutive_losses {
            let reason = format!(
                "{streak}회 연속 손절 발생 (한도 {}회)",
                g.max_consecutive_losses
            );
            trip_bot(state, bot, today, &reason).await;
            return Some(reason);
        }
    }

    None
}

/// Stale Data Guard (*151 Trading Strategies* bot skeleton safety principle, MT-25):
/// rejects frozen/stale market feeds before any live or intraday order decision is made.
/// Skipped when `is_mock` is true so deterministic fixture tests never fail on wall clock.
pub fn check_candle_freshness(
    candles: &[Candle],
    interval: Interval,
    now: DateTime<Utc>,
    is_mock: bool,
) -> Result<(), String> {
    if is_mock {
        return Ok(());
    }
    let Some(last) = candles.last() else {
        return Err("시세 캔들이 비어 있어 주문을 보류했습니다".to_string());
    };
    if last.close <= 0.0 || !last.close.is_finite() {
        return Err("비정상 시세(0 이하/NaN)가 감지되어 주문을 보류했습니다".to_string());
    }
    let age_sec = now.timestamp() - last.time;
    let max_age_sec = match interval {
        Interval::D1 => 8 * 86400, // covers long holiday streaks (Chuseok / Lunar New Year)
        Interval::W1 => 21 * 86400,
        _ => (3 * interval.seconds()).max(900), // 3× candle width, minimum 15m feed tolerance
    };
    if age_sec > max_age_sec {
        let mins = age_sec / 60;
        return Err(format!(
            "시세 데이터 지연(Stale, {}분 경과 / 허용 {}분) 감지 — 주문을 안전하게 보류했습니다",
            mins,
            max_age_sec / 60
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_candle(ts: i64, close: f64) -> Candle {
        Candle { time: ts, open: close, high: close, low: close, close, volume: 100.0 }
    }

    #[test]
    fn stale_data_guard_blocks_frozen_intraday_and_daily_feeds() {
        let now = Utc::now();
        let fresh_5m = vec![mk_candle(now.timestamp() - 300, 70_000.0)];
        assert!(check_candle_freshness(&fresh_5m, Interval::M5, now, false).is_ok());

        let stale_5m = vec![mk_candle(now.timestamp() - 2400, 70_000.0)];
        let err = check_candle_freshness(&stale_5m, Interval::M5, now, false).unwrap_err();
        assert!(err.contains("Stale"), "expected stale msg, got: {err}");
        // Mock provider bypasses wall-clock check
        assert!(check_candle_freshness(&stale_5m, Interval::M5, now, true).is_ok());

        let stale_1d = vec![mk_candle(now.timestamp() - 10 * 86400, 70_000.0)];
        assert!(check_candle_freshness(&stale_1d, Interval::D1, now, false).is_err());
        let fresh_1d = vec![mk_candle(now.timestamp() - 2 * 86400, 70_000.0)];
        assert!(check_candle_freshness(&fresh_1d, Interval::D1, now, false).is_ok());
    }
}
