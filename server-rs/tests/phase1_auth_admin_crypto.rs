use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use http_body_util::BodyExt;
use mighty_trade_server::auth::google::{enforce_email_policy, GoogleUserInfo, GoogleVerifier};
use mighty_trade_server::auth::otp::{current_step, generate_totp_code};
use mighty_trade_server::build_router;
use mighty_trade_server::config::Config;
use mighty_trade_server::errors::ApiResult;
use mighty_trade_server::state::{AppState, NotifierHook};
use parking_lot::Mutex;
use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Oaep, RsaPublicKey};
use serde_json::{json, Value};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

struct FakeGoogle {
    users: Mutex<HashMap<String, GoogleUserInfo>>,
}

#[async_trait]
impl GoogleVerifier for FakeGoogle {
    async fn verify(&self, cfg: &Config, credential: &str) -> ApiResult<GoogleUserInfo> {
        let info = self
            .users
            .lock()
            .get(credential)
            .cloned()
            .expect("registered fake credential");
        enforce_email_policy(cfg, &info)?;
        Ok(info)
    }
}

struct RecordingNotifier {
    calls: Mutex<Vec<(String, Value)>>,
}

#[async_trait]
impl NotifierHook for RecordingNotifier {
    async fn send_http(&self, url: &str, payload: &Value) -> Result<(), String> {
        self.calls.lock().push((url.to_string(), payload.clone()));
        Ok(())
    }
}

struct TestClient {
    app: axum::Router,
    cookies: HashMap<String, String>,
}

impl TestClient {
    fn new(app: axum::Router) -> Self {
        Self {
            app,
            cookies: HashMap::new(),
        }
    }

    async fn req(&mut self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if !self.cookies.is_empty() {
            let cookie_hdr = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            builder = builder.header(header::COOKIE, cookie_hdr);
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
        let status = resp.status();
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
        let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, val)
    }

    async fn login_full(&mut self, cred: &str) -> String {
        let (st, r) = self
            .req("POST", "/api/auth/google", Some(json!({ "credential": cred })))
            .await;
        assert_eq!(st, StatusCode::OK, "google login failed: {r}");
        assert_eq!(r["next"], "otp_setup");
        let (st, s) = self.req("POST", "/api/auth/otp/setup", None).await;
        assert_eq!(st, StatusCode::OK);
        let secret = s["secret"].as_str().unwrap().to_string();
        let code = generate_totp_code(&secret, current_step(Utc::now())).unwrap();
        let (st, v) = self
            .req("POST", "/api/auth/otp/verify", Some(json!({ "code": code })))
            .await;
        assert_eq!(st, StatusCode::OK, "otp verify failed: {v}");
        secret
    }
}

fn setup_state() -> (tempfile::TempDir, AppState, Arc<FakeGoogle>, Arc<RecordingNotifier>) {
    let td = tempfile::tempdir().unwrap();
    let cfg = Config::test_default(td.path().to_path_buf());
    let mut state = AppState::new(cfg).unwrap();
    let fg = Arc::new(FakeGoogle {
        users: Mutex::new(HashMap::new()),
    });
    fg.users.lock().insert(
        "admin-cred".to_string(),
        GoogleUserInfo {
            sub: "adm".to_string(),
            email: "Admin@Example.com".to_string(),
            email_verified: true,
            name: Some("운영자".to_string()),
            picture: None,
        },
    );
    fg.users.lock().insert(
        "alice-cred".to_string(),
        GoogleUserInfo {
            sub: "alice".to_string(),
            email: "alice@example.com".to_string(),
            email_verified: true,
            name: Some("앨리스".to_string()),
            picture: None,
        },
    );
    fg.users.lock().insert(
        "bob-cred".to_string(),
        GoogleUserInfo {
            sub: "bob".to_string(),
            email: "bob@example.com".to_string(),
            email_verified: true,
            name: Some("밥".to_string()),
            picture: None,
        },
    );
    let notifier = Arc::new(RecordingNotifier {
        calls: Mutex::new(Vec::new()),
    });
    state.google_verifier = fg.clone();
    state.notifier_hook = notifier.clone();
    (td, state, fg, notifier)
}

#[tokio::test]
async fn health_auth_admin_and_requests_isolation() {
    let (_td, state, _fg, notifier) = setup_state();
    let app = build_router(state.clone());

    // 1. Health check
    let mut anon = TestClient::new(app.clone());
    let (st, h) = anon.req("GET", "/api/health", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h["status"], "ok");
    assert!(h["version"].as_str().is_some());

    // 2. Admin auto-approved login + OTP enrollment
    let mut admin = TestClient::new(app.clone());
    let admin_secret = admin.login_full("admin-cred").await;

    // 3. New signup waits for approval
    let mut alice = TestClient::new(app.clone());
    let (st, r) = alice
        .req("POST", "/api/auth/google", Some(json!({ "credential": "alice-cred" })))
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(r["next"], "approval_pending");
    let alice_id = r["user"]["id"].as_str().unwrap().to_string();
    assert_eq!(alice.req("GET", "/api/auth/me", None).await.0, StatusCode::UNAUTHORIZED);

    // 4. Admin sees pending summary & approves Alice and Bob
    let (st, sum) = admin.req("GET", "/api/admin/summary", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sum, json!({ "pending": 1, "open_requests": 0 }));
    assert_eq!(
        admin
            .req("POST", &format!("/api/admin/users/{alice_id}/approve"), None)
            .await
            .0,
        StatusCode::OK
    );

    let mut bob = TestClient::new(app.clone());
    let (_, br) = bob
        .req("POST", "/api/auth/google", Some(json!({ "credential": "bob-cred" })))
        .await;
    let bob_id = br["user"]["id"].as_str().unwrap().to_string();
    admin
        .req("POST", &format!("/api/admin/users/{bob_id}/approve"), None)
        .await;

    alice.login_full("alice-cred").await;
    bob.login_full("bob-cred").await;

    // 5. Configure Slack webhooks for BOTH Admin and Bob; only Admin must get request alerts
    admin
        .req(
            "PUT",
            "/api/users/me/notifications",
            Some(json!({ "enabled": true, "slack_webhook_url": "https://hooks.slack.com/services/ADMIN/HOOK" })),
        )
        .await;
    bob.req(
        "PUT",
        "/api/users/me/notifications",
        Some(json!({ "enabled": true, "slack_webhook_url": "https://hooks.slack.com/services/BOB/HOOK" })),
    )
    .await;

    // SSRF guard on private IP webhook
    let (st, err) = bob
        .req(
            "PUT",
            "/api/users/me/notifications",
            Some(json!({ "slack_webhook_url": "https://127.0.0.1/hook" })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(err["error"], "invalid_webhook_url");

    // 6. Alice submits a request
    let (st, created) = alice
        .req(
            "POST",
            "/api/users/me/requests",
            Some(json!({
                "category": "strategy",
                "title": "일목균형표 전략 요청",
                "content": "자동매매 봇에서도 일목균형표를 지원해 주세요!",
                "page": "/quant",
                "app_version": "0.2.0"
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let req_id = created["item"]["id"].as_str().unwrap().to_string();
    assert!(created["item"].get("replied_by").is_none());

    // Only admin webhook notified
    {
        let calls = notifier.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "https://hooks.slack.com/services/ADMIN/HOOK");
        assert!(calls[0].1["text"].as_str().unwrap().contains("일목균형표 전략 요청"));
    }

    // Strict isolation: Bob sees empty /me/requests and gets 403 on every /admin endpoint
    assert_eq!(bob.req("GET", "/api/users/me/requests", None).await.1, json!({ "items": [] }));
    assert_eq!(bob.req("GET", "/api/admin/requests", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        bob.req("PATCH", &format!("/api/admin/requests/{req_id}"), Some(json!({"status":"resolved"}))).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        alice.req("DELETE", &format!("/api/admin/requests/{req_id}"), None).await.0,
        StatusCode::FORBIDDEN
    );

    // Admin sees open_requests=1, replies, and resolves
    assert_eq!(
        admin.req("GET", "/api/admin/summary", None).await.1,
        json!({ "pending": 0, "open_requests": 1 })
    );
    let (st, patched) = admin
        .req(
            "PATCH",
            &format!("/api/admin/requests/{req_id}"),
            Some(json!({ "status": "resolved", "admin_reply": "반영 완료했습니다!" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(patched["item"]["replied_by"], "Admin@Example.com");
    assert_eq!(
        admin.req("GET", "/api/admin/summary", None).await.1,
        json!({ "pending": 0, "open_requests": 0 })
    );

    // Alice sees reply without leaking admin email
    let (_, mine) = alice.req("GET", "/api/users/me/requests", None).await;
    assert_eq!(mine["items"][0]["admin_reply"], "반영 완료했습니다!");
    assert!(mine["items"][0].get("replied_by").is_none());

    // 7. Transport RSA-OAEP-256 + AES-256-GCM envelope roundtrip test
    let (st, tkey) = alice.req("GET", "/api/users/transport-key", None).await;
    assert_eq!(st, StatusCode::OK);
    let spki_der = STANDARD.decode(tkey["spki"].as_str().unwrap()).unwrap();
    let pub_key = RsaPublicKey::from_public_key_der(&spki_der).unwrap();
    let mut aes_key = [0u8; 32];
    let mut iv = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut aes_key);
    rand::thread_rng().fill_bytes(&mut iv);
    let ek = pub_key
        .encrypt(&mut rand::thread_rng(), Oaep::new::<Sha256>(), &aes_key)
        .unwrap();
    let plain = json!({ "hello": "world", "iat": Utc::now().timestamp() }).to_string();
    let ct = Aes256Gcm::new_from_slice(&aes_key)
        .unwrap()
        .encrypt(
            Nonce::from_slice(&iv),
            Payload {
                msg: plain.as_bytes(),
                aad: alice_id.as_bytes(),
            },
        )
        .unwrap();
    let env = json!({
        "kid": tkey["kid"],
        "alg": tkey["alg"],
        "ek": STANDARD.encode(ek),
        "iv": STANDARD.encode(iv),
        "ct": STANDARD.encode(ct),
    });
    let opened = state
        .transport
        .open_envelope(state.store.as_ref(), &state.crypto, Some(&env), &alice_id)
        .unwrap();
    assert_eq!(opened["hello"], "world");
    // Replaying into Bob's AAD fails
    assert!(state
        .transport
        .open_envelope(state.store.as_ref(), &state.crypto, Some(&env), &bob_id)
        .is_err());

    // 8. Admin deletes Bob with next-step OTP and purges records
    let next_code = generate_totp_code(&admin_secret, current_step(Utc::now()) + 1).unwrap();
    let (st, del) = admin
        .req(
            "DELETE",
            &format!("/api/admin/users/{bob_id}"),
            Some(json!({ "otp_code": next_code })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "delete failed: {del}");
    assert_eq!(del["removed"]["users"], 1);
}
