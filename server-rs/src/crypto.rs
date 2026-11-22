//! Fernet encryption-at-rest compatible 1:1 with Python `cryptography.fernet.Fernet`.
//!
//! Wire format (before URL-safe base64 with `=` padding):
//! `version (0x80, 1B) || timestamp (u64 BE, 8B) || iv (16B) || ciphertext (AES-128-CBC + PKCS7) || hmac-sha256 (32B)`

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;
type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;
type HmacSha256 = Hmac<Sha256>;

const FERNET_VERSION: u8 = 0x80;

#[derive(Clone)]
pub struct Crypto {
    signing_key: [u8; 16],
    encryption_key: [u8; 16],
}

impl Crypto {
    pub fn from_config(cfg: &Config) -> Result<Self, String> {
        let key_bytes: Vec<u8> = if !cfg.encryption_key.is_empty() {
            URL_SAFE
                .decode(cfg.encryption_key.trim())
                .map_err(|e| format!("Invalid ENCRYPTION_KEY base64: {e}"))?
        } else {
            // Dev fallback: SHA-256(SECRET_KEY) matches server/app/crypto.py exactly
            Sha256::digest(cfg.secret_key.as_bytes()).to_vec()
        };
        if key_bytes.len() != 32 {
            return Err(format!(
                "Fernet key must be 32 url-safe base64-decoded bytes, got {}",
                key_bytes.len()
            ));
        }
        let mut signing_key = [0u8; 16];
        let mut encryption_key = [0u8; 16];
        signing_key.copy_from_slice(&key_bytes[0..16]);
        encryption_key.copy_from_slice(&key_bytes[16..32]);
        Ok(Self {
            signing_key,
            encryption_key,
        })
    }

    pub fn encrypt(&self, plain: &str) -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut iv = [0u8; 16];
        rand::thread_rng().fill_bytes(&mut iv);

        let pt = plain.as_bytes();
        let mut buf = vec![0u8; pt.len() + 16];
        buf[..pt.len()].copy_from_slice(pt);
        let ct = Aes128CbcEnc::new(&self.encryption_key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, pt.len())
            .expect("buffer sized for PKCS7 padding");

        let mut payload = Vec::with_capacity(1 + 8 + 16 + ct.len() + 32);
        payload.push(FERNET_VERSION);
        payload.extend_from_slice(&ts.to_be_bytes());
        payload.extend_from_slice(&iv);
        payload.extend_from_slice(ct);

        let mut mac = HmacSha256::new_from_slice(&self.signing_key).expect("HMAC accepts 16B key");
        mac.update(&payload);
        let tag = mac.finalize().into_bytes();
        payload.extend_from_slice(&tag);

        URL_SAFE.encode(payload)
    }

    pub fn decrypt(&self, token: &str) -> Result<String, String> {
        let raw = URL_SAFE
            .decode(token.trim())
            .map_err(|e| format!("fernet base64 error: {e}"))?;
        // Minimum: 1 (ver) + 8 (ts) + 16 (iv) + 16 (min 1 AES block) + 32 (hmac) = 73 bytes
        if raw.len() < 73 {
            return Err("fernet token too short".to_string());
        }
        if raw[0] != FERNET_VERSION {
            return Err("invalid fernet version".to_string());
        }
        let body_len = raw.len() - 32;
        let body = &raw[..body_len];
        let tag = &raw[body_len..];

        let mut mac = HmacSha256::new_from_slice(&self.signing_key).expect("HMAC accepts 16B key");
        mac.update(body);
        mac.verify_slice(tag)
            .map_err(|_| "fernet HMAC verification failed".to_string())?;

        let iv: [u8; 16] = body[9..25].try_into().unwrap();
        let mut ct = body[25..].to_vec();
        if ct.len() % 16 != 0 {
            return Err("fernet ciphertext not block-aligned".to_string());
        }
        let pt = Aes128CbcDec::new(&self.encryption_key.into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut ct)
            .map_err(|_| "fernet padding/decryption error".to_string())?;
        String::from_utf8(pt.to_vec()).map_err(|e| format!("fernet utf-8 error: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn roundtrip_fernet() {
        let cfg = Config::test_default(PathBuf::from("/tmp"));
        let c = Crypto::from_config(&cfg).unwrap();
        let tok = c.encrypt("hello-마이티트레이드-secret-12345");
        assert_eq!(c.decrypt(&tok).unwrap(), "hello-마이티트레이드-secret-12345");
    }

    #[test]
    fn tamper_fails() {
        let cfg = Config::test_default(PathBuf::from("/tmp"));
        let c = Crypto::from_config(&cfg).unwrap();
        let mut raw = URL_SAFE.decode(c.encrypt("secret")).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        assert!(c.decrypt(&URL_SAFE.encode(raw)).is_err());
    }

    #[test]
    fn decrypts_python_cryptography_fernet_token() {
        let cfg = Config::test_default(PathBuf::from("/tmp"));
        let c = Crypto::from_config(&cfg).unwrap();
        let py_tok = "gAAAAABrAu_c50gYenaAvEDIPrycBmoSKI0IhV413Ms1xeeQRv67OKfWNry_o91d8HMgUtle_uxoqnJ3maPAN1Y-H3XD5_IEEPRdBP7_nZ-bU4srLLLWCxk=";
        assert_eq!(c.decrypt(py_tok).unwrap(), "toss-api-secret-한글-123");
    }
}
