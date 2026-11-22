//! Application-layer hybrid encryption (RSA-3072 OAEP-SHA256 + AES-256-GCM),
//! 100% wire-compatible with `client/src/lib/secureEnvelope.ts` and `server/app/transport_crypto.py`.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use parking_lot::Mutex;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey, LineEnding};
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto::Crypto;
use crate::errors::{ApiError, ApiResult};
use crate::storage::Store;

pub const ALG: &str = "RSA-OAEP-256+A256GCM";
const KEYS: &str = "server_keys";
const KEY_ID: &str = "transport_rsa";
const MAX_AGE_SEC: f64 = 300.0;

#[derive(Clone)]
pub struct TransportKey {
    pub private_key: RsaPrivateKey,
    pub kid: String,
    pub spki_b64: String,
}

pub struct TransportCrypto {
    cache: Mutex<Option<TransportKey>>,
}

impl Default for TransportCrypto {
    fn default() -> Self {
        Self::new()
    }
}

impl TransportCrypto {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
        }
    }

    pub fn load_or_create(&self, store: &dyn Store, crypto: &Crypto) -> ApiResult<TransportKey> {
        {
            if let Some(hit) = self.cache.lock().clone() {
                return Ok(hit);
            }
        }
        let _guard = store.lock(KEY_ID);
        let private_key = if let Some(rec) = store.get(KEYS, KEY_ID) {
            let enc = rec
                .get("private_pem_enc")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::internal("transport_key_corrupt", "missing private_pem_enc"))?;
            let pem = crypto
                .decrypt(enc)
                .map_err(|e| ApiError::internal("transport_key_decrypt", e))?;
            RsaPrivateKey::from_pkcs8_pem(&pem)
                .map_err(|e| ApiError::internal("transport_key_parse", e.to_string()))?
        } else {
            let mut rng = rand::thread_rng();
            let key = RsaPrivateKey::new(&mut rng, 3072)
                .map_err(|e| ApiError::internal("rsa_keygen_failed", e.to_string()))?;
            let pem = key
                .to_pkcs8_pem(LineEnding::LF)
                .map_err(|e| ApiError::internal("rsa_pem_encode", e.to_string()))?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            store.put(
                KEYS,
                KEY_ID,
                json!({
                    "private_pem_enc": crypto.encrypt(&pem),
                    "created_at": now,
                }),
            );
            key
        };

        let pub_key = RsaPublicKey::from(&private_key);
        let spki_doc = pub_key
            .to_public_key_der()
            .map_err(|e| ApiError::internal("spki_encode", e.to_string()))?;
        let spki_bytes = spki_doc.as_bytes();
        let digest = Sha256::digest(spki_bytes);
        let kid = hex_lower(&digest)[..16].to_string();
        let spki_b64 = STANDARD.encode(spki_bytes);

        let tk = TransportKey {
            private_key,
            kid,
            spki_b64,
        };
        *self.cache.lock() = Some(tk.clone());
        Ok(tk)
    }

    pub fn public_key_info(&self, store: &dyn Store, crypto: &Crypto) -> ApiResult<Value> {
        let tk = self.load_or_create(store, crypto)?;
        Ok(json!({
            "kid": tk.kid,
            "alg": ALG,
            "spki": tk.spki_b64,
        }))
    }

    pub fn open_envelope(
        &self,
        store: &dyn Store,
        crypto: &Crypto,
        envelope: Option<&Value>,
        aad: &str,
    ) -> ApiResult<Value> {
        let env = envelope
            .and_then(|v| v.as_object())
            .ok_or_else(|| ApiError::bad_request("invalid_envelope", "unsupported or missing envelope"))?;
        if env.get("alg").and_then(|v| v.as_str()) != Some(ALG) {
            return Err(ApiError::bad_request(
                "invalid_envelope",
                "unsupported or missing envelope",
            ));
        }
        let tk = self.load_or_create(store, crypto)?;
        if env.get("kid").and_then(|v| v.as_str()) != Some(tk.kid.as_str()) {
            return Err(ApiError::bad_request(
                "stale_transport_key",
                "public key changed, please retry",
            ));
        }

        let ek_raw = b64d(env.get("ek"))?;
        let iv_raw = b64d(env.get("iv"))?;
        let ct_raw = b64d(env.get("ct"))?;
        if iv_raw.len() != 12 {
            return Err(ApiError::bad_request("invalid_envelope", "could not decrypt payload"));
        }

        let padding = Oaep::new::<Sha256>();
        let aes_key = tk
            .private_key
            .decrypt(padding, &ek_raw)
            .map_err(|_| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))?;
        let cipher = Aes256Gcm::new_from_slice(&aes_key)
            .map_err(|_| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))?;
        let nonce = Nonce::from_slice(&iv_raw);
        let plain = cipher
            .decrypt(
                nonce,
                Payload {
                    msg: &ct_raw,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))?;

        let payload: Value = serde_json::from_slice(&plain)
            .map_err(|_| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))?;
        if !payload.is_object() {
            return Err(ApiError::bad_request("invalid_envelope", "payload must be an object"));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let iat = payload.get("iat").and_then(|v| v.as_f64()).ok_or_else(|| {
            ApiError::bad_request("envelope_expired", "payload is too old or clock is off")
        })?;
        if (now - iat).abs() > MAX_AGE_SEC {
            return Err(ApiError::bad_request(
                "envelope_expired",
                "payload is too old or clock is off",
            ));
        }
        Ok(payload)
    }
}

fn b64d(val: Option<&Value>) -> ApiResult<Vec<u8>> {
    let s = val
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))?;
    STANDARD
        .decode(s)
        .map_err(|_| ApiError::bad_request("invalid_envelope", "could not decrypt payload"))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}
