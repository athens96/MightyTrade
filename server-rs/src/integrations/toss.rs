//! Toss Securities Open API client (shared encrypted token cache + lock + 401 refresh + 429 retry).

use async_trait::async_trait;
use reqwest::{Client, Method};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex as AsyncMutex;

use crate::crypto::Crypto;
use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::storage::Store;

pub const TOKENS: &str = "integration_tokens";
const TOKEN_MARGIN_SECS: f64 = 300.0;
const TOKEN_ERRORS: [&str; 3] = ["invalid-token", "expired-token", "token-revoked"];
const MAX_RETRY_AFTER_SECS: f64 = 3.0;

#[async_trait]
pub trait TossHttpOverride: Send + Sync {
    async fn request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        json_body: Option<&Value>,
        account_seq: Option<i64>,
    ) -> ApiResult<Option<Value>>;
}

pub fn token_id(client_id: &str) -> String {
    let digest = Sha256::digest(client_id.as_bytes());
    let mut hex = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{b:02x}");
    }
    format!("toss_{hex}")
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

pub struct TossClient {
    base_url: String,
    client_id: String,
    client_secret: String,
    store: Arc<dyn Store>,
    crypto: Arc<Crypto>,
    http: Client,
    tok_id: String,
    mem: AsyncMutex<(Option<String>, f64)>,
    override_backend: Option<Arc<dyn TossHttpOverride>>,
}

impl TossClient {
    pub fn new(
        base_url: String,
        client_id: String,
        client_secret: String,
        store: Arc<dyn Store>,
        crypto: Arc<Crypto>,
        http: Client,
        override_backend: Option<Arc<dyn TossHttpOverride>>,
    ) -> ApiResult<Self> {
        if client_id.is_empty() || client_secret.is_empty() {
            return Err(ApiError::internal(
                "toss_not_configured",
                "TOSS_CLIENT_ID / TOSS_CLIENT_SECRET are not set",
            ));
        }
        let tok_id = token_id(&client_id);
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client_id,
            client_secret,
            store,
            crypto,
            http,
            tok_id,
            mem: AsyncMutex::new((None, 0.0)),
            override_backend,
        })
    }

    async fn issue_token(&self) -> ApiResult<(String, f64)> {
        let form = [
            ("grant_type", "client_credentials"),
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
        ];
        let resp = self
            .http
            .post(format!("{}/oauth2/token", self.base_url))
            .form(&form)
            .send()
            .await
            .map_err(|e| ApiError::bad_gateway("toss_unreachable", e.to_string()))?;
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or_else(|_| json!({}));
        if status != 200 {
            let code = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("token_error");
            let desc = body
                .get("error_description")
                .and_then(|v| v.as_str())
                .unwrap_or("Toss token issue failed");
            return Err(ApiError::bad_gateway(format!("toss_auth_{code}"), desc));
        }
        let access_token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::bad_gateway("toss_auth_token_error", "missing access_token"))?
            .to_string();
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_f64())
            .unwrap_or(3600.0);
        Ok((access_token, now_secs() + expires_in))
    }

    fn load_shared(&self) -> (Option<String>, f64) {
        let Some(rec) = self.store.get(TOKENS, &self.tok_id) else {
            return (None, 0.0);
        };
        let enc = rec.get("token_enc").and_then(|v| v.as_str());
        let exp = rec.get("expires_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        match enc.and_then(|s| self.crypto.decrypt(s).ok()) {
            Some(tok) => (Some(tok), exp),
            None => (None, 0.0),
        }
    }

    async fn get_token(&self, force_refresh: bool, rejected: Option<&str>) -> ApiResult<String> {
        let mut guard = self.mem.lock().await;
        if !force_refresh {
            if let Some(ref tok) = guard.0 {
                if now_secs() < guard.1 - TOKEN_MARGIN_SECS {
                    return Ok(tok.clone());
                }
            }
        }
        let _flock = self.store.lock(&self.tok_id);
        let (shared_tok, shared_exp) = self.load_shared();
        let usable = shared_tok.is_some() && now_secs() < shared_exp - TOKEN_MARGIN_SECS;
        let same_rejected = force_refresh && shared_tok.as_deref() == rejected;
        let (tok, exp) = if !usable || same_rejected {
            let (new_tok, new_exp) = self.issue_token().await?;
            self.store.put(
                TOKENS,
                &self.tok_id,
                json!({
                    "token_enc": self.crypto.encrypt(&new_tok),
                    "expires_at": new_exp,
                }),
            );
            (new_tok, new_exp)
        } else {
            (shared_tok.unwrap(), shared_exp)
        };
        *guard = (Some(tok.clone()), exp);
        Ok(tok)
    }

    pub async fn access_token(&self, rejected: Option<&str>) -> ApiResult<String> {
        if self.override_backend.is_some() {
            return Ok("fake-toss-token".to_string());
        }
        self.get_token(rejected.is_some(), rejected).await
    }

    pub async fn request(
        &self,
        method: &str,
        path: &str,
        params: Option<&HashMap<String, String>>,
        json_body: Option<&Value>,
        account_seq: Option<i64>,
    ) -> ApiResult<Option<Value>> {
        if let Some(ref mock) = self.override_backend {
            return mock.request(method, path, params, json_body, account_seq).await;
        }
        let mut token = self.get_token(false, None).await?;
        let http_method = Method::from_bytes(method.as_bytes())
            .map_err(|_| ApiError::internal("invalid_method", method))?;

        for attempt in 0..3 {
            let mut req = self
                .http
                .request(http_method.clone(), format!("{}{}", self.base_url, path))
                .bearer_auth(&token);
            if let Some(seq) = account_seq {
                req = req.header("X-Tossinvest-Account", seq.to_string());
            }
            if let Some(p) = params {
                req = req.query(p);
            }
            if let Some(j) = json_body {
                req = req.json(j);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| ApiError::bad_gateway("toss_unreachable", e.to_string()))?;
            let status = resp.status().as_u16();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<f64>().ok());
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| ApiError::bad_gateway("toss_unreachable", e.to_string()))?;

            if status < 300 {
                if bytes.is_empty() {
                    return Ok(None);
                }
                let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                return Ok(match parsed {
                    Value::Object(mut m) => m.remove("result"),
                    other => Some(other),
                });
            }

            let parsed: Value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
            let err = parsed.get("error").cloned().unwrap_or_else(|| json!({}));
            let code = err
                .get("code")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| err.as_str().unwrap_or("").to_string());
            let message = err
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if status == 401 && TOKEN_ERRORS.contains(&code.as_str()) && attempt == 0 {
                token = self.get_token(true, Some(&token)).await?;
                continue;
            }
            if status == 429 && attempt < 2 {
                let wait = retry_after.unwrap_or(1.0).min(MAX_RETRY_AFTER_SECS);
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                continue;
            }
            return Err(map_error(status, &code, &message));
        }
        Err(ApiError::bad_gateway("toss_retry_exhausted", "toss_retry_exhausted"))
    }
}

fn map_error(status: u16, code: &str, message: &str) -> ApiError {
    let code = if code.is_empty() { "unknown" } else { code };
    if status == 401 || status == 403 {
        let msg = if message.is_empty() {
            "Toss authentication failed"
        } else {
            message
        };
        return ApiError::bad_gateway(format!("toss_{code}"), msg);
    }
    if status == 429 {
        return ApiError::too_many_requests("toss_rate_limited", message);
    }
    if status >= 500 {
        let msg = if message.is_empty() {
            "Toss server error"
        } else {
            message
        };
        return ApiError::bad_gateway(format!("toss_{code}"), msg);
    }
    ApiError::new(status, format!("toss_{code}"), message)
}

pub async fn verify_credentials(
    state: &AppState,
    client_id: &str,
    client_secret: &str,
) -> ApiResult<Option<i64>> {
    let client = TossClient::new(
        state.config.toss_api_base_url.clone(),
        client_id.to_string(),
        client_secret.to_string(),
        state.store.clone(),
        state.crypto.clone(),
        state.http.clone(),
        state.toss_override.clone(),
    )?;
    let accounts_val = match client.request("GET", "/api/v1/accounts", None, None, None).await {
        Ok(v) => v.unwrap_or_else(|| json!([])),
        Err(e) => {
            let friendly = match e.code.as_str() {
                "toss_auth_invalid_client" => Some("client_id 또는 client_secret이 올바르지 않습니다."),
                "toss_auth_access_denied" | "toss_ip-not-allowed" => {
                    Some("토스증권 허용 IP에 이 서버의 IP가 등록되어 있지 않습니다.")
                }
                c if c.starts_with("toss_auth_") => Some(e.message.as_str()),
                _ => None,
            };
            if let Some(msg) = friendly {
                return Err(ApiError::bad_request("toss_verify_failed", msg));
            }
            return Err(e);
        }
    };
    let arr = accounts_val.as_array().cloned().unwrap_or_default();
    let picked = arr
        .iter()
        .find(|a| a.get("accountType").and_then(|v| v.as_str()) == Some("BROKERAGE"))
        .or_else(|| arr.first());
    Ok(picked.and_then(|a| a.get("accountSeq")).and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    }))
}
