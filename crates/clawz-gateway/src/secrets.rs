//! Optional symmetric encryption for API keys at rest (`CLAWZ_SECRETS_KEY`).

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use base64::Engine;
use sha2::{Digest, Sha256};

const PLAIN_PREFIX: &str = "plain:";
const ENC_PREFIX: &str = "enc:";

fn derive_key() -> [u8; 32] {
    let secret = std::env::var("CLAWZ_SECRETS_KEY").unwrap_or_else(|_| "dev-insecure-key".into());
    Sha256::digest(secret.as_bytes()).into()
}

/// Seal a secret for database storage. Without `CLAWZ_SECRETS_KEY`, stores `plain:` prefix (dev only).
pub fn seal_secret(plaintext: &str) -> String {
    if std::env::var("CLAWZ_SECRETS_KEY").is_err() {
        return format!("{PLAIN_PREFIX}{plaintext}");
    }
    let cipher = Aes256Gcm::new_from_slice(&derive_key()).expect("valid key length");
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .expect("encryption");
    let mut payload = nonce.to_vec();
    payload.extend(ciphertext);
    format!(
        "{ENC_PREFIX}{}",
        base64::engine::general_purpose::STANDARD.encode(payload)
    )
}

fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("api_key")
        || k.contains("apikey")
        || k.ends_with("_key")
        || k.contains("secret")
        || k.contains("token")
        || k.contains("password")
        || k == "authorization"
}

/// Seal string values under sensitive keys in a JSON config blob.
pub fn seal_sensitive_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_sensitive_key(k) {
                    if let Some(s) = v.as_str() {
                        out.insert(k.clone(), serde_json::Value::String(seal_secret(s)));
                    } else {
                        out.insert(k.clone(), seal_sensitive_json(v));
                    }
                } else {
                    out.insert(k.clone(), seal_sensitive_json(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(seal_sensitive_json).collect())
        }
        other => other.clone(),
    }
}

/// Open sealed values in a JSON config blob.
pub fn open_sensitive_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_sensitive_key(k) {
                    if let Some(s) = v.as_str() {
                        let opened = open_secret(s).unwrap_or_else(|| s.to_string());
                        out.insert(k.clone(), serde_json::Value::String(opened));
                    } else {
                        out.insert(k.clone(), open_sensitive_json(v));
                    }
                } else {
                    out.insert(k.clone(), open_sensitive_json(v));
                }
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(open_sensitive_json).collect())
        }
        other => other.clone(),
    }
}

/// Open a stored secret. Returns `None` if the payload is invalid.
pub fn open_secret(stored: &str) -> Option<String> {
    if let Some(plain) = stored.strip_prefix(PLAIN_PREFIX) {
        return Some(plain.to_string());
    }
    let encoded = stored.strip_prefix(ENC_PREFIX)?;
    let payload = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    if payload.len() < 12 {
        return None;
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new_from_slice(&derive_key()).ok()?;
    let plain = cipher.decrypt(nonce, ciphertext).ok()?;
    String::from_utf8(plain).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_secrets_key() {
        // SAFETY: test-only; single-threaded `cargo test` for this crate.
        unsafe {
            std::env::set_var("CLAWZ_SECRETS_KEY", "test-key-material");
        }
        let sealed = seal_secret("sk-test-123");
        assert!(sealed.starts_with(ENC_PREFIX));
        assert_eq!(open_secret(&sealed).as_deref(), Some("sk-test-123"));
        unsafe {
            std::env::remove_var("CLAWZ_SECRETS_KEY");
        }
    }
}
