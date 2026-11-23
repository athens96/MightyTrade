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
use mighty_trade_server::integrations::kiwoom::KiwoomHttpOverride;
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

struct FakeKiwoom {
    calls: Mutex<Vec<(bool, String, String, Value)>>,
}

#[async_trait]
impl KiwoomHttpOverride for FakeKiwoom {
    async fn tr_post(
        &self,
        is_mock: bool,
        path: &str,
        api_id: &str,
        body: &Value,
        _cont_yn: &str,
        _next_key: &str,
    ) -> ApiResult<Value> {
        self.calls
            .lock()
            .push((is_mock, path.to_string(), api_id.to_string(), body.clone()));
        match api_id {
            "ka10001" => Ok(json!({
                "stk_cd": "005930",
                "stk_nm": "삼성전자",
                "cur_prc": "+72000",
                "upl_pric": "+93600",
                "lst_pric": "-50400",
            })),
            "usa10098" => Ok(json!({
                "list": [{ "stex_tp": "ND", "stk_cd": body["stk_cd"], "stk_nm": "애플", "stk_enm": "APPLE INC", "mkgb": "NASDAQ" }]
            })),
            "usa10099" => Ok(json!({
                "list": [
                    { "stex_tp": "ND", "stk_cd": "AAPL", "stk_nm": "애플", "stk_enm": "APPLE INC", "mkgb": "NASDAQ" },
                    { "stex_tp": "ND", "stk_cd": "NVDA", "stk_nm": "엔비디아", "stk_enm": "NVIDIA CORP", "mkgb": "NASDAQ" }
                ]
            })),
            "usa20100" => Ok(json!({
                "stex_tp": "ND",
                "stk_cd": body["stk_cd"],
                "cur_prc": "+230.5000",
                "base_close_pric": "228.0000"
            })),
            "usa20101" => Ok(json!({
                "sel_1bid": "+230.5500", "sel_1bid_req": "40",
                "buy_1bid": "+230.4800", "buy_1bid_req": "55"
            })),
            "usa20151" => Ok(json!({
                "result_list": [{ "dt": "20261123", "cur_prc": "+230.5000", "pred_pre": "+2.5000", "acc_trde_qty": "45000000" }]
            })),
            "usa06011" => {
                assert_eq!(body["exrt_appl_tp"], json!("0"));
                Ok(json!({
                    "result_list": [
                        { "cntr_tm": "20261120093500", "bus_dt": "20261120", "open_pric": "230.1000", "high_pric": "230.6000", "low_pric": "230.0000", "cur_prc": "230.5000", "trde_qty": "12500" },
                        { "cntr_tm": "20261120093400", "bus_dt": "20261120", "open_pric": "229.8000", "high_pric": "230.2000", "low_pric": "229.7000", "cur_prc": "230.1000", "trde_qty": "9800" },
                    ]
                }))
            }
            "usa06012" => {
                assert_eq!(body["exrt_appl_tp"], json!("0"));
                Ok(json!({
                    "result_list": [
                        { "dt": "20261123", "open_pric": "228.5000", "high_pric": "231.2000", "low_pric": "227.9000", "cur_prc": "230.5000", "acc_trde_qty": "45000000" },
                        { "dt": "20261120", "open_pric": "226.0000", "high_pric": "229.0000", "low_pric": "225.5000", "cur_prc": "228.0000", "acc_trde_qty": "39000000" }
                    ]
                }))
            }
            "ka10081" => Ok(json!({
                "stk_dt_pole_chart_qry": [
                    { "dt": "20261123", "open_pric": "71500", "high_pric": "72500", "low_pric": "71200", "cur_prc": "+72000", "trde_qty": "1200000" },
                    { "dt": "20261120", "open_pric": "70500", "high_pric": "71500", "low_pric": "70000", "cur_prc": "+71000", "trde_qty": "980000" }
                ]
            })),
            "ka10004" => Ok(json!({
                "sel_fpr_bid": "+72100", "sel_fpr_req": "1500",
                "buy_fpr_bid": "+72000", "buy_fpr_req": "2100",
                "tot_sel_req": "1500", "tot_buy_req": "2100"
            })),
            "ka10003" => Ok(json!({
                "cntr_infr": [
                    { "tm": "110500", "cur_prc": "+72000", "cntr_trde_qty": "+25" }
                ]
            })),
            "ka10059" => Ok(json!({
                "stk_invsr_orgn": [
                    { "dt": "20261123", "ind_invsr": "-5000", "frgnr_invsr": "+3500", "orgn": "+1500" }
                ]
            })),
            "ka10099" => Ok(json!({
                "list": [
                    { "code": "005930", "name": "삼성전자" },
                    { "code": "000660", "name": "SK하이닉스" }
                ]
            })),
            "kt00018" => Ok(json!({
                "tot_evlt_amt": "720000",
                "tot_evlt_pl": "+20000",
                "tot_prft_rt": "+2.86",
                "acnt_evlu_remn_indv_tot": [{
                    "stk_cd": "A005930",
                    "stk_nm": "삼성전자",
                    "rmnd_qty": "10",
                    "pur_pric": "70000",
                    "cur_prc": "72000",
                    "evlt_amt": "720000",
                    "evltv_prft": "20000",
                    "prft_rt": "2.86"
                }]
            })),
            "kt00001" => Ok(json!({ "ord_alow_amt": "15000000" })),
            "ust21110" => Ok(json!({
                "result_list": [
                    { "crnc_code": "USD", "fc_ord_alowa": "8500.25", "fc_entra": "8500.25" }
                ]
            })),
            "ust21070" => Ok(json!({
                "tot_evlt_amt": "2305.00",
                "tot_pl_amt": "105.00",
                "result_list": [{
                    "stk_cd": "AAPL",
                    "frgn_stk_nm": "애플",
                    "poss_qty": "000000000010",
                    "sell_alowq": "000000000010",
                    "frgn_stk_book_uv": "220.0000",
                    "now_pric": "230.5000",
                    "evlt_amt": "2305.0000",
                    "pl_amt": "105.0000",
                    "pl_rt": "4.77"
                }]
            })),
            "ust21510" => Ok(json!({
                "result_lsit": [{
                    "ord_no": "KW-US-001",
                    "stk_cd": "AAPL",
                    "frgn_stk_nm": "애플",
                    "slby_tp": "2",
                    "slby_tp_nm": "매수",
                    "ord_qty": "000000000002",
                    "cntr_qty": "000000000000",
                    "ord_remnq": "000000000002",
                    "ord_uv": "230.5000",
                    "ord_stat": "접수",
                    "ord_time": "22:35:00"
                }]
            })),
            "kt10000" => Ok(json!({ "ord_no": "KW-KR-001" })),
            "ust20000" => Ok(json!({ "ord_no": "KW-US-001" })),
            "ust20003" => Ok(json!({ "ord_no": "KW-US-001" })),
            "kt10003" => Ok(json!({ "ord_no": "KW-KR-001" })),
            "ka10075" => Ok(json!({
                "oso": [{
                    "ord_no": "KW-KR-001",
                    "stk_cd": "A005930",
                    "stk_nm": "삼성전자",
                    "ord_qty": "5",
                    "oso_qty": "5",
                    "ord_pric": "72000",
                    "io_tp_nm": "매수",
                    "tm": "110600"
                }]
            })),
            "ka10076" => Ok(json!({ "cntr": [] })),
            _ => Ok(json!({})),
        }
    }

    async fn ws_conditions(&self, _is_mock: bool) -> ApiResult<Vec<(String, String)>> {
        Ok(vec![
            ("0".to_string(), "단기돌파검색".to_string()),
            ("1".to_string(), "외국인수급급증".to_string()),
        ])
    }

    async fn ws_condition_matches(&self, _is_mock: bool, _seq: &str) -> ApiResult<Vec<Value>> {
        Ok(vec![
            json!({ "9001": "A005930", "302": "삼성전자", "10": "+72000", "11": "+1000", "12": "+1.41", "13": "1200000" }),
            json!({ "9001": "A000660", "302": "SK하이닉스", "10": "+198000", "11": "+3500", "12": "+1.80", "13": "850000" }),
        ])
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
        let body = if let Some(v) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        } else {
            Body::empty()
        };
        let resp = self
            .app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        for hv in resp.headers().get_all(header::SET_COOKIE) {
            if let Ok(s) = hv.to_str() {
                if let Some((kv, _)) = s.split_once(';') {
                    if let Some((k, v)) = kv.split_once('=') {
                        if v.is_empty() {
                            self.cookies.remove(k.trim());
                        } else {
                            self.cookies.insert(k.trim().to_string(), v.trim().to_string());
                        }
                    }
                }
            }
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, val)
    }
}

#[tokio::test]
async fn kiwoom_rest_account_market_trading_and_conditions_e2e() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = Config::test_default(tmp.path().to_path_buf());
    cfg.market_providers.insert("KR".to_string(), "toss".to_string());
    cfg.market_providers.insert("US".to_string(), "toss".to_string());
    let mut state = AppState::new(cfg).unwrap();
    state.google_verifier = Arc::new(StaticGoogle);
    let fake_kw = Arc::new(FakeKiwoom {
        calls: Mutex::new(Vec::new()),
    });
    state.kiwoom_override = Some(fake_kw.clone());

    let app = build_router(state.clone());
    let mut c = Client::new(app);

    // 1. Admin login + OTP setup
    let (st, v) = c.req("POST", "/api/auth/google", Some(json!({ "credential": "admin" }))).await;
    assert_eq!(st, StatusCode::OK);
    let uid = v["user"]["id"].as_str().unwrap().to_string();
    let (st, enroll) = c.req("POST", "/api/auth/otp/setup", None).await;
    assert_eq!(st, StatusCode::OK);
    let secret = enroll["secret"].as_str().unwrap().to_string();
    let step0 = current_step(Utc::now());
    let code_login = generate_totp_code(&secret, step0 - 1).unwrap();
    let code_unlock = generate_totp_code(&secret, step0).unwrap();
    let (st, _) = c.req("POST", "/api/auth/otp/verify", Some(json!({ "code": code_login }))).await;
    assert_eq!(st, StatusCode::OK);

    // 2. Save Kiwoom mock credentials + verify overview & active broker switching
    credentials::save_kiwoom(&state, &uid, "kw_app_1234567890", "kw_sec_secret", "5012345610", true, true);
    let (st, ov) = c.req("GET", "/api/users/me/brokers", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ov["active_broker"], json!("kiwoom"));
    assert_eq!(ov["kiwoom"]["configured"], json!(true));
    assert_eq!(ov["kiwoom"]["is_mock"], json!(true));

    // Switching to unconfigured Toss should fail cleanly
    let (st, _) = c.req("PUT", "/api/users/me/broker/active", Some(json!({ "broker": "toss" }))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 3. Market candles, quote, orderbook, search, investor flow via Kiwoom
    let (st, candles) = c.req("GET", "/api/market/candles?market=KR&symbol=005930&interval=1d&limit=10", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(candles["provider"], json!("kiwoom"));
    assert_eq!(candles["candles"].as_array().unwrap().len(), 2);

    let (st, quote) = c.req("GET", "/api/market/quote?market=KR&symbol=005930", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(quote["quote"]["price"], json!(72000.0));

    let (st, book) = c.req("GET", "/api/market/orderbook?market=KR&symbol=005930", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(book["orderbook"]["asks"][0]["price"], json!(72100.0));
    assert_eq!(book["orderbook"]["bids"][0]["price"], json!(72000.0));

    let (st, srch) = c.req("GET", "/api/market/search?market=KR&q=삼성", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(srch["items"][0]["symbol"], json!("005930"));

    // 3b. Official Kiwoom US stock candles (usa06012 daily + usa06011 minute), quote (usa20100), orderbook (usa20101), search (usa10099)
    let (st, us_candles) = c.req("GET", "/api/market/candles?market=US&symbol=AAPL&interval=1d&limit=10", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(us_candles["provider"], json!("kiwoom"));
    assert_eq!(us_candles["candles"].as_array().unwrap().len(), 2);
    assert_eq!(us_candles["candles"][1]["volume"], json!(45000000.0));

    let (st, us_min) = c.req("GET", "/api/market/candles?market=US&symbol=AAPL&interval=1m&limit=10", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(us_min["candles"].as_array().unwrap().len(), 2);
    assert_eq!(us_min["candles"][1]["close"], json!(230.5));

    let (st, us_quote) = c.req("GET", "/api/market/quote?market=US&symbol=AAPL", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(us_quote["quote"]["price"], json!(230.5));
    assert_eq!(us_quote["quote"]["change"], json!(2.5));
    assert_eq!(us_quote["quote"]["currency"], json!("USD"));

    let (st, us_book) = c.req("GET", "/api/market/orderbook?market=US&symbol=AAPL", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(us_book["orderbook"]["asks"][0]["price"], json!(230.55));
    assert_eq!(us_book["orderbook"]["bids"][0]["price"], json!(230.48));

    let (st, us_srch) = c.req("GET", "/api/market/search?market=US&q=NVDA", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(us_srch["items"][0]["symbol"], json!("NVDA"));

    // 4. Account summary (KR + US holdings + USD buying power) & OCO graceful notice
    let (st, acc) = c.req("GET", "/api/account/summary", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(acc["broker"], json!("kiwoom"));
    assert_eq!(acc["kiwoom_mock"], json!(true));
    assert_eq!(acc["buying_power"]["KRW"], json!(15000000.0));
    assert_eq!(acc["buying_power"]["USD"], json!(8500.25));
    assert_eq!(acc["holdings"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(acc["holdings"]["items"][0]["symbol"], json!("005930"));
    assert_eq!(acc["holdings"]["items"][1]["symbol"], json!("AAPL"));

    let (st, cond) = c.req("GET", "/api/account/conditional-orders?status=OPEN", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cond["items"].as_array().unwrap().len(), 0);

    // 5. Direct trading preview + OTP unlock + KR Buy + US Buy + Cancel via Kiwoom
    let (st, _) = c.req("POST", "/api/trading/unlock", Some(json!({ "otp_code": code_unlock }))).await;
    assert_eq!(st, StatusCode::OK);

    let (st, ord_kr) = c.req(
        "POST",
        "/api/trading/orders",
        Some(json!({
            "market": "KR",
            "symbol": "005930",
            "side": "BUY",
            "order_type": "LIMIT",
            "quantity": 5,
            "price": 72000,
            "dry_run": false
        })),
    ).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ord_kr["result"]["orderId"], json!("KW-KR-001"));

    let (st, ord_us) = c.req(
        "POST",
        "/api/trading/orders",
        Some(json!({
            "market": "US",
            "symbol": "AAPL",
            "side": "BUY",
            "order_type": "LIMIT",
            "quantity": 2,
            "price": 230.5,
            "dry_run": false
        })),
    ).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ord_us["result"]["orderId"], json!("KW-US-001"));

    let (st, open) = c.req("GET", "/api/trading/orders?status=OPEN", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(open["items"].as_array().unwrap().len(), 2);
    assert_eq!(open["items"][0]["orderId"], json!("KW-KR-001"));
    assert_eq!(open["items"][1]["orderId"], json!("KW-US-001"));

    let (st, canceled) = c.req("POST", "/api/trading/orders/KW-KR-001/cancel", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(canceled["result"]["status"], json!("CANCELED"));

    let (st, canceled_us) = c.req("POST", "/api/trading/orders/KW-US-001/cancel", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(canceled_us["result"]["status"], json!("CANCELED"));

    // 6. Phase 3: Kiwoom HTS Condition Search (CNSRLST / CNSRREQ)
    let (st, clist) = c.req("GET", "/api/kiwoom/conditions", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(clist["count"], json!(2));
    assert_eq!(clist["items"][0]["name"], json!("단기돌파검색"));

    let (st, cmatches) = c.req("GET", "/api/kiwoom/conditions/0/matches", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cmatches["count"], json!(2));
    assert_eq!(cmatches["items"][0]["symbol"], json!("005930"));
    assert_eq!(cmatches["items"][0]["price"], json!(72000.0));

    // Verify all TR calls used mock mode as configured and hit official KR + US TR IDs
    let calls = fake_kw.calls.lock();
    assert!(calls.iter().all(|(is_mock, _, _, _)| *is_mock));
    for tr in [
        "kt10000", "kt10003", "usa10098", "usa10099", "usa20100", "usa20101", "usa20151",
        "usa06011", "usa06012", "ust21070", "ust21110", "ust21510", "ust20000", "ust20003",
    ] {
        assert!(calls.iter().any(|(_, _, id, _)| id == tr), "missing expected TR {tr}");
    }
}
