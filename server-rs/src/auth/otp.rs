//! RFC 6238 TOTP (SHA-1, 6 digits, 30s period, ±1 step window, monotonic replay protection, lockout).
//! 100% compatible with `pyotp` and existing Google Authenticator enrollments.

use chrono::{DateTime, Duration, Utc};
use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use rand::RngCore;
use serde_json::{json, Value};
use sha1::Sha1;
use subtle::ConstantTimeEq;

use crate::errors::{ApiError, ApiResult};
use crate::state::AppState;
use crate::users::repository as users;

type HmacSha1 = Hmac<Sha1>;

pub fn generate_totp_code(secret_b32: &str, step: i64) -> Option<String> {
    let clean = secret_b32.trim().replace('=', "").to_uppercase();
    let key = BASE32_NOPAD.decode(clean.as_bytes()).ok()?;
    let mut mac = HmacSha1::new_from_slice(&key).ok()?;
    mac.update(&(step as u64).to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[19] & 0x0f) as usize;
    let bin = ((hash[offset] as u32 & 0x7f) << 24)
        | ((hash[offset + 1] as u32) << 16)
        | ((hash[offset + 2] as u32) << 8)
        | (hash[offset + 3] as u32);
    let code = bin % 1_000_000;
    Some(format!("{code:06}"))
}

pub fn current_step(now: DateTime<Utc>) -> i64 {
    now.timestamp().div_euclid(30)
}

pub fn random_base32_secret() -> String {
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    BASE32_NOPAD.encode(&bytes)
}

pub fn begin_setup(state: &AppState, user: &Value) -> ApiResult<Value> {
    if user.get("otp_enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(ApiError::code_only(409, "otp_already_enabled"));
    }
    let uid = user["id"].as_str().unwrap_or("");
    let secret = random_base32_secret();
    let enc = state.crypto.encrypt(&secret);
    users::update_user(state, uid, json!({ "otp_pending_secret_enc": enc }))?;

    let issuer = &state.config.otp_issuer;
    let account = user
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(uid);
    let label = format!("{}:{}", urlencoding::encode(issuer), urlencoding::encode(account));
    let uri = format!(
        "otpauth://totp/{label}?secret={secret}&issuer={}",
        urlencoding::encode(issuer)
    );
    Ok(json!({ "otpauth_url": uri, "secret": secret }))
}

fn matched_step(secret: &str, code: &str, now: DateTime<Utc>) -> Option<i64> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let cur = current_step(now);
    for step in [cur, cur - 1, cur + 1] {
        if let Some(expected) = generate_totp_code(secret, step) {
            if bool::from(expected.as_bytes().ct_eq(code.as_bytes())) {
                return Some(step);
            }
        }
    }
    None
}

pub fn verify(state: &AppState, user: &Value, raw_code: &str) -> ApiResult<Value> {
    let now = Utc::now();
    let uid = user["id"].as_str().unwrap_or("");
    if let Some(locked_str) = user.get("otp_locked_until").and_then(|v| v.as_str()) {
        if let Ok(locked_dt) = DateTime::parse_from_rfc3339(locked_str) {
            if locked_dt.with_timezone(&Utc) > now {
                return Err(ApiError::too_many_requests(
                    "otp_locked",
                    format!("Too many attempts. Try again after {locked_str}"),
                ));
            }
        }
    }

    let otp_enabled = user.get("otp_enabled").and_then(|v| v.as_bool()).unwrap_or(false);
    let enc = if otp_enabled {
        user.get("otp_secret_enc").and_then(|v| v.as_str())
    } else {
        user.get("otp_pending_secret_enc").and_then(|v| v.as_str())
    };
    let enc = enc.ok_or_else(|| {
        ApiError::bad_request("otp_not_setup", "OTP가 아직 등록되지 않았습니다")
    })?;

    let secret = state
        .crypto
        .decrypt(enc)
        .map_err(|_| ApiError::internal("otp_decrypt_failed", "OTP 시크릿 복호화 실패"))?;

    let code: String = raw_code.chars().filter(|c| !c.is_whitespace()).collect();
    let mut step = matched_step(&secret, &code, now);

    let last_step = user
        .get("otp_last_step")
        .and_then(|v| v.as_i64())
        .unwrap_or(-1);
    if let Some(s) = step {
        if s <= last_step {
            step = None;
        }
    }

    let Some(valid_step) = step else {
        let fails = user.get("otp_fail_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize + 1;
        let patch = if fails >= state.config.otp_max_fails {
            let until = (now + Duration::minutes(state.config.otp_lock_min)).to_rfc3339();
            json!({ "otp_fail_count": 0, "otp_locked_until": until })
        } else {
            json!({ "otp_fail_count": fails })
        };
        let _ = users::update_user(state, uid, patch);
        return Err(ApiError::unauthorized(
            "invalid_otp",
            "OTP 코드가 올바르지 않거나 이미 사용된 번호입니다",
        ));
    };

    let mut patch = json!({
        "otp_fail_count": 0,
        "otp_locked_until": Value::Null,
        "otp_last_step": valid_step,
    });
    if !otp_enabled {
        let obj = patch.as_object_mut().unwrap();
        obj.insert("otp_enabled".to_string(), Value::Bool(true));
        obj.insert("otp_secret_enc".to_string(), Value::String(enc.to_string()));
        obj.insert("otp_pending_secret_enc".to_string(), Value::Null);
    }
    users::update_user(state, uid, patch)
}
