use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mighty_trade_server::auth::google::{enforce_email_policy, GoogleUserInfo, GoogleVerifier};
use mighty_trade_server::auth::otp::{current_step, generate_totp_code};
use mighty_trade_server::build_router;
use mighty_trade_server::config::Config;
use mighty_trade_server::errors::ApiResult;
use mighty_trade_server::integrations::toss::TossHttpOverride;
use mighty_trade_server::state::AppState;
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
        match (method, path) {
            ("GET", "/api/v1/holdings") => Ok(Some(json!({
                "holdings": [{
                    "symbol": "005930",
                    "name": "삼성전자",
                    "quantity": "10",
                    "rate": "0.0525",
                    "rateAfterCost": "0.05"
                }]
            }))),
            ("GET", "/api/v1/buying-power") => {
                let cur = params.and_then(|p| p.get("currency")).map(|s| s.as_str()).unwrap_or("KRW");
                let amt = if cur == "KRW" { "10000000" } else { "5000.50" };
                Ok(Some(json!({ "cashBuyingPower": amt })))
            }
            ("GET", "/api/v1/prices") => Ok(Some(json!([{ "symbol": "005930", "lastPrice": "70000" }]))),
            ("GET", "/api/v1/price-limits") => Ok(Some(json!({
                "upperLimitPrice": "91000",
                "lowerLimitPrice": "49000"
            }))),
            ("GET", "/api/v1/sellable-quantity") => Ok(Some(json!({ "sellableQuantity": "10" }))),
            ("GET", "/api/v1/market-calendar/KR") => Ok(Some(json!({ "today": {} }))),
            ("POST", "/api/v1/orders") => Ok(Some(json!({
                "orderId": "ord-1",
                "status": "ACCEPTED",
                "clientOrderId": json_body.and_then(|b| b.get("clientOrderId")).cloned().unwrap_or(Value::Null)
            }))),
            ("POST", "/api/v1/conditional-orders") => Ok(Some(json!({
                "conditionalOrderId": "oco-1",
                "status": "OPEN"
            }))),
            ("POST", "/api/v1/orders/ord-1/cancel") => Ok(Some(json!({ "orderId": "ord-1", "status": "CANCELLED" }))),
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
async fn market_account_and_trading_safety_rails() {
    let td = tempfile::tempdir().unwrap();
    let cfg = Config::test_default(td.path().to_path_buf());
    let mut state = AppState::new(cfg).unwrap();
    state.google_verifier = Arc::new(StaticGoogle);
    let fake_toss = Arc::new(FakeToss {
        calls: Mutex::new(Vec::new()),
    });
    state.toss_override = Some(fake_toss.clone());
    let app = build_router(state.clone());

    let mut c = Client::new(app);
    // Admin logs in directly (auto-approved)
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
        Some(json!({ "code": generate_totp_code(&secret, step0).unwrap() })),
    )
    .await;

    // 1. Market mock endpoints
    let (st, cdl) = c
        .req("GET", "/api/market/candles?market=KR&symbol=005930&interval=1d&limit=50", None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cdl["candles"].as_array().unwrap().len(), 50);

    let (st, qts) = c
        .req("GET", "/api/market/quotes?market=US&symbols=AAPL,NVDA", None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(qts["items"].as_array().unwrap().len(), 2);
    for item in qts["items"].as_array().unwrap() {
        assert!(item["open"].as_f64().is_some_and(|v| v > 0.0), "open missing: {item}");
        assert!(item["high"].as_f64().is_some_and(|v| v > 0.0), "high missing: {item}");
        assert!(item["low"].as_f64().is_some_and(|v| v > 0.0), "low missing: {item}");
        assert!(item["prev_close"].as_f64().is_some_and(|v| v > 0.0), "prev_close missing: {item}");
    }

    let (st, srch) = c.req("GET", "/api/market/search?market=KR&q=삼성", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(srch["items"][0]["symbol"], "005930");

    let (st, regs) = c.req("GET", "/api/market/regimes", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(regs["KR"]["regime"].is_string() && regs["US"]["regime"].is_string());

    // 2. Register Toss credentials directly in store for testing Account + Trading
    credentials::save_toss(&state, &uid, "cid-1", "csec-1", Some(12345));

    // 3. Account summary converts string decimals & rates
    let (st, acc) = c.req("GET", "/api/account/summary", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(acc["buying_power"]["KRW"], 10000000.0);
    assert_eq!(acc["holdings"]["holdings"][0]["ratePct"], 5.25);

    // 4. Manual order defaults to dry_run=true; live requires OTP unlock
    let order_body = json!({
        "market": "KR",
        "symbol": "005930",
        "side": "BUY",
        "order_type": "LIMIT",
        "quantity": 5,
        "price": 70040
    });
    let (st, dry) = c.req("POST", "/api/trading/orders", Some(order_body.clone())).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dry["dry_run"], true);
    assert_eq!(dry["payload"]["price"], "70000"); // BUY rounds down to KRX 100 KRW tick

    // Sending dry_run=false while locked -> 403 trading_locked
    let mut live_body = order_body.clone();
    live_body.as_object_mut().unwrap().insert("dry_run".to_string(), json!(false));
    let (st, locked) = c.req("POST", "/api/trading/orders", Some(live_body.clone())).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(locked["error"], "trading_locked");

    // Unlock with next OTP step and place real order
    let (st, unl) = c
        .req(
            "POST",
            "/api/trading/unlock",
            Some(json!({ "otp_code": generate_totp_code(&secret, step0 + 1).unwrap() })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(unl["unlocked"], true);

    let (st, sent) = c.req("POST", "/api/trading/orders", Some(live_body)).await;
    assert_eq!(st, StatusCode::OK, "live order failed: {sent}");
    assert_eq!(sent["dry_run"], false);
    assert_eq!(sent["result"]["orderId"], "ord-1");

    // Cancel works even without unlock
    let (st, can) = c.req("POST", "/api/trading/orders/ord-1/cancel", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(can["result"]["status"], "CANCELLED");

    // 5. OCO dry_run check
    let (st, oco) = c
        .req(
            "POST",
            "/api/account/conditional-orders/oco",
            Some(json!({
                "market": "KR",
                "symbol": "005930",
                "quantity": 5,
                "take_profit": { "trigger": 77000, "price": 76900 },
                "stop_loss": { "trigger": 65000, "price": 64800 }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "oco dry_run failed: {oco}");
    assert_eq!(oco["dry_run"], true);
    assert_eq!(oco["payload"]["type"], "OCO");
}
