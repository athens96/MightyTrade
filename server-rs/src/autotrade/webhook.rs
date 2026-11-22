//! Inbound TradingView / external signal webhook trading bot (MT-10).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;
use uuid::Uuid;

use crate::account::toss_account::account_client;
use crate::autotrade::{bots, executor, protections, scheduler};
use crate::errors::{ApiError, ApiResult};
use crate::market::{self, extras::normalize, models::{Interval, Market}};
use crate::quant::data::cost_defaults;
use crate::state::AppState;
use crate::trading::orders as trading;
use crate::trading::ticks::{fmt_price, round_price};
use crate::users::{credentials, repository as users};

const RATE_WINDOW_SECS: f64 = 60.0;
const RATE_MAX_CALLS: usize = 15;
const LIVE_BUY_HEADROOM: f64 = 0.005;

static RATE_BUCKETS: Lazy<DashMap<(String, String), Vec<Instant>>> = Lazy::new(DashMap::new);

pub fn reset_rate_limits() {
    RATE_BUCKETS.clear();
}

fn check_rate_limit(bot_id: &str, remote_ip: &str) -> ApiResult<()> {
    let now = Instant::now();
    let key = (
        bot_id.to_string(),
        if remote_ip.is_empty() {
            "unknown".to_string()
        } else {
            remote_ip.to_string()
        },
    );
    let mut entry = RATE_BUCKETS.entry(key).or_default();
    entry.retain(|t| now.duration_since(*t).as_secs_f64() < RATE_WINDOW_SECS);
    if entry.len() >= RATE_MAX_CALLS {
        return Err(ApiError::too_many_requests(
            "webhook_rate_limited",
            "웹훅 요청이 너무 많습니다 (분당 최대 15회)",
        ));
    }
    entry.push(now);
    Ok(())
}

pub fn generate_secret_fields(state: &AppState) -> (String, String, String) {
    let mut buf = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut buf);
    let raw = URL_SAFE_NO_PAD.encode(buf);
    let enc = state.crypto.encrypt(&raw);
    let chars: Vec<char> = raw.chars().collect();
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    let hint = format!("{head}…{tail}");
    (raw, enc, hint)
}

pub async fn get_or_rotate_secret(
    state: &AppState,
    bot: &mut Value,
    rotate: bool,
) -> ApiResult<String> {
    if bots::bot_kind(bot) != "webhook" {
        return Err(ApiError::bad_request(
            "not_webhook_bot",
            "웹훅 봇에서만 비밀 토큰을 관리할 수 있습니다",
        ));
    }
    if !rotate {
        if let Some(enc) = bot.get("webhook_secret_enc").and_then(|v| v.as_str()) {
            if let Ok(plain) = state.crypto.decrypt(enc) {
                return Ok(plain);
            }
        }
    }
    let (raw, enc, hint) = generate_secret_fields(state);
    bot["webhook_secret_enc"] = json!(enc);
    bot["webhook_secret_hint"] = json!(hint);
    bots::log(
        state,
        bot,
        "config",
        if rotate {
            "웹훅 비밀 토큰이 새로 발급되었습니다"
        } else {
            "웹훅 비밀 토큰을 확인했습니다"
        },
        "info",
        None,
    )
    .await;
    bots::save(state, bot);
    Ok(raw)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn verify_secret(state: &AppState, bot: &Value, provided: &str) -> ApiResult<()> {
    let enc = bot
        .get("webhook_secret_enc")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::forbidden("invalid_webhook_secret", "웹훅 비밀 토큰이 올바르지 않습니다")
        })?;
    if provided.is_empty() {
        return Err(ApiError::forbidden(
            "invalid_webhook_secret",
            "웹훅 비밀 토큰이 올바르지 않습니다",
        ));
    }
    let expected = state.crypto.decrypt(enc).map_err(|_| {
        ApiError::forbidden("invalid_webhook_secret", "웹훅 비밀 토큰 검증에 실패했습니다")
    })?;
    if !ct_eq(expected.as_bytes(), provided.trim().as_bytes()) {
        return Err(ApiError::forbidden(
            "invalid_webhook_secret",
            "웹훅 비밀 토큰이 일치하지 않습니다",
        ));
    }
    Ok(())
}

fn cid(bot_id: &str, sig_id: &str) -> String {
    crate::autotrade::sha256_hex32(&format!("{bot_id}:wh:{sig_id}"))
}

async fn resolve_reference_price(
    state: &AppState,
    bot: &Value,
    payload_price: Option<f64>,
    use_toss: bool,
) -> ApiResult<f64> {
    let uid = bot["user_id"].as_str().unwrap_or("");
    let sym = bot["symbol"].as_str().unwrap_or("");
    if use_toss {
        if let Ok((client, _)) = account_client(state, uid) {
            let mut pp = HashMap::new();
            pp.insert("symbols".to_string(), sym.to_string());
            if let Ok(Some(Value::Array(rows))) =
                client.request("GET", "/api/v1/prices", Some(&pp), None, None).await
            {
                if let Some(lp) = rows
                    .first()
                    .and_then(|r| r.get("lastPrice"))
                    .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
                {
                    if lp > 0.0 {
                        return Ok(lp);
                    }
                }
            }
        }
    }
    if let Some(pp) = payload_price.filter(|&p| p > 0.0) {
        return Ok(pp);
    }
    let market = Market::parse(bot["market"].as_str().unwrap_or("KR"))?;
    if let Ok(q) = market::get_quote(state, uid, market, sym).await {
        if q.price > 0.0 {
            return Ok(q.price);
        }
    }
    if let Ok((_, cs)) = market::get_candles(state, uid, market, sym, Interval::D1, 5).await {
        if let Some(last) = cs.last() {
            return Ok(last.close);
        }
    }
    Err(ApiError::bad_request(
        "price_unavailable",
        "현재 시세를 확인할 수 없습니다 (웹훅 JSON에 price를 함께 보내거나 시세 연결을 확인하세요)",
    ))
}

pub async fn preview(state: &AppState, bot: &Value, today: &str) -> ApiResult<Value> {
    let st = bots::state(bot);
    let pos = &st["position"];
    let mut last_px = st
        .get("last_price")
        .and_then(|v| v.as_f64())
        .filter(|&x| x > 0.0)
        .or_else(|| pos.get("entry_price").and_then(|v| v.as_f64()).filter(|&x| x > 0.0))
        .unwrap_or(0.0);
    if last_px == 0.0 {
        if let Ok(px) = resolve_reference_price(state, bot, None, false).await {
            last_px = px;
        }
    }
    let cash = if bot["mode"].as_str() == Some("paper") {
        st["cash"].as_f64().unwrap_or(0.0)
    } else {
        (bot["capital"].as_f64().unwrap_or(0.0)
            - pos["qty"].as_f64().unwrap_or(0.0) * pos["entry_price"].as_f64().unwrap_or(0.0))
        .max(0.0)
    };
    Ok(json!({
        "date": today,
        "bar_date": st.get("last_run_date").and_then(|v| v.as_str()).unwrap_or(today),
        "due": false,
        "reason": "외부 웹훅(TradingView 등) 신호 수신 대기 중",
        "position": pos,
        "cash": cash,
        "today_open": Value::Null,
        "last": last_px,
        "orders": [],
    }))
}

pub async fn receive_webhook(
    state: &AppState,
    bot_id: &str,
    body: &Value,
    header_secret: Option<&str>,
    remote_ip: &str,
) -> ApiResult<Value> {
    check_rate_limit(bot_id, remote_ip)?;
    let mut bot = state
        .store
        .get(bots::BOTS, bot_id)
        .filter(|b| bots::bot_kind(b) == "webhook")
        .ok_or_else(|| ApiError::not_found("bot_not_found", "웹훅 봇을 찾을 수 없습니다"))?;

    let provided_secret = header_secret
        .filter(|s| !s.is_empty())
        .or_else(|| body.get("secret").and_then(|v| v.as_str()))
        .unwrap_or("");
    verify_secret(state, &bot, provided_secret)?;

    let uid = bot["user_id"].as_str().unwrap_or("").to_string();
    let user = users::get_user(state, &uid)
        .filter(|u| users::status_of(u) == "approved")
        .ok_or_else(|| ApiError::forbidden("user_not_approved", "승인된 계정이 아닙니다"))?;
    if user.get("autotrade_halted").and_then(|v| v.as_bool()).unwrap_or(false) {
        bots::log(
            state,
            &bot,
            "skip",
            "[웹훅] 전체 비상 중지 상태라 수신한 신호를 무시했습니다",
            "warn",
            None,
        )
        .await;
        return Err(ApiError::conflict(
            "autotrade_halted",
            "자동매매 전체 중지 상태입니다",
        ));
    }
    if !bot.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        bots::log(
            state,
            &bot,
            "skip",
            "[웹훅] 봇이 꺼져 있어 수신한 신호를 실행하지 않았습니다",
            "warn",
            None,
        )
        .await;
        return Err(ApiError::conflict(
            "bot_disabled",
            "웹훅 봇이 꺼져 있습니다. 먼저 봇을 켜 주세요",
        ));
    }

    let action_raw = body
        .get("action")
        .or_else(|| body.get("side"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let action = match action_raw.as_str() {
        "exit" | "close" | "flat" => "exit",
        "buy" | "long" => "buy",
        "sell" | "short_exit" => "sell",
        _ => {
            return Err(ApiError::bad_request(
                "invalid_action",
                "action은 buy / sell / exit 중 하나여야 합니다",
            ))
        }
    };

    let bot_sym = bot["symbol"].as_str().unwrap_or("").to_string();
    if let Some(inc) = body.get("symbol").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        let incoming_sym = inc.trim().to_uppercase();
        let incoming_sym = incoming_sym.rsplit(':').next().unwrap_or("").to_string();
        if !incoming_sym.is_empty() && incoming_sym != bot_sym {
            return Err(ApiError::bad_request(
                "symbol_mismatch",
                format!("봇 종목({bot_sym})과 웹훅 종목({incoming_sym})이 다릅니다"),
            ));
        }
    }

    let qty_pct = match body.get("qty_pct") {
        None | Some(Value::Null) => 100.0,
        Some(v) => v
            .as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            .ok_or_else(|| ApiError::bad_request("invalid_qty_pct", "qty_pct는 1 ~ 100 사이 숫자여야 합니다"))?,
    };
    if !(1.0..=100.0).contains(&qty_pct) {
        return Err(ApiError::bad_request(
            "invalid_qty_pct",
            "qty_pct는 1 ~ 100 사이 숫자여야 합니다",
        ));
    }

    let order_type = body
        .get("order_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("market")
        .trim()
        .to_lowercase();
    if order_type != "market" && order_type != "limit" {
        return Err(ApiError::bad_request(
            "invalid_order_type",
            "order_type은 market 또는 limit이어야 합니다",
        ));
    }

    let payload_price: Option<f64> = match body.get("price") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => None,
        Some(v) => {
            let p = v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| ApiError::bad_request("invalid_price", "price는 숫자여야 합니다"))?;
            if !p.is_finite() || p <= 0.0 {
                return Err(ApiError::bad_request("invalid_price", "price는 0보다 커야 합니다"));
            }
            Some(p)
        }
    };
    if order_type == "limit" && payload_price.is_none() {
        return Err(ApiError::bad_request(
            "price_required",
            "지정가(limit) 웹훅 주문에는 price 값이 필요합니다",
        ));
    }

    let default_reason = format!("외부 웹훅 ({})", action.to_uppercase());
    let reason: String = body
        .get("reason")
        .or_else(|| body.get("comment"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&default_reason)
        .trim()
        .chars()
        .take(100)
        .collect();

    let idem_raw = body
        .get("idempotency_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            body.get("alert_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            body.get("bar_time")
                .filter(|v| !v.is_null())
                .map(|v| format!("{}:{action}", v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string())))
        })
        .unwrap_or_default();
    let idem: String = idem_raw.trim().chars().take(80).collect();

    let mode = bot["mode"].as_str().unwrap_or("paper").to_string();
    let sk = bots::state_key(&mode).to_string();
    if !idem.is_empty() {
        let seen = bot["state"][&sk]
            .as_object_mut()
            .unwrap()
            .entry("webhook_seen".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap();
        if seen.iter().any(|x| x.as_str() == Some(idem.as_str())) {
            return Ok(json!({ "ok": true, "deduped": true, "action": action }));
        }
        seen.push(json!(idem));
        if seen.len() > 200 {
            let drop_n = seen.len() - 200;
            seen.drain(0..drop_n);
        }
    }

    let today = trading::kst_today();
    let sig_id = if idem.is_empty() {
        Uuid::new_v4().simple().to_string()[..12].to_string()
    } else {
        idem
    };
    let m_str = bot["market"].as_str().unwrap_or("KR").to_string();
    let market = Market::parse(&m_str)?;

    // 1) PAPER MODE
    if mode == "paper" {
        let ref_px = resolve_reference_price(state, &bot, payload_price, false).await?;
        let c = executor::costs_for(state, &uid, market).await;
        let res_qty: i64;
        if action == "buy" {
            let cur_eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
                + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * ref_px;
            if let Some(blk) = protections::check_buy_allowed(&bot, &bot["state"]["paper"], &today, Some(cur_eq)) {
                bots::log(state, &bot, "skip", &format!("[모의/웹훅] 신규 매수 보류: {blk}"), "warn", None).await;
                bots::save(state, &mut bot);
                return Ok(json!({ "ok": true, "skipped": blk }));
            }
            let px = ref_px * (1.0 + c.slippage_pct / 100.0);
            let unit = px * (1.0 + c.commission_pct / 100.0);
            let cash = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
            let spend = cash * (qty_pct / 100.0);
            let qty = if unit > 0.0 { (spend / unit).floor() as i64 } else { 0 };
            if qty <= 0 {
                bots::log(state, &bot, "skip", "[모의/웹훅] 가용 현금 부족으로 매수하지 못했습니다", "warn", None).await;
                bots::save(state, &mut bot);
                return Ok(json!({ "ok": true, "skipped": "insufficient_cash" }));
            }
            let pos_q = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) as i64;
            if pos_q == 0 {
                executor::paper_buy(&mut bot["state"]["paper"], qty, ref_px, c, &today, &reason, None, None);
            } else {
                let cost = (qty as f64) * unit;
                bot["state"]["paper"]["cash"] = json!(cash - cost);
                let old_ep = bot["state"]["paper"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
                let old_ed = bot["state"]["paper"]["position"]["entry_date"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| today.clone());
                let new_q = pos_q + qty;
                let avg = ((pos_q as f64) * old_ep + (qty as f64) * px) / (new_q as f64);
                bot["state"]["paper"]["position"] = json!({ "qty": new_q, "entry_price": avg, "entry_date": old_ed });
                let pk = bot["state"]["paper"].get("peak_price").and_then(|v| v.as_f64()).unwrap_or(avg).max(px);
                bot["state"]["paper"]["peak_price"] = json!(pk);
            }
            bots::log(state, &bot, "fill", &format!("[모의/웹훅] 매수 체결 {qty}주 @ {px:.2} — {reason}"), "info", None).await;
            res_qty = qty;
        } else {
            let pos_q = bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) as i64;
            if pos_q <= 0 {
                bots::log(state, &bot, "skip", "[모의/웹훅] 보유 수량이 없어 매도 신호를 건너뛰었습니다", "info", None).await;
                bots::save(state, &mut bot);
                return Ok(json!({ "ok": true, "skipped": "no_position" }));
            }
            let want_q = if action == "exit" || qty_pct >= 99.9 {
                pos_q
            } else {
                ((pos_q as f64) * qty_pct / 100.0).floor().max(1.0) as i64
            };
            let sell_q = want_q.min(pos_q);
            if sell_q >= pos_q {
                executor::paper_sell(&mut bot["state"]["paper"], ref_px, c, &today, &reason);
            } else {
                let px = ref_px * (1.0 - c.slippage_pct / 100.0);
                let net = (sell_q as f64) * px * (1.0 - (c.commission_pct + c.tax_pct) / 100.0);
                let cash = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0);
                bot["state"]["paper"]["cash"] = json!(cash + net);
                bot["state"]["paper"]["position"]["qty"] = json!(pos_q - sell_q);
            }
            bots::log(state, &bot, "fill", &format!("[모의/웹훅] 매도 체결 {sell_q}주 @ {ref_px:.2} — {reason}"), "info", None).await;
            res_qty = sell_q;
        }
        bot["state"]["paper"]["last_run_date"] = json!(today);
        bot["state"]["paper"]["last_price"] = json!(ref_px);
        let eq = bot["state"]["paper"]["cash"].as_f64().unwrap_or(0.0)
            + bot["state"]["paper"]["position"]["qty"].as_f64().unwrap_or(0.0) * ref_px;
        executor::record_day_equity(&mut bot["state"]["paper"], &today, eq);
        protections::evaluate_after_step(state, &mut bot, &today).await;
        bots::save(state, &mut bot);
        return Ok(json!({ "ok": true, "mode": "paper", "action": action, "qty": res_qty, "price": ref_px }));
    }

    // 2) SEMI & LIVE MODES
    if credentials::get_toss(state, &uid).is_none() {
        return Err(ApiError::conflict(
            "toss_credentials_required",
            "토스증권 API 키가 등록되어 있지 않습니다",
        ));
    }
    let _ = executor::reconcile_live(state, &mut bot, &today).await;
    let (client, seq) = account_client(state, &uid)?;
    let currency = if m_str == "KR" { "KRW" } else { "USD" };
    let d_cost = cost_defaults(state, &uid, market).await;
    let headroom = ((d_cost["slippage_pct"].as_f64().unwrap_or(0.05)
        + d_cost["commission_pct"].as_f64().unwrap_or(0.015))
        / 100.0)
        .max(LIVE_BUY_HEADROOM);
    let ref_px = resolve_reference_price(state, &bot, payload_price, true).await?;
    let pos_q = bot["state"]["live"]["position"]["qty"].as_f64().unwrap_or(0.0);
    let pos_ep = bot["state"]["live"]["position"]["entry_price"].as_f64().unwrap_or(0.0);
    let cap = bot["capital"].as_f64().unwrap_or(0.0);
    let realized = bot["state"]["live"].get("realized").and_then(|v| v.as_f64()).unwrap_or(0.0);

    let (side, qty, est_px): (&str, i64, f64) = if action == "buy" {
        let cur_eq = cap + realized + pos_q * (ref_px - pos_ep);
        if let Some(blk) = protections::check_buy_allowed(&bot, &bot["state"]["live"], &today, Some(cur_eq)) {
            bots::log(state, &bot, "skip", &format!("[웹훅] 신규 매수 보류: {blk}"), "warn", None).await;
            bots::save(state, &mut bot);
            return Ok(json!({ "ok": true, "skipped": blk }));
        }
        let mut bp_p = HashMap::new();
        bp_p.insert("currency".to_string(), currency.to_string());
        let bp = normalize(
            client
                .request("GET", "/api/v1/buying-power", Some(&bp_p), None, Some(seq))
                .await?
                .unwrap_or_else(|| json!({})),
            "",
        );
        let cash_bp = bp.get("cashBuyingPower").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let rem_cap = (cap - pos_q * pos_ep).max(0.0);
        let budget = rem_cap.min(cash_bp) * (qty_pct / 100.0);
        let est = if order_type == "limit" { payload_price.unwrap_or(ref_px) } else { ref_px };
        let q = if est > 0.0 {
            (budget / (est * (1.0 + headroom))).floor() as i64
        } else {
            0
        };
        ("BUY", q, est)
    } else {
        if pos_q <= 0.0 {
            bots::log(state, &bot, "skip", "[웹훅] 봇 보유 수량이 없어 매도 신호를 건너뛰었습니다", "info", None).await;
            bots::save(state, &mut bot);
            return Ok(json!({ "ok": true, "skipped": "no_position" }));
        }
        let mut sq = HashMap::new();
        sq.insert("symbol".to_string(), bot_sym.clone());
        let sellable = client
            .request("GET", "/api/v1/sellable-quantity", Some(&sq), None, Some(seq))
            .await?
            .and_then(|v| v.get("sellableQuantity").cloned())
            .and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(0.0) as i64;
        let pq = pos_q as i64;
        let want_q = if action == "exit" || qty_pct >= 99.9 {
            pq
        } else {
            ((pq as f64) * qty_pct / 100.0).floor().max(1.0) as i64
        };
        let q = want_q.min(pq).min(sellable);
        let est = if order_type == "limit" { payload_price.unwrap_or(ref_px) } else { ref_px };
        ("SELL", q, est)
    };

    if qty <= 0 {
        bots::log(
            state,
            &bot,
            "skip",
            &format!(
                "[웹훅] {} 부족으로 주문을 생략했습니다",
                if side == "BUY" { "매수 가능 자금" } else { "매도 가능 수량" }
            ),
            "warn",
            None,
        )
        .await;
        bots::save(state, &mut bot);
        return Ok(json!({ "ok": true, "skipped": "zero_qty" }));
    }

    if mode == "semi" {
        let now_dt = Utc::now();
        let sess = scheduler::active_session(state, &uid, &m_str, now_dt).await.ok().flatten();
        let exp_dt = sess.map(|s| s.2).unwrap_or_else(|| now_dt + Duration::hours(4));
        let app_id = Uuid::new_v4().simple().to_string()[..12].to_string();
        bot["state"]["live"]["pending_approval"] = json!({
            "id": app_id,
            "date": today,
            "created_at": now_dt.to_rfc3339(),
            "expires_at": exp_dt.to_rfc3339(),
            "reason": format!("[웹훅] {reason}"),
            "orders": [{
                "symbol": bot_sym,
                "side": side,
                "kind": order_type,
                "qty": qty,
                "signal_price": (ref_px * 10000.0).round() / 10000.0,
                "level": if order_type == "limit" { Some((est_px * 10000.0).round() / 10000.0) } else { None },
                "reason": reason,
                "text": format!(
                    "[웹훅] {bot_sym} {} {qty}주 ({}) — {reason}",
                    if side == "BUY" { "매수" } else { "매도" },
                    if order_type == "limit" { "지정가" } else { "시장가" }
                ),
            }],
        });
        bot["state"]["live"]["last_run_date"] = json!(today);
        bot["state"]["live"]["last_price"] = json!(ref_px);
        bots::log(
            state,
            &bot,
            "approval",
            &format!(
                "✋ [웹훅 승인 대기] {bot_sym} {} {qty}주 — {reason}",
                if side == "BUY" { "매수" } else { "매도" }
            ),
            "info",
            None,
        )
        .await;
        bots::save(state, &mut bot);
        return Ok(json!({ "ok": true, "mode": "semi", "staged": 1, "approval_id": app_id, "qty": qty }));
    }

    // LIVE execution
    if side == "SELL"
        && bot["state"]["live"]["pending"]
            .as_array()
            .is_some_and(|a| a.iter().any(|p| p.get("role").and_then(|v| v.as_str()) == Some("protection")))
    {
        executor::cancel_protection_conditionals(state, &mut bot).await;
    }

    let order_cid = cid(bot_id, &sig_id);
    let (r, sig_record_px) = if order_type == "limit" && payload_price.is_some() {
        let px = round_price(&m_str, payload_price.unwrap(), if side == "BUY" { "down" } else { "up" });
        let payload = json!({
            "clientOrderId": order_cid,
            "symbol": bot_sym,
            "side": side,
            "orderType": "LIMIT",
            "timeInForce": "DAY",
            "quantity": qty.to_string(),
            "price": fmt_price(&m_str, px),
        });
        let built = json!({
            "market": m_str,
            "payload": payload,
            "checks": { "estimated_value": (qty as f64) * px },
        });
        let res = trading::submit(state, &built, &uid, &format!("bot:{bot_id}:webhook")).await?;
        (res, px)
    } else {
        let payload = json!({
            "clientOrderId": order_cid,
            "symbol": bot_sym,
            "side": side,
            "orderType": "MARKET",
            "timeInForce": "DAY",
            "quantity": qty.to_string(),
        });
        let built = json!({
            "market": m_str,
            "payload": payload,
            "checks": { "estimated_value": (qty as f64) * ref_px },
        });
        let res = trading::submit(state, &built, &uid, &format!("bot:{bot_id}:webhook")).await?;
        (res, ref_px)
    };

    bot["state"]["live"]["pending"].as_array_mut().unwrap().push(json!({
        "kind": "order",
        "id": r["orderId"],
        "symbol": bot_sym,
        "side": side,
        "qty": qty,
        "date": today,
        "synced": 0.0,
        "signal_price": sig_record_px,
    }));
    bot["state"]["live"]["last_run_date"] = json!(today);
    bot["state"]["live"]["last_price"] = json!(ref_px);
    let eq = cap + realized + pos_q * (ref_px - pos_ep);
    executor::record_day_equity(&mut bot["state"]["live"], &today, eq);
    bots::log(
        state,
        &bot,
        "order",
        &format!(
            "[웹훅 실거래] {bot_sym} {} {qty}주 주문 전송 — {reason}",
            if side == "BUY" { "매수" } else { "매도" }
        ),
        "info",
        None,
    )
    .await;
    protections::evaluate_after_step(state, &mut bot, &today).await;
    bots::save(state, &mut bot);
    Ok(json!({ "ok": true, "mode": "live", "order_id": r["orderId"], "side": side, "qty": qty }))
}
