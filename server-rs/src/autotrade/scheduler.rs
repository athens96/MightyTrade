//! Background scheduler for bots. Exactly one per server: guarded by an exclusive file lock.

use chrono::{DateTime, Duration, Utc};
use fs2::FileExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs::OpenOptions;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{interval, Duration as TokioDuration};

use crate::autotrade::{bots, executor, notifier, scalp, semi};
use crate::errors::ApiResult;
use crate::market::extras;
use crate::state::AppState;
use crate::users::{credentials, repository as users};

const TICK_SEC: u64 = 30;
const OPEN_DELAY_MINS: i64 = 1;
const LATE_CUTOFF_HOURS: i64 = 1;
const RECONCILE_EVERY_MINS: i64 = 2;
const ERROR_BACKOFF_MINS: i64 = 10;
const BRIEFING_DELAY_MINS: i64 = 2;
const BRIEFING_WINDOW_MINS: i64 = 40;

pub struct SchedulerHandle {
    running: AtomicBool,
    last_tick: Mutex<Option<String>>,
    last_error: Mutex<Option<String>>,
    _lock_file: Mutex<Option<std::fs::File>>,
}

impl Default for SchedulerHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerHandle {
    pub fn new() -> Self {
        Self {
            running: AtomicBool::new(false),
            last_tick: Mutex::new(None),
            last_error: Mutex::new(None),
            _lock_file: Mutex::new(None),
        }
    }

    pub fn status_json(&self) -> Value {
        json!({
            "running": self.running.load(Ordering::SeqCst),
            "last_tick": self.last_tick.lock().clone(),
            "error": self.last_error.lock().clone(),
        })
    }
}

fn parse_dt(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

pub async fn sessions(
    state: &AppState,
    user_id: &str,
    market: &str,
) -> ApiResult<Vec<(String, DateTime<Utc>, DateTime<Utc>)>> {
    let cal = extras::calendar(state, user_id, market).await?;
    let mut out = Vec::new();
    for key in ["previousBusinessDay", "today", "nextBusinessDay"] {
        let Some(day) = cal.get(key).and_then(|v| v.as_object()) else {
            continue;
        };
        let block = if market == "KR" {
            day.get("integrated").and_then(|v| v.as_object())
        } else {
            Some(day)
        };
        let Some(reg) = block
            .and_then(|b| b.get("regularMarket"))
            .and_then(|v| v.as_object())
        else {
            continue;
        };
        let date_s = day
            .get("date")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let st = reg.get("startTime").and_then(|v| v.as_str()).and_then(parse_dt);
        let en = reg.get("endTime").and_then(|v| v.as_str()).and_then(parse_dt);
        if let (Some(s), Some(e)) = (st, en) {
            out.push((date_s, s, e));
        }
    }
    Ok(out)
}

pub async fn active_session(
    state: &AppState,
    user_id: &str,
    market: &str,
    now: DateTime<Utc>,
) -> ApiResult<Option<(String, DateTime<Utc>, DateTime<Utc>)>> {
    let all = sessions(state, user_id, market).await?;
    Ok(all.into_iter().find(|(_, s, e)| *s <= now && now < *e))
}

pub async fn target_date(
    state: &AppState,
    user_id: &str,
    market: &str,
    now: DateTime<Utc>,
) -> ApiResult<String> {
    let all = sessions(state, user_id, market).await?;
    for (d, _, e) in all {
        if now < e && !d.is_empty() {
            return Ok(d);
        }
    }
    let cal = extras::calendar(state, user_id, market).await?;
    Ok(cal
        .get("nextBusinessDay")
        .and_then(|v| v.get("date"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| now.date_naive().to_string()))
}

fn recent(iso: Option<&str>, within: Duration, now: DateTime<Utc>) -> bool {
    let Some(s) = iso else { return false };
    parse_dt(s).is_some_and(|dt| now - dt < within)
}

pub async fn tick(state: &AppState, now_opt: Option<DateTime<Utc>>) {
    let now = now_opt.unwrap_or_else(Utc::now);
    let enabled = bots::all_enabled(state);
    let mut by_user: HashMap<String, Vec<Value>> = HashMap::new();
    for b in enabled {
        let uid = b["user_id"].as_str().unwrap_or("").to_string();
        by_user.entry(uid).or_default().push(b);
    }

    for (uid, user_bots) in by_user {
        let Some(user) = users::get_user(state, &uid) else {
            continue;
        };
        if users::status_of(&user) != "approved"
            || user.get("autotrade_halted").and_then(|v| v.as_bool()).unwrap_or(false)
            || !credentials::has_active_broker(state, &uid)
        {
            continue;
        }
        let markets: BTreeSet<String> = user_bots
            .iter()
            .filter_map(|b| b.get("market").and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect();
        for market in markets {
            let all_sess = match sessions(state, &uid, &market).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "calendar failed for {}/{}: {}",
                        &uid[..uid.len().min(8)],
                        market,
                        e.message
                    );
                    continue;
                }
            };
            if let Some((date, start, end)) = all_sess
                .iter()
                .find(|(_, s, e)| *s <= now && now < *e)
                .cloned()
            {
                for mut bot in user_bots
                    .iter()
                    .filter(|b| b["market"].as_str() == Some(market.as_str()))
                    .cloned()
                {
                    step(state, &mut bot, &date, start, end, now).await;
                }
            } else if let Some((closed_date, _, _)) = all_sess.iter().find(|(_, _, e)| {
                now >= *e + Duration::minutes(BRIEFING_DELAY_MINS)
                    && now < *e + Duration::minutes(BRIEFING_WINDOW_MINS)
            }) {
                let _ = notifier::maybe_send_daily_briefing(state, &uid, &market, closed_date).await;
            }
        }
    }
}

async fn step(
    state: &AppState,
    bot: &mut Value,
    date: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) {
    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();
    let res: ApiResult<()> = async {
        let has_pending = bot["state"][&sk]["pending"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        let last_rec = bot["state"][&sk]
            .get("last_reconcile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        if (mode == "live" || mode == "semi")
            && has_pending
            && !recent(last_rec.as_deref(), Duration::minutes(RECONCILE_EVERY_MINS), now)
        {
            let _ = executor::reconcile_live(state, bot, date).await;
            bots::save(state, bot);
        }
        if bots::bot_kind(bot) == "webhook" {
            return Ok(());
        }
        let last_err_at = bot["state"][&sk]
            .get("last_error")
            .and_then(|e| e.get("at"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let open_delay = Duration::minutes(OPEN_DELAY_MINS);
        let err_backoff = Duration::minutes(ERROR_BACKOFF_MINS);
        let late_cutoff = Duration::hours(LATE_CUTOFF_HOURS);

        if mode == "semi" {
            if now < start + open_delay || recent(last_err_at.as_deref(), err_backoff, now) {
                return Ok(());
            }
            if executor::is_closing_bet(bot) {
                let holding = bots::has_open_position(bot, Some("live"));
                let in_close = now >= end - Duration::minutes(25) && now < end - Duration::minutes(2);
                let checked = bot["state"][&sk]
                    .get("closing_checked_date")
                    .and_then(|v| v.as_str())
                    == Some(date);
                if (!holding && (!in_close || checked))
                    || (holding && bot["state"][&sk]["last_run_date"].as_str() == Some(date))
                {
                    return Ok(());
                }
                let _ = semi::stage_from_scheduler(state, bot, date, end, Some(now)).await?;
                if in_close && !holding {
                    bot["state"][&sk]["closing_checked_date"] = json!(date);
                    bots::save(state, bot);
                }
                if let Some(m) = bot["state"][&sk].as_object_mut() {
                    m.remove("last_error");
                }
                return Ok(());
            }
            if !executor::is_intraday(bot) && bot["state"][&sk]["last_run_date"].as_str() == Some(date) {
                return Ok(());
            }
            if !executor::is_intraday(bot) && now > end - late_cutoff {
                bot["state"][&sk]["last_run_date"] = json!(date);
                bots::save(state, bot);
                bots::log(
                    state,
                    bot,
                    "skip",
                    "장 마감 1시간 전 이후라 오늘 반자동 점검을 건너뜁니다",
                    "warn",
                    None,
                )
                .await;
                return Ok(());
            }
            let _ = semi::stage_from_scheduler(state, bot, date, end, Some(now)).await?;
            if let Some(m) = bot["state"][&sk].as_object_mut() {
                m.remove("last_error");
            }
            return Ok(());
        }

        if executor::is_scalp(bot) {
            if now < start + open_delay || recent(last_err_at.as_deref(), err_backoff, now) {
                return Ok(());
            }
            let _ = scalp::run_intraday(state, bot, date, now, start, end).await?;
            if let Some(m) = bot["state"][&sk].as_object_mut() {
                m.remove("last_error");
            }
            return Ok(());
        }

        if executor::is_closing_bet(bot) {
            if now < start + open_delay || recent(last_err_at.as_deref(), err_backoff, now) {
                return Ok(());
            }
            let _ = executor::run_closing_bet(state, bot, date, now, start, end, false).await?;
            if let Some(m) = bot["state"][&sk].as_object_mut() {
                m.remove("last_error");
            }
            return Ok(());
        }

        if executor::is_intraday(bot) {
            if now < start + open_delay || recent(last_err_at.as_deref(), err_backoff, now) {
                return Ok(());
            }
            let _ = executor::run_intraday(state, bot, date, now, end).await?;
            if let Some(m) = bot["state"][&sk].as_object_mut() {
                m.remove("last_error");
            }
            return Ok(());
        }

        if bot["state"][&sk]["last_run_date"].as_str() == Some(date) || now < start + open_delay {
            return Ok(());
        }
        if recent(last_err_at.as_deref(), err_backoff, now) {
            return Ok(());
        }
        if now > end - late_cutoff {
            bot["state"][&sk]["last_run_date"] = json!(date);
            bots::save(state, bot);
            bots::log(
                state,
                bot,
                "skip",
                "장 마감 1시간 전 이후라 오늘 실행을 건너뜁니다",
                "warn",
                None,
            )
            .await;
            return Ok(());
        }
        let _ = executor::run(state, bot, date).await?;
        if let Some(m) = bot["state"][&sk].as_object_mut() {
            m.remove("last_error");
        }
        Ok(())
    }
    .await;

    if let Err(e) = res {
        bot["state"][&sk]["last_error"] = json!({
            "at": users::now_iso(),
            "message": e.message,
        });
        bots::save(state, bot);
        bots::log(
            state,
            bot,
            "error",
            &format!("실행 실패: {} (10분 뒤 재시도)", e.message),
            "error",
            Some(json!({ "code": e.code })),
        )
        .await;
    }
}

pub fn maybe_start(state: AppState) -> bool {
    if !state.config.autotrade_scheduler {
        return false;
    }
    let _ = std::fs::create_dir_all(&state.config.data_dir);
    let lock_path = state.config.data_dir.join(".autotrade.lock");
    let Ok(file) = OpenOptions::new().create(true).append(true).open(&lock_path) else {
        tracing::warn!("could not open autotrade lock file {:?}", lock_path);
        return false;
    };
    if file.try_lock_exclusive().is_err() {
        tracing::info!("autotrade scheduler already locked by another process");
        return false;
    }
    *state.scheduler._lock_file.lock() = Some(file);
    state.scheduler.running.store(true, Ordering::SeqCst);
    let st = Arc::new(state);
    tokio::spawn(async move {
        tracing::info!("autotrade scheduler started (pid {})", std::process::id());
        let mut ticker = interval(TokioDuration::from_secs(TICK_SEC));
        loop {
            ticker.tick().await;
            tick(&st, None).await;
            *st.scheduler.last_error.lock() = None;
            *st.scheduler.last_tick.lock() = Some(users::now_iso());
        }
    });
    true
}
