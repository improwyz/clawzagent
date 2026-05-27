//! AWS Signature Version 4 request signing for deploy adapters.

use std::collections::HashMap;
use std::fmt::Write;

use sha2::{Digest, Sha256};

/// Build SigV4 authorization headers for an AWS API request.
#[allow(clippy::too_many_arguments)]
pub fn sign_headers(
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
    method: &str,
    host: &str,
    path: &str,
    query: &str,
    payload: &[u8],
) -> HashMap<String, String> {
    let payload_hash = sha256_hex(payload);
    let now = chrono::Utc::now();
    let date_stamp = now.format("%Y%m%d").to_string();
    let datetime_stamp = now.format("%Y%m%dT%H%M%SZ").to_string();

    let canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{datetime_stamp}\n");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request =
        format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}");

    let credential_scope = format!("{date_stamp}/{region}/{service}/aws4_request");
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{datetime_stamp}\n{credential_scope}\n{canonical_request_hash}");

    let signing_key = derive_signing_key(secret_key, &date_stamp, region, service);
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());

    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={access_key}/{credential_scope},SignedHeaders={signed_headers},Signature={signature}"
    );

    let mut headers = HashMap::new();
    headers.insert("Authorization".to_string(), authorization);
    headers.insert("x-amz-date".to_string(), datetime_stamp);
    headers.insert("x-amz-content-sha256".to_string(), payload_hash);
    headers.insert("host".to_string(), host.to_string());
    headers
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").ok();
        s
    })
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let block_size = 64;
    let mut k = key.to_vec();
    if k.len() > block_size {
        let mut hasher = Sha256::new();
        hasher.update(&k);
        k = hasher.finalize().to_vec();
    }
    k.resize(block_size, 0);
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    outer.finalize().to_vec()
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    hmac_sha256(key, data)
        .iter()
        .fold(String::new(), |mut s, b| {
            write!(s, "{b:02x}").ok();
            s
        })
}

fn derive_signing_key(secret_key: &str, date_stamp: &str, region: &str, service: &str) -> Vec<u8> {
    let k = format!("AWS4{secret_key}");
    let k1 = hmac_sha256(k.as_bytes(), date_stamp.as_bytes());
    let k2 = hmac_sha256(&k1, region.as_bytes());
    let k3 = hmac_sha256(&k2, service.as_bytes());
    hmac_sha256(&k3, b"aws4_request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_headers_includes_authorization() {
        let headers = sign_headers(
            "AKIA_TEST",
            "secret",
            "us-east-1",
            "lambda",
            "GET",
            "lambda.us-east-1.amazonaws.com",
            "/2015-03-31/functions",
            "",
            b"",
        );
        assert!(headers["Authorization"].starts_with("AWS4-HMAC-SHA256"));
        assert!(headers.contains_key("x-amz-date"));
    }
}
