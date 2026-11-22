use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use mighty_trade_server::auth::google::{enforce_email_policy, GoogleUserInfo, GoogleVerifier};
use mighty_trade_server::auth::otp::{current_step, generate_totp_code};
use mighty_trade_server::autotrade::{bots, notifier, protections, semi, webhook};
use mighty_trade_server::build_router;
use mighty_trade_server::config::Config;
use mighty_trade_server::errors::ApiResult;
use mighty_trade_server::integrations::toss::TossHttpOverride;
use mighty_trade_server::state::{AppState, NotifierHook};
use mighty_trade_server::users::credentials;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

struct StaticGoogle;

#[async_trait]
impl GoogleVerifier for StaticGoogle {
    async fn verify(&self, cfg: &Config, cred: &str) -> ApiResult<GoogleUserInfo> {
        let info = GoogleUserInfo {
            sub: cred.to_string(),
            email: format!("{cred}@example.com"),
            email_verified: true,
            name: Some(cred.to_string()),
            picture: None,
        };
        enforce_email_policy(cfg, &info)?;
        Ok(info)
    }
}

struct CaptureNotifier {
    sent: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl NotifierHook for CaptureNotifier {
    async fn send_http(&self, url: &str, payload: &Value) -> Result<(), String> {
        self.sent.lock().push((url.to_string(), payload.clone()));
        Ok(())
    }
}

struct FakeToss {
    calls: Mutex<Vec<(String, String, Option<Value>)>>,
}

#[async_trait]
impl TossHttpOverride for FakeToss {
    async fn request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        json_body: Option<&Value>,
        _account_seq: Option<i64>,
    ) -> ApiResult<Option<Value>> {
        self.calls
            .lock()
            .push((method.to_string(), path.to_string(), json_body.cloned()));
        let now = Utc::now();
        let today = now.date_naive().to_string();
        let st_iso = (now - Duration::hours(1)).to_rfc3339();
        let en_iso = (now + Duration::hours(4)).to_rfc3339();

        match (method, path) {
            ("GET", "/api/v1/market-calendar/KR") => Ok(Some(json!({
                "today": {
                    "date": today,
                    "integrated": {
                        "regularMarket": { "startTime": st_iso, "endTime": en_iso }
                    }
                }
            }))),
            ("GET", "/api/v1/market-calendar/US") => Ok(Some(json!({
                "today": {
                    "date": today,
                    "regularMarket": { "startTime": st_iso, "endTime": en_iso }
                }
            }))),
            ("GET", "/api/v1/buying-power") => {
                let cur = params.and_then(|p| p.get("currency")).map(|s| s.as_str()).unwrap_or("KRW");
                let amt = if cur == "KRW" { "20000000" } else { "20000" };
                Ok(Some(json!({ "cashBuyingPower": amt })))
            }
            ("GET", "/api/v1/prices") => {
                let syms = params.and_then(|p| p.get("symbols")).cloned().unwrap_or_else(|| "005930".to_string());
                let arr: Vec<Value> = syms
                    .split(',')
                    .map(|s| json!({ "symbol": s, "lastPrice": "70000" }))
                    .collect();
                Ok(Some(Value::Array(arr)))
            }
            ("GET", "/api/v1/sellable-quantity") => Ok(Some(json!({ "sellableQuantity": "100" }))),
            ("POST", "/api/v1/orders") => Ok(Some(json!({
                "orderId": "ord-auto-1",
                "status": "ACCEPTED",
                "clientOrderId": json_body.and_then(|b| b.get("clientOrderId")).cloned().unwrap_or(Value::Null)
            }))),
            ("POST", "/api/v1/orders/ord-auto-1/cancel") => Ok(Some(json!({ "orderId": "ord-auto-1", "status": "CANCELED" }))),
            ("GET", "/api/v1/orders/ord-auto-1") => Ok(Some(json!({
                "orderId": "ord-auto-1",
                "status": "FILLED",
                "execution": { "filledQuantity": "10", "averageFilledPrice": "70100" }
            }))),
            ("POST", "/api/v1/conditional-orders") => Ok(Some(json!({
                "conditionalOrderId": "co-auto-1",
                "status": "OPEN"
            }))),
            _ => Ok(Some(json!({}))),
        }
    }
}

struct Client {
    app: axum::Router,
    cookies: HashMap<String, String>,
}

impl Client {
    fn new(app: axum::Router) -> Self {
        Self {
            app,
            cookies: HashMap::new(),
        }
    }

    async fn req(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.req_with_headers(method, uri, body, &[]).await
    }

    async fn req_with_headers(
        &mut self,
        method: &str,
        uri: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !self.cookies.is_empty() {
            let hdr = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            builder = builder.header(header::COOKIE, hdr);
        }
        for &(k, v) in extra {
            builder = builder.header(k, v);
        }
        let req = if let Some(b) = body {
            builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string()))
                .unwrap()
        } else {
            builder.body(Body::empty()).unwrap()
        };
        let resp = self.app.clone().oneshot(req).await.unwrap();
        let st = resp.status();
        for hv in resp.headers().get_all(header::SET_COOKIE) {
            if let Ok(s) = hv.to_str() {
                if let Some(first) = s.split(';').next() {
                    if let Some((k, v)) = first.split_once('=') {
                        if v.is_empty() || s.contains("Max-Age=0") {
                            self.cookies.remove(k.trim());
                        } else {
                            self.cookies.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }
            }
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn autotrade_all_6_bot_categories_protections_semi_webhook_killswitch() {
    webhook::reset_rate_limits();
    let td = tempfile::tempdir().unwrap();
    let cfg = Config::test_default(td.path().to_path_buf());
    let mut state = AppState::new(cfg).unwrap();
    state.google_verifier = Arc::new(StaticGoogle);
    let capture = Arc::new(CaptureNotifier {
        sent: Mutex::new(Vec::new()),
    });
    state.notifier_hook = capture.clone();
    let fake_toss = Arc::new(FakeToss {
        calls: Mutex::new(Vec::new()),
    });
    state.toss_override = Some(fake_toss.clone());
    let app = build_router(state.clone());

    let mut c = Client::new(app);
    // Log in admin + setup OTP
    let (st, r) = c
        .req("POST", "/api/auth/google", Some(json!({ "credential": "admin" })))
        .await;
    assert_eq!(st, StatusCode::OK);
    let uid = r["user"]["id"].as_str().unwrap().to_string();
    let (_, setup) = c.req("POST", "/api/auth/otp/setup", None).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    let step0 = current_step(Utc::now());
    c.req(
        "POST",
        "/api/auth/otp/verify",
        Some(json!({ "code": generate_totp_code(&secret, step0 - 1).unwrap() })),
    )
    .await;

    // Configure Toss creds & notifications
    credentials::save_toss(&state, &uid, "cid-1", "csec-1", Some(999));
    credentials::save_notifications(
        &state,
        &uid,
        &json!({
            "enabled": true,
            "paper_notify": true,
            "discord_webhook_url": "https://discord.com/api/webhooks/123/abc"
        }),
    )
    .unwrap();

    // 1. Create all 6 bot categories:
    // (a) Swing single bot
    let (st, b_swing) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "single",
                "market": "KR",
                "symbol": "005930",
                "strategy": "ma_cross",
                "capital": 5000000,
                "risk": { "stop_loss_pct": 5.0, "take_profit_pct": 15.0 },
                "guard": { "max_drawdown_pct": 10.0, "max_consecutive_losses": 3, "cooldown_days": 2 }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create swing bot failed: {b_swing}");
    let swing_id = b_swing["bot"]["id"].as_str().unwrap().to_string();

    // (b) Intraday daytrade bot
    let (st, b_day) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "single",
                "market": "KR",
                "symbol": "005930",
                "strategy": "orb_breakout",
                "capital": 3000000
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create daytrade bot failed: {b_day}");
    assert_eq!(b_day["bot"]["category"], "daytrade");
    assert_eq!(b_day["bot"]["timeframe"], "15m");

    // (c) Rebalance bot (buy_only=true)
    let (st, b_rebal) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "rebalance",
                "market": "US",
                "model": "all_weather",
                "capital": 10000,
                "buy_only": true
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create rebalance bot failed: {b_rebal}");
    let rebal_id = b_rebal["bot"]["id"].as_str().unwrap().to_string();
    assert_eq!(b_rebal["bot"]["buy_only"], true);

    // (d) Rotation bot
    let (st, b_rot) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "rotation",
                "market": "KR",
                "preset": "momentum_quality",
                "universe": "kr_all_150",
                "top_n": 3,
                "hold_buffer": 6,
                "rebalance": "monthly",
                "capital": 10000000
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create rotation bot failed: {b_rot}");
    let rot_id = b_rot["bot"]["id"].as_str().unwrap().to_string();

    // (e) Smart DCA bot
    let (st, b_dca) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "dca",
                "market": "KR",
                "symbol": "005930",
                "rebalance": "daily",
                "base_amount": 500000,
                "capital": 5000000,
                "smart_scale": true
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create dca bot failed: {b_dca}");
    let dca_id = b_dca["bot"]["id"].as_str().unwrap().to_string();

    // (f) Inbound TradingView Webhook bot
    let (st, b_wh) = c
        .req(
            "POST",
            "/api/autotrade/bots",
            Some(json!({
                "kind": "webhook",
                "market": "KR",
                "symbol": "005930",
                "capital": 4000000
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "create webhook bot failed: {b_wh}");
    let wh_id = b_wh["bot"]["id"].as_str().unwrap().to_string();
    assert!(b_wh["bot"].get("webhook_secret_enc").is_none());
    assert!(b_wh["bot"]["webhook_secret_hint"].is_string());

    // 2. Rotation historical backtest endpoint (/api/quant/screener/rotation-backtest)
    let (st, rbt) = c
        .req(
            "POST",
            "/api/quant/screener/rotation-backtest",
            Some(json!({
                "market": "KR",
                "preset": "momentum_quality",
                "universe": "kr_all_150",
                "top_n": 3,
                "hold_buffer": 6,
                "cadence": "monthly",
                "period": "1y",
                "capital": 10000000
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "rotation backtest failed: {rbt}");
    assert!(!rbt["equity"].as_array().unwrap().is_empty());

    // 3. Evaluate & Run paper bots (swing, rebalance, rotation, dca)
    for bid in [&swing_id, &rebal_id, &rot_id, &dca_id] {
        let (st, ev) = c.req("POST", &format!("/api/autotrade/bots/{bid}/evaluate"), None).await;
        assert_eq!(st, StatusCode::OK, "evaluate {bid} failed: {ev}");
        let (st, rn) = c.req("POST", &format!("/api/autotrade/bots/{bid}/run"), None).await;
        assert_eq!(st, StatusCode::OK, "run {bid} failed: {rn}");
    }

    // 4. Enable Webhook bot, retrieve secret, send BUY -> dedupe -> SELL
    let (st, _) = c.req("POST", &format!("/api/autotrade/bots/{wh_id}/enable"), None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, sec_r) = c
        .req("POST", &format!("/api/autotrade/bots/{wh_id}/webhook-secret"), Some(json!({ "rotate": false })))
        .await;
    assert_eq!(st, StatusCode::OK);
    let wh_secret = sec_r["secret"].as_str().unwrap().to_string();

    // Wrong secret -> 403
    let (st, _) = c
        .req_with_headers(
            "POST",
            &format!("/api/autotrade/webhook/{wh_id}"),
            Some(json!({ "action": "buy", "price": 70000 })),
            &[("X-Webhook-Secret", "wrong-secret")],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Valid BUY with idempotency_key -> executes paper buy
    let (st, buy_r) = c
        .req_with_headers(
            "POST",
            &format!("/api/autotrade/webhook/{wh_id}"),
            Some(json!({ "action": "buy", "price": 70000, "idempotency_key": "alert-1" })),
            &[("X-Webhook-Secret", &wh_secret)],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "webhook buy failed: {buy_r}");
    assert!(buy_r["qty"].as_i64().unwrap() > 0);

    // Repeat same idempotency_key -> deduped!
    let (st, dup_r) = c
        .req_with_headers(
            "POST",
            &format!("/api/autotrade/webhook/{wh_id}"),
            Some(json!({ "action": "buy", "price": 70000, "idempotency_key": "alert-1" })),
            &[("X-Webhook-Secret", &wh_secret)],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dup_r["deduped"], true);

    // Exit webhook -> closes position
    let (st, exit_r) = c
        .req_with_headers(
            "POST",
            &format!("/api/autotrade/webhook/{wh_id}"),
            Some(json!({ "action": "exit", "price": 72000, "idempotency_key": "alert-2" })),
            &[("X-Webhook-Secret", &wh_secret)],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "webhook exit failed: {exit_r}");
    assert_eq!(exit_r["action"], "exit");

    // 5. Semi-auto 1-click approval flow (stage -> approve with OTP -> Toss order placed)
    let (st, m_res) = c
        .req(
            "POST",
            &format!("/api/autotrade/bots/{dca_id}/mode"),
            Some(json!({ "mode": "semi", "otp_code": generate_totp_code(&secret, step0).unwrap() })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "switch to semi failed: {m_res}");
    assert_eq!(m_res["bot"]["mode"], "semi");

    // Stage a semi-auto proposal directly via scheduler helper & approve via HTTP
    {
        let mut dca_bot = bots::get(&state, &uid, &dca_id).unwrap();
        let today = Utc::now().date_naive().to_string();
        let stage = semi::stage_from_scheduler(
            &state,
            &mut dca_bot,
            &today,
            Utc::now() + Duration::hours(3),
            None,
        )
        .await
        .unwrap();
        assert_eq!(stage["staged"], 1, "expected 1 staged order: {stage}");
    }
    let (st, app_r) = c
        .req(
            "POST",
            &format!("/api/autotrade/bots/{dca_id}/approve"),
            Some(json!({ "otp_code": generate_totp_code(&secret, step0 + 1).unwrap() })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "approve failed: {app_r}");
    assert_eq!(app_r["result"]["sent"], 1);

    // 6. Control Tower Overview (/api/autotrade/overview)
    let (st, ov) = c.req("GET", "/api/autotrade/overview", None).await;
    assert_eq!(st, StatusCode::OK, "overview failed: {ov}");
    assert_eq!(ov["total_bots"], 6);
    assert!(!ov["duplicate_symbols"].as_array().unwrap().is_empty());

    // 7. Protections circuit breaker trips on MDD >= 10% and re-enable clears lock
    {
        let mut sbot = bots::get(&state, &uid, &swing_id).unwrap();
        sbot["enabled"] = json!(true);
        sbot["state"]["paper"]["equity"] = json!([
            { "date": "2026-01-01", "value": 5000000.0 },
            { "date": "2026-01-02", "value": 4200000.0 }
        ]);
        let tripped = protections::evaluate_after_step(&state, &mut sbot, "2026-01-02").await;
        assert!(tripped.is_some());
        assert_eq!(sbot["enabled"], false);
        bots::save(&state, &mut sbot);
    }
    let (st, en_r) = c.req("POST", &format!("/api/autotrade/bots/{swing_id}/enable"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(en_r["bot"]["enabled"], true);
    assert!(en_r["bot"]["state"]["tripped"].is_null());

    // 8. Daily briefing dedupe + notification delivery
    let today_s = Utc::now().date_naive().to_string();
    assert!(notifier::maybe_send_daily_briefing(&state, &uid, "KR", &today_s).await);
    assert!(!notifier::maybe_send_daily_briefing(&state, &uid, "KR", &today_s).await);
    assert!(!capture.sent.lock().is_empty());

    // 9. Global Kill Switch (/halt cancels live pending & blocks webhook; /resume requires OTP)
    let (st, halt_r) = c.req("POST", "/api/autotrade/halt", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(halt_r["halted"], true);
    assert_eq!(halt_r["cancelled"], 1); // cancelled the approved DCA live pending order

    let (st, _) = c
        .req_with_headers(
            "POST",
            &format!("/api/autotrade/webhook/{wh_id}"),
            Some(json!({ "action": "buy", "price": 70000 })),
            &[("X-Webhook-Secret", &wh_secret)],
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Clear otp_last_step to simulate 30s elapsed before OTP resume
    mighty_trade_server::users::repository::update_user(&state, &uid, json!({ "otp_last_step": Value::Null })).unwrap();
    let (st, res_r) = c
        .req(
            "POST",
            "/api/autotrade/resume",
            Some(json!({ "otp_code": generate_totp_code(&secret, step0).unwrap() })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "resume failed: {res_r}");
    assert_eq!(res_r["halted"], false);
}
