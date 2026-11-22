//! Two-stage session tokens stored in httpOnly SameSite=Strict cookies (`mt_pending` / `mt_session`).

use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use chrono::{Duration, Utc};
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Pending,
    Session,
}

impl Stage {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stage::Pending => "pending",
            Stage::Session => "session",
        }
    }

    pub fn cookie_name(&self) -> &'static str {
        match self {
            Stage::Pending => "mt_pending",
            Stage::Session => "mt_session",
        }
    }

    pub fn cookie_path(&self) -> &'static str {
        match self {
            Stage::Pending => "/api/auth",
            Stage::Session => "/api",
        }
    }

    pub fn ttl_secs(&self, cfg: &Config) -> i64 {
        let mins = match self {
            Stage::Pending => cfg.pending_token_ttl_min,
            Stage::Session => cfg.access_token_ttl_min,
        };
        mins * 60
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: String,
    stage: String,
    iat: i64,
    exp: i64,
}

pub fn issue(jar: CookieJar, cfg: &Config, user_id: &str, stage: Stage) -> CookieJar {
    let now = Utc::now();
    let ttl_secs = stage.ttl_secs(cfg);
    let claims = Claims {
        sub: user_id.to_string(),
        stage: stage.as_str().to_string(),
        iat: now.timestamp(),
        exp: (now + Duration::seconds(ttl_secs)).timestamp(),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(cfg.secret_key.as_bytes()),
    )
    .expect("HS256 JWT encode");

    let mut c = Cookie::new(stage.cookie_name(), token);
    c.set_http_only(true);
    c.set_secure(cfg.cookie_secure);
    c.set_same_site(SameSite::Strict);
    c.set_path(stage.cookie_path());
    c.set_max_age(time::Duration::seconds(ttl_secs));
    jar.add(c)
}

pub fn clear(jar: CookieJar, stage: Stage) -> CookieJar {
    let mut c = Cookie::new(stage.cookie_name(), "");
    c.set_path(stage.cookie_path());
    c.set_max_age(time::Duration::ZERO);
    jar.add(c)
}

pub fn read(jar: &CookieJar, cfg: &Config, stage: Stage) -> Option<String> {
    let cookie = jar.get(stage.cookie_name())?;
    let token = cookie.value();
    if token.is_empty() {
        return None;
    }
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(cfg.secret_key.as_bytes()),
        &validation,
    )
    .ok()?;
    if data.claims.stage != stage.as_str() {
        return None;
    }
    Some(data.claims.sub)
}
