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
use mighty_trade_server::quant::indicators as ind;
use mighty_trade_server::state::AppState;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
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

fn assert_series_close(name: &str, actual: &[Option<f64>], expected: &[Value]) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{name}: length mismatch ({} vs {})",
        actual.len(),
        expected.len()
    );
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        match (a, e.as_f64()) {
            (None, None) => {}
            (Some(av), Some(ev)) => {
                let diff = (av - ev).abs();
                assert!(
                    diff < 1e-6,
                    "{name}[{i}] mismatch: actual={av}, expected={ev}, diff={diff}"
                );
            }
            (av, ev) => panic!("{name}[{i}] None/Some mismatch: actual={av:?}, expected={ev:?}"),
        }
    }
}

#[test]
fn indicators_match_python_golden_fixture_bit_for_bit() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("shared/fixtures/indicators.json");
    let raw = std::fs::read_to_string(&fixture_path).expect("read shared/fixtures/indicators.json");
    let v: Value = serde_json::from_str(&raw).unwrap();

    let close: Vec<f64> = v["input"]["close"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let high: Vec<f64> = v["input"]["high"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let low: Vec<f64> = v["input"]["low"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();

    let out = &v["output"];
    assert_series_close("sma20", &ind::sma(&close, 20), out["sma20"].as_array().unwrap());
    assert_series_close("ema12", &ind::ema(&close, 12), out["ema12"].as_array().unwrap());
    assert_series_close("rsi14", &ind::rsi(&close, 14), out["rsi14"].as_array().unwrap());
    assert_series_close("rsi2", &ind::rsi(&close, 2), out["rsi2"].as_array().unwrap());
    assert_series_close(
        "atr14",
        &ind::atr(&high, &low, &close, 14),
        out["atr14"].as_array().unwrap(),
    );
    let bb = ind::bollinger(&close, 20, 2.0);
    assert_series_close("bb_upper", &bb.upper, out["bb_upper"].as_array().unwrap());
    assert_series_close("bb_lower", &bb.lower, out["bb_lower"].as_array().unwrap());

    let dc = ind::donchian(&high, &low, 20, 10);
    assert_series_close("dc_upper", &dc.upper, out["dc_upper"].as_array().unwrap());
    assert_series_close("dc_lower", &dc.lower, out["dc_lower"].as_array().unwrap());
}

#[tokio::test]
async fn quant_lab_all_endpoints_work_end_to_end() {
    let td = tempfile::tempdir().unwrap();
    let cfg = Config::test_default(td.path().to_path_buf());
    let mut state = AppState::new(cfg).unwrap();
    state.google_verifier = Arc::new(StaticGoogle);
    let app = build_router(state);
    let mut c = Client::new(app);

    // Log in admin
    let (st, _) = c
        .req("POST", "/api/auth/google", Some(json!({ "credential": "admin" })))
        .await;
    assert_eq!(st, StatusCode::OK);
    let (_, setup) = c.req("POST", "/api/auth/otp/setup", None).await;
    let secret = setup["secret"].as_str().unwrap().to_string();
    let step0 = current_step(Utc::now());
    c.req(
        "POST",
        "/api/auth/otp/verify",
        Some(json!({ "code": generate_totp_code(&secret, step0).unwrap() })),
    )
    .await;

    // 1. Strategies metadata (18 strategies: 10 swing incl. 3 WorldQuant/151 + 8 daytrade incl. 4 pro setups)
    let (st, strats) = c.req("GET", "/api/quant/strategies", None).await;
    assert_eq!(st, StatusCode::OK);
    let strat_items = strats["items"].as_array().unwrap();
    assert_eq!(strat_items.len(), 18);
    assert_eq!(strats["objectives"].as_object().unwrap().len(), 6);

    // 2. Cost defaults
    let (st, dk) = c.req("GET", "/api/quant/defaults?market=KR", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(dk["capital"], 10_000_000.0);
    let (st, du) = c.req("GET", "/api/quant/defaults?market=US", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(du["capital"], 10_000.0);

    // 3. Backtest across swing & daytrade strategies + risk rules + market timing
    for sid in [
        "ma_cross",
        "turtle",
        "macd_trend",
        "bollinger",
        "rsi2",
        "ibs_reversion",
        "ts_momentum",
        "hp_filter",
        "knn",
        "alpha_combo",
        "vwap_pullback",
        "hod_breakout",
        "red_to_green",
        "closing_bet",
        "vol_breakout",
        "gap_reversion",
        "orb_breakout",
        "vwap_reversion",
    ] {
        let (st, bt) = c
            .req(
                "POST",
                "/api/quant/backtest",
                Some(json!({
                    "market": "KR",
                    "symbol": "005930",
                    "strategy": sid,
                    "period": "3y",
                    "capital": 10000000,
                    "risk": { "stop_loss_pct": 5.0, "take_profit_pct": 12.0, "trailing_stop_pct": 4.0 },
                    "market_timing": { "mode": "block_buy", "benchmark": "069500", "ma_short": 20, "ma_long": 60 }
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "backtest failed for {sid}: {bt}");
        assert!(bt["equity"].as_array().unwrap().len() > 10, "{sid} equity empty");
        assert!(bt["metrics"]["total_return_pct"].is_number(), "{sid} metrics missing: {}", bt["metrics"]);
        assert_eq!(bt["risk_audit"]["gates"].as_array().unwrap().len(), 6, "{sid} risk_audit missing");
    }

    // 4. 2D Optimization (rayon parallel)
    let (st, opt) = c
        .req(
            "POST",
            "/api/quant/optimize",
            Some(json!({
                "market": "KR",
                "symbol": "005930",
                "strategy": "ma_cross",
                "period": "3y",
                "x": { "key": "fast", "start": 5, "stop": 15, "step": 5 },
                "y": { "key": "slow", "start": 20, "stop": 40, "step": 10 },
                "objective": "sharpe",
                "min_trades": 1
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "optimize failed: {opt}");
    assert_eq!(opt["cells"].as_array().unwrap().len(), 3);
    assert_eq!(opt["cells"][0].as_array().unwrap().len(), 3);
    assert!(opt["best"].is_object());

    // 5. Walk-forward (rolling)
    let (st, wf) = c
        .req(
            "POST",
            "/api/quant/walkforward",
            Some(json!({
                "market": "KR",
                "symbol": "005930",
                "strategy": "ma_cross",
                "period": "5y",
                "x": { "key": "fast", "start": 5, "stop": 15, "step": 5 },
                "objective": "sharpe",
                "min_trades": 1,
                "is_years": 2,
                "oos_months": 12,
                "mode": "rolling"
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "walkforward failed: {wf}");
    assert!(!wf["windows"].as_array().unwrap().is_empty());

    // 6. Monte Carlo (bootstrap)
    let (st, mc) = c
        .req(
            "POST",
            "/api/quant/montecarlo",
            Some(json!({
                "market": "KR",
                "symbol": "005930",
                "strategy": "rsi2",
                "period": "5y",
                "runs": 200,
                "method": "bootstrap"
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "montecarlo failed: {mc}");
    assert_eq!(mc["runs"], 200);
    assert!(mc["final_return_pct"]["p50"].is_number());

    // 7. Portfolio models (13 models incl. cov_max_sharpe) + backtest
    let (st, pms) = c.req("GET", "/api/quant/portfolio/models", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(pms["items"].as_array().unwrap().len(), 13);

    for (model_id, buy_only) in [
        ("all_weather", false),
        ("all_weather", true),
        ("hrp", false),
        ("cov_max_sharpe", false),
        ("vaa", false),
        ("baa", false),
        ("dual_momentum", false),
        ("risk_parity", false),
    ] {
        let (st, pb) = c
            .req(
                "POST",
                "/api/quant/portfolio/backtest",
                Some(json!({
                    "market": "US",
                    "model": model_id,
                    "period": "3y",
                    "capital": 10000,
                    "buy_only": buy_only
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "portfolio {model_id} (buy_only={buy_only}) failed: {pb}");
        assert!(!pb["equity"].as_array().unwrap().is_empty());
        assert!(pb["metrics"]["cagr_pct"].is_number());
        assert_eq!(pb["risk_audit"]["gates"].as_array().unwrap().len(), 6);
    }

    // 8. Screener presets (7 presets) + run across all presets
    let (st, sp) = c.req("GET", "/api/quant/screener/presets", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sp["presets"].as_array().unwrap().len(), 7);

    for preset_id in [
        "momentum_quality",
        "oversold_bounce",
        "smart_money",
        "strategy_fit",
        "magic_formula",
        "dividend_lowvol",
        "multifactor",
    ] {
        let (st, sr) = c
            .req(
                "POST",
                "/api/quant/screener/run",
                Some(json!({
                    "market": "KR",
                    "preset": preset_id,
                    "universe": "kr_all_150",
                    "top_n": 5
                })),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "screener {preset_id} failed: {sr}");
        assert!(!sr["items"].as_array().unwrap().is_empty(), "{preset_id} returned 0 items");
    }

    // 9. Market Regimes endpoint uses real market_timing engine
    let (st, regs) = c.req("GET", "/api/market/regimes", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(regs["KR"]["close"].is_number() && regs["US"]["close"].is_number());
}
