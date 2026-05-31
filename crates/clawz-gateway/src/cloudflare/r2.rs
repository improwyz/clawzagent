//! Cloudflare R2 — S3-compatible object storage for agent artifacts.
//!
//! This module provides a lightweight HTTP client for uploading, downloading,
//! listing, and deleting objects in a Cloudflare R2 bucket.  It intentionally
//! avoids pulling in the full AWS SDK by using the simpler token-based auth
//! supported by Cloudflare's own REST endpoints where possible, and by
//! hand-parsing the S3 `ListObjectsV2` XML response.
//!
//! API base: `https://{account_id}.r2.cloudflarestorage.com/{bucket}/`
//!
//! Auth note: The R2 S3-compatible API normally requires AWS Signature V4.
//! This implementation uses the simpler `Authorization: Bearer <api_token>`
//! scheme supported by Cloudflare's own HTTP API when accessing objects
//! through the REST API.  For full S3-compat with pre-signed URLs you would
//! need `aws-sigv4`; that is intentionally out of scope here to keep the
//! dependency tree small.
//!
//! # Key dependencies
//! - [`clawz_core::error::ClawzError`] — unified error type.
//! - [`super::config::CloudflareConfig`] — provides account credentials and bucket name.

use clawz_core::error::ClawzError;
use serde::{Deserialize, Serialize};

// Dependency: top-level config used to decide if this client is active.
use super::config::CloudflareConfig;

// ── Public types ──────────────────────────────────────────────────────────────

/// Metadata for a single object stored in R2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct R2Object {
    /// Object key (path within the bucket).
    pub key: String,
    /// Size in bytes.
    pub size: u64,
    /// Last-modified timestamp in ISO-8601 format.
    pub last_modified: String,
    /// ETag (content hash) if returned by the API.
    pub etag: Option<String>,
    /// MIME type reported at upload time.
    pub content_type: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the Cloudflare R2 S3-compatible API.
pub struct R2Client {
    /// Cloudflare account ID (used as the S3 endpoint subdomain).
    account_id: String,
    /// API token with R2 read/write scopes.
    api_token: String,
    /// Target bucket name.
    bucket_name: String,
    /// HTTP transport.
    client: reqwest::Client,
}

impl R2Client {
    /// Construct from the master config; returns `None` when the service is
    /// disabled or the bucket name is not configured.
    pub fn from_config(cfg: &CloudflareConfig) -> Option<Self> {
        if !cfg.enabled || !cfg.r2.enabled {
            return None;
        }
        let bucket_name = cfg.r2.bucket_name.clone()?;
        Some(Self::new(
            cfg.account_id.clone(),
            cfg.api_token.clone(),
            bucket_name,
        ))
    }

    /// Direct constructor when credentials and bucket are already known.
    pub fn new(account_id: String, api_token: String, bucket_name: String) -> Self {
        Self {
            account_id,
            api_token,
            bucket_name,
            client: reqwest::Client::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// S3-compatible endpoint URL for a specific object key.
    fn bucket_url(&self, key: &str) -> String {
        format!(
            "https://{}.r2.cloudflarestorage.com/{}/{}",
            self.account_id, self.bucket_name, key
        )
    }

    /// List endpoint (virtual-hosted style) with optional prefix filter.
    fn list_url(&self, prefix: Option<&str>) -> String {
        let base = format!(
            "https://{}.r2.cloudflarestorage.com/{}",
            self.account_id, self.bucket_name
        );
        match prefix {
            Some(p) if !p.is_empty() => format!("{base}?list-type=2&prefix={p}"),
            _ => format!("{base}?list-type=2"),
        }
    }

    /// Bearer token header value.
    fn auth_header(&self) -> String {
        format!("Bearer {}", self.api_token)
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Upload an object to R2.
    ///
    /// `content_type` is stored as metadata and returned on subsequent `get_object`
    /// calls via the `Content-Type` response header.
    pub async fn put_object(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), ClawzError> {
        let url = self.bucket_url(key);

        let resp = self
            .client
            .put(&url)
            .header("Authorization", self.auth_header())
            .header("Content-Type", content_type)
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 put_object failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "R2 put_object returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// Download an object from R2.
    ///
    /// Returns the raw bytes; the caller is responsible for decoding
    /// (e.g. JSON, images, etc.).
    pub async fn get_object(&self, key: &str) -> Result<Vec<u8>, ClawzError> {
        let url = self.bucket_url(key);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 get_object failed: {e}")))?;

        if resp.status().as_u16() == 404 {
            return Err(ClawzError::NotFound {
                entity: "R2 object".into(),
                id: key.to_owned(),
            });
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "R2 get_object returned {status}: {text}"
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 get_object body read failed: {e}")))?;

        Ok(bytes.to_vec())
    }

    /// Delete an object from R2.
    pub async fn delete_object(&self, key: &str) -> Result<(), ClawzError> {
        let url = self.bucket_url(key);

        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 delete_object failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "R2 delete_object returned {status}: {text}"
            )));
        }

        Ok(())
    }

    /// List objects in the bucket, optionally filtered by prefix.
    ///
    /// Cloudflare R2 returns an S3-style XML `ListObjectsV2` response.  We parse
    /// it with a lightweight string scan rather than pulling in an XML crate
    /// to keep compile times and binary size down.
    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<R2Object>, ClawzError> {
        let url = self.list_url(prefix);

        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 list_objects failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ClawzError::Transport(format!(
                "R2 list_objects returned {status}: {text}"
            )));
        }

        let xml = resp
            .text()
            .await
            .map_err(|e| ClawzError::Transport(format!("R2 list_objects body read failed: {e}")))?;

        // Lightweight XML parse: extract <Contents> blocks one by one.
        let objects = parse_list_objects_xml(&xml);
        Ok(objects)
    }
}

/// Parse S3 `ListObjectsV2` XML response into an `R2Object` vec without an XML crate.
///
/// We scan for `<Contents>...</Contents>` blocks and then pull out the inner
/// tags (`Key`, `Size`, `LastModified`, `ETag`, `ContentType`) with simple
/// substring searches.  This is brittle against deep XML nesting or CDATA,
/// but sufficient for the flat S3 response format.
fn parse_list_objects_xml(xml: &str) -> Vec<R2Object> {
    let mut objects = Vec::new();
    let mut rest = xml;

    while let Some(start) = rest.find("<Contents>") {
        rest = &rest[start + "<Contents>".len()..];
        let end = rest.find("</Contents>").unwrap_or(rest.len());
        let block = &rest[..end];

        let key = extract_xml_tag(block, "Key").unwrap_or_default();
        let size = extract_xml_tag(block, "Size")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let last_modified = extract_xml_tag(block, "LastModified").unwrap_or_default();
        let etag = extract_xml_tag(block, "ETag");
        let content_type = extract_xml_tag(block, "ContentType");

        if !key.is_empty() {
            objects.push(R2Object {
                key,
                size,
                last_modified,
                etag,
                content_type,
            });
        }

        rest = &rest[end..];
    }

    objects
}

/// Extract the text content between `<tag>...</tag>` from a flat XML fragment.
///
/// Returns `None` if the tag is missing or malformed (end tag before start tag).
fn extract_xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml.find(&close)?;
    if end < start {
        return None;
    }
    Some(xml[start..end].to_owned())
}

// Keep old adapter name as an alias so existing code compiles unchanged.
pub type R2Adapter = R2Client;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_r2_client_from_disabled_config() {
        let cfg = CloudflareConfig::default();
        assert!(R2Client::from_config(&cfg).is_none());
    }

    #[test]
    fn test_r2_client_from_enabled_config() {
        let mut cfg = CloudflareConfig {
            enabled: true,
            account_id: "acc123".into(),
            api_token: "tok456".into(),
            ..Default::default()
        };
        cfg.r2.enabled = true;
        cfg.r2.bucket_name = Some("my-bucket".into());
        let client = R2Client::from_config(&cfg).unwrap();
        assert_eq!(client.bucket_name, "my-bucket");
        assert_eq!(
            client.bucket_url("test-key"),
            "https://acc123.r2.cloudflarestorage.com/my-bucket/test-key"
        );
    }

    #[test]
    fn test_parse_list_objects_xml_empty() {
        let xml = "<ListBucketResult></ListBucketResult>";
        let result = parse_list_objects_xml(xml);
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_list_objects_xml_with_contents() {
        let xml = r#"
            <ListBucketResult>
                <Contents>
                    <Key>artifacts/run-1.json</Key>
                    <Size>1024</Size>
                    <LastModified>2026-01-01T00:00:00.000Z</LastModified>
                    <ETag>&quot;abc123&quot;</ETag>
                </Contents>
                <Contents>
                    <Key>logs/worker.log</Key>
                    <Size>512</Size>
                    <LastModified>2026-01-02T00:00:00.000Z</LastModified>
                </Contents>
            </ListBucketResult>
        "#;
        let result = parse_list_objects_xml(xml);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].key, "artifacts/run-1.json");
        assert_eq!(result[0].size, 1024);
        assert_eq!(result[1].key, "logs/worker.log");
    }

    #[test]
    fn test_list_url_with_prefix() {
        let client = R2Client::new("acc".into(), "tok".into(), "bkt".into());
        let url = client.list_url(Some("prefix/"));
        assert!(url.contains("prefix=prefix/"));
    }

    #[test]
    fn test_list_url_without_prefix() {
        let client = R2Client::new("acc".into(), "tok".into(), "bkt".into());
        let url = client.list_url(None);
        assert!(url.contains("list-type=2"));
        assert!(!url.contains("prefix="));
    }
}
