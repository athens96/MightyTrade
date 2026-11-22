use async_trait::async_trait;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use parking_lot::Mutex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::errors::{ApiError, ApiResult};

const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const JWKS_TTL_SECS: u64 = 3600;

#[derive(Debug, Clone)]
pub struct GoogleUserInfo {
    pub sub: String,
    pub email: String,
    pub email_verified: bool,
    pub name: Option<String>,
    pub picture: Option<String>,
}

#[async_trait]
pub trait GoogleVerifier: Send + Sync {
    async fn verify(&self, cfg: &Config, credential: &str) -> ApiResult<GoogleUserInfo>;
}

#[derive(Deserialize)]
struct JwkKey {
    kid: String,
    n: String,
    e: String,
}

#[derive(Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

pub struct LiveGoogleVerifier {
    client: Client,
    cache: Mutex<Option<(Instant, HashMap<String, (String, String)>)>>,
}

impl LiveGoogleVerifier {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            cache: Mutex::new(None),
        }
    }

    async fn get_key(&self, kid: &str, force_refresh: bool) -> ApiResult<Option<(String, String)>> {
        if !force_refresh {
            if let Some((exp, map)) = self.cache.lock().as_ref() {
                if *exp > Instant::now() {
                    if let Some(hit) = map.get(kid) {
                        return Ok(Some(hit.clone()));
                    }
                }
            }
        }
        let resp = self
            .client
            .get(GOOGLE_JWKS_URL)
            .send()
            .await
            .map_err(|e| ApiError::unauthorized("invalid_google_token", e.to_string()))?;
        let jwks: JwksResponse = resp
            .json()
            .await
            .map_err(|e| ApiError::unauthorized("invalid_google_token", e.to_string()))?;
        let mut map = HashMap::new();
        for k in jwks.keys {
            map.insert(k.kid, (k.n, k.e));
        }
        let found = map.get(kid).cloned();
        *self.cache.lock() = Some((Instant::now() + Duration::from_secs(JWKS_TTL_SECS), map));
        Ok(found)
    }
}

#[async_trait]
impl GoogleVerifier for LiveGoogleVerifier {
    async fn verify(&self, cfg: &Config, credential: &str) -> ApiResult<GoogleUserInfo> {
        if cfg.google_client_id.is_empty() {
            return Err(ApiError::internal(
                "google_not_configured",
                "GOOGLE_CLIENT_ID is not set",
            ));
        }
        let header = decode_header(credential)
            .map_err(|e| ApiError::unauthorized("invalid_google_token", e.to_string()))?;
        let kid = header
            .kid
            .ok_or_else(|| ApiError::unauthorized("invalid_google_token", "missing kid"))?;
        let key_pair = match self.get_key(&kid, false).await? {
            Some(k) => k,
            None => self
                .get_key(&kid, true)
                .await?
                .ok_or_else(|| ApiError::unauthorized("invalid_google_token", "unknown Google key id"))?,
        };
        let dec_key = DecodingKey::from_rsa_components(&key_pair.0, &key_pair.1)
            .map_err(|e| ApiError::unauthorized("invalid_google_token", e.to_string()))?;

        let mut val = Validation::new(Algorithm::RS256);
        val.set_audience(&[cfg.google_client_id.as_str()]);
        val.set_issuer(&["accounts.google.com", "https://accounts.google.com"]);
        val.validate_exp = true;

        let data = decode::<Value>(credential, &dec_key, &val)
            .map_err(|e| ApiError::unauthorized("invalid_google_token", e.to_string()))?;
        let claims = data.claims;

        let info = GoogleUserInfo {
            sub: claims
                .get("sub")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::unauthorized("invalid_google_token", "missing sub"))?
                .to_string(),
            email: claims
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            email_verified: claims
                .get("email_verified")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            name: claims.get("name").and_then(|v| v.as_str()).map(|s| s.to_string()),
            picture: claims
                .get("picture")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        };

        enforce_email_policy(cfg, &info)?;
        Ok(info)
    }
}

pub fn enforce_email_policy(cfg: &Config, info: &GoogleUserInfo) -> ApiResult<()> {
    if !info.email_verified {
        return Err(ApiError::code_only(403, "email_not_verified"));
    }
    if !cfg.allowed_emails.is_empty() && !cfg.allowed_emails.contains(&info.email.to_lowercase()) {
        return Err(ApiError::code_only(403, "email_not_allowed"));
    }
    Ok(())
}
