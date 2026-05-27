//! # AWS Connector
//!
//! Integrates with Amazon Web Services using Signature Version 4 (SigV4) authentication.
//! Supports S3 (list buckets, put/delete objects), SNS (list topics, create topics, publish),
//! and SES (send email). Unlike most connectors, AWS does not use OAuth2 or Bearer tokens;
//! every request must be signed with HMAC-SHA256 derived from the secret access key.
//!
//! // Dependency: `crate::connectors::trait::{AuthType, Credentials, Filters, SaaSConnector}`.
//! // Dependency: `reqwest::Client` used directly because requests require custom SigV4 headers.

use async_trait::async_trait;
use clawz_core::error::{ClawzError, Result};
use reqwest::Client;
use serde_json::Value;
use std::collections::HashMap;

use crate::connectors::r#trait::{AuthType, Credentials, Filters, SaaSConnector};

/// AWS connector supporting S3, SES, and SNS via SigV4 authentication.
///
/// Stores IAM-style credentials (access key ID + secret access key) and the target AWS
/// region. All requests are signed on-the-fly using the `sign_request` helper.
pub struct AwsConnector {
    /// IAM access key ID.
    access_key_id: String,
    /// IAM secret access key used to derive the signing key.
    secret_access_key: String,
    /// AWS region (e.g. `us-east-1`, `eu-west-1`).
    region: String,
    /// Reusable HTTP client.
    client: Client,
}

impl AwsConnector {
    /// Create a new AWS connector.
    pub fn new(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        region: impl Into<String>,
    ) -> Self {
        Self {
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            region: region.into(),
            client: Client::new(),
        }
    }

    /// Sign a request using AWS Signature Version 4.
    ///
    /// Implements the full SigV4 signing process: canonical request, string-to-sign,
    /// HMAC-SHA256 key derivation, and signature generation. Returns a map of headers
    /// that must be added to the outgoing HTTP request.
    fn sign_request(
        &self,
        method: &str,
        service: &str,
        host: &str,
        path: &str,
        query: &str,
        payload_hash: &str,
    ) -> HashMap<String, String> {
        use sha2::{Digest, Sha256};
        use std::fmt::Write;

        let now = chrono::Utc::now();
        let date_stamp = now.format("%Y%m%d").to_string();
        let datetime_stamp = now.format("%Y%m%dT%H%M%SZ").to_string();

        // Build the canonical request components.
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            host, payload_hash, datetime_stamp
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, path, query, canonical_headers, signed_headers, payload_hash
        );

        // Build the credential scope and string to sign.
        let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, self.region, service);
        let mut hasher = Sha256::new();
        hasher.update(canonical_request.as_bytes());
        let canonical_request_hash = hasher.finalize().iter().fold(String::new(), |mut s, b| {
            write!(s, "{:02x}", b).ok();
            s
        });

        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime_stamp, credential_scope, canonical_request_hash
        );

        // HMAC-SHA256 helper. We implement it manually to avoid adding an hmac crate
        // dependency; sha2 alone is sufficient.
        fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
            use sha2::Sha256;
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
            outer.update(opad);
            outer.update(inner_hash);
            outer.finalize().to_vec()
        }

        // Derive the signing key via the SigV4 key chain.
        let signing_key = {
            let k = format!("AWS4{}", self.secret_access_key);
            let k1 = hmac_sha256(k.as_bytes(), date_stamp.as_bytes());
            let k2 = hmac_sha256(&k1, self.region.as_bytes());
            let k3 = hmac_sha256(&k2, service.as_bytes());
            hmac_sha256(&k3, b"aws4_request")
        };

        let signature = hmac_sha256(&signing_key, string_to_sign.as_bytes())
            .iter()
            .fold(String::new(), |mut s, b| {
                write!(s, "{:02x}", b).ok();
                s
            });

        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{},SignedHeaders={},Signature={}",
            self.access_key_id, credential_scope, signed_headers, signature
        );

        let mut headers = HashMap::new();
        headers.insert("Authorization".into(), authorization);
        headers.insert("x-amz-date".into(), datetime_stamp);
        headers.insert("x-amz-content-sha256".into(), payload_hash.into());
        headers
    }
}

#[async_trait]
impl SaaSConnector for AwsConnector {
    fn platform_id(&self) -> &str {
        "aws"
    }

    fn display_name(&self) -> &str {
        "AWS"
    }

    fn auth_type(&self) -> AuthType {
        AuthType::ApiKey
    }

    async fn auth_url(&self, _redirect: &str) -> Result<String> {
        Err(ClawzError::Auth("AWS uses SigV4 authentication".into()))
    }

    async fn exchange_code(&self, _code: &str) -> Result<Credentials> {
        Err(ClawzError::Auth("AWS uses SigV4 authentication".into()))
    }

    async fn list_objects(&self, obj: &str, _filters: &Filters) -> Result<Vec<Value>> {
        match obj {
            "s3_buckets" => {
                let host = "s3.amazonaws.com";
                // SHA256 hash of an empty body; required for GET requests with no payload.
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let headers = self.sign_request("GET", "s3", host, "/", "", empty_hash);
                let mut req = self.client.get(format!("https://{}/", host));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req.send().await.map_err(|e| {
                    ClawzError::Provider(format!("AWS S3 list buckets failed: {e}"))
                })?;
                // S3 returns XML; wrap it in a JSON envelope so callers get a consistent Value shape.
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClawzError::Internal(format!("read body: {e}")))?;
                Ok(vec![serde_json::json!({ "xml_response": text })])
            }
            "sns_topics" => {
                let host = format!("sns.{}.amazonaws.com", self.region);
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let query = "Action=ListTopics&Version=2010-03-31";
                let headers = self.sign_request("GET", "sns", &host, "/", query, empty_hash);
                let mut req = self.client.get(format!("https://{}/?{}", host, query));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req.send().await.map_err(|e| {
                    ClawzError::Provider(format!("AWS SNS list topics failed: {e}"))
                })?;
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClawzError::Internal(format!("read body: {e}")))?;
                Ok(vec![serde_json::json!({ "xml_response": text })])
            }
            _ => Err(ClawzError::Provider(format!("Unknown AWS object: {obj}"))),
        }
    }

    async fn create_object(&self, obj: &str, data: Value) -> Result<Value> {
        match obj {
            "s3_object" => {
                let bucket = data["bucket"].as_str().unwrap_or("");
                let key = data["key"].as_str().unwrap_or("");
                let body_str = data["body"].as_str().unwrap_or("");
                let host = format!("{}.s3.{}.amazonaws.com", bucket, self.region);
                use sha2::{Digest, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(body_str.as_bytes());
                let payload_hash = format!("{:x}", hasher.finalize());
                let headers =
                    self.sign_request("PUT", "s3", &host, &format!("/{}", key), "", &payload_hash);
                let mut req = self
                    .client
                    .put(format!("https://{}/{}", host, key))
                    .body(body_str.to_string());
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("AWS S3 put object failed: {e}")))?;
                if resp.status().is_success() {
                    Ok(serde_json::json!({ "status": "created", "key": key }))
                } else {
                    Err(ClawzError::Provider(format!(
                        "AWS S3 put failed: HTTP {}",
                        resp.status().as_u16()
                    )))
                }
            }
            "sns_topic" => {
                let name = data["name"].as_str().unwrap_or("NewTopic");
                let host = format!("sns.{}.amazonaws.com", self.region);
                let query = format!("Action=CreateTopic&Name={}&Version=2010-03-31", name);
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let headers = self.sign_request("GET", "sns", &host, "/", &query, empty_hash);
                let mut req = self.client.get(format!("https://{}/?{}", host, query));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req.send().await.map_err(|e| {
                    ClawzError::Provider(format!("AWS SNS create topic failed: {e}"))
                })?;
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClawzError::Internal(format!("read body: {e}")))?;
                Ok(serde_json::json!({ "xml_response": text }))
            }
            _ => Err(ClawzError::Provider(format!("Unknown AWS object: {obj}"))),
        }
    }

    async fn update_object(&self, obj: &str, _id: &str, _data: Value) -> Result<Value> {
        // AWS REST APIs do not have a generic "update" semantic for S3/SNS/SES;
        // callers should use `execute_action` for platform-specific mutations.
        Err(ClawzError::Provider(format!(
            "AWS {obj} update not supported directly; use execute_action"
        )))
    }

    async fn delete_object(&self, obj: &str, id: &str) -> Result<()> {
        match obj {
            "s3_object" => {
                // id = "bucket/key"
                let parts: Vec<&str> = id.splitn(2, '/').collect();
                let (bucket, key) = (
                    parts.first().copied().unwrap_or(""),
                    parts.last().copied().unwrap_or(""),
                );
                let host = format!("{}.s3.{}.amazonaws.com", bucket, self.region);
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let headers =
                    self.sign_request("DELETE", "s3", &host, &format!("/{}", key), "", empty_hash);
                let mut req = self.client.delete(format!("https://{}/{}", host, key));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("AWS S3 delete failed: {e}")))?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    Err(ClawzError::Provider(format!(
                        "AWS S3 delete failed: HTTP {}",
                        resp.status().as_u16()
                    )))
                }
            }
            _ => Err(ClawzError::Provider(format!("Cannot delete AWS {obj}"))),
        }
    }

    async fn execute_action(&self, action: &str, params: Value) -> Result<Value> {
        match action {
            "ses_send_email" => {
                let host = format!("email.{}.amazonaws.com", self.region);
                let to = params["to"].as_str().unwrap_or("");
                let from = params["from"].as_str().unwrap_or("");
                let subject = params["subject"].as_str().unwrap_or("");
                let body = params["body"].as_str().unwrap_or("");
                let query = format!(
                    "Action=SendEmail&Source={}&Destination.ToAddresses.member.1={}&Message.Subject.Data={}&Message.Body.Text.Data={}&Version=2010-12-01",
                    urlencoding_encode(from),
                    urlencoding_encode(to),
                    urlencoding_encode(subject),
                    urlencoding_encode(body)
                );
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let headers = self.sign_request("GET", "ses", &host, "/", &query, empty_hash);
                let mut req = self.client.get(format!("https://{}/?{}", host, query));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("AWS SES send email failed: {e}")))?;
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClawzError::Internal(format!("read body: {e}")))?;
                Ok(serde_json::json!({ "xml_response": text }))
            }
            "sns_publish" => {
                let host = format!("sns.{}.amazonaws.com", self.region);
                let topic = params["topic_arn"].as_str().unwrap_or("");
                let message = params["message"].as_str().unwrap_or("");
                let query = format!(
                    "Action=Publish&TopicArn={}&Message={}&Version=2010-03-31",
                    urlencoding_encode(topic),
                    urlencoding_encode(message)
                );
                let empty_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
                let headers = self.sign_request("GET", "sns", &host, "/", &query, empty_hash);
                let mut req = self.client.get(format!("https://{}/?{}", host, query));
                for (k, v) in &headers {
                    req = req.header(k.as_str(), v.as_str());
                }
                let resp = req
                    .send()
                    .await
                    .map_err(|e| ClawzError::Provider(format!("AWS SNS publish failed: {e}")))?;
                let text = resp
                    .text()
                    .await
                    .map_err(|e| ClawzError::Internal(format!("read body: {e}")))?;
                Ok(serde_json::json!({ "xml_response": text }))
            }
            _ => Err(ClawzError::Provider(format!(
                "Unknown AWS action: {action}"
            ))),
        }
    }
}

fn urlencoding_encode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            c => format!("%{:02X}", c as u32),
        })
        .collect()
}
