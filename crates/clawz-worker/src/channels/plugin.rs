//! Channel Plugin SDK helpers — reusable utilities for writing channel plugins.
//!
//! This module provides cross-cutting concerns that every channel plugin
//! needs but that should not be duplicated in each implementation:
//!
//! - **Token-bucket rate limiting** — `RateLimiter` and `SharedRateLimiter`
//!   prevent a worker from overwhelming upstream APIs with bursty traffic.
//! - **Retry logic** — `retry_with_backoff` handles transient failures and
//!   API-specific `RateLimited` responses with exponential back-off.
//! - **Markdown conversion** — platform-specific formatters turn the
//!   worker's internal CommonMark into Slack `mrkdwn`, Discord markup, or
//!   plain text.
//! - **HTTP helper** — `parse_ratelimit_headers` and `map_http_error`
//!   normalise REST responses into `ClawzError` variants.
//! - **Credential extraction** — `cred_str` / `cred_string` safely pull secrets
//!   from the JSON credentials map supplied in `ChannelConfig`.
//!
//! All functions are pure or self-contained; they carry no global state and
//! are safe to call from any plugin context.
//!
//! # Dependencies
//!
//! - `clawz_core::error::{ClawzError, Result}` — unified error vocabulary.
//! - `tokio::sync::Mutex` — async-safe rate-limiter state.
//! - `reqwest` — HTTP types for header parsing (implicit via `map_http_error`).

use std::sync::Arc;
use std::time::{Duration, Instant};

// Dependency: clawz_core::error — shared error types from the core crate.
use clawz_core::error::{ClawzError, Result};
// Dependency: tokio::sync::Mutex — async-aware mutual exclusion for the shared rate limiter.
use tokio::sync::Mutex;

// ── Token-bucket rate limiter ─────────────────────────────────────────────────

/// A per-channel token-bucket rate limiter.
///
/// Tokens refill at `rate_per_sec` per second up to `capacity`.  Each
/// `acquire()` call consumes one token; the caller should await the returned
/// future before making an API call.
#[derive(Debug)]
pub struct RateLimiter {
    /// Maximum number of tokens the bucket can hold (burst capacity).
    capacity: f64,
    /// Tokens currently available; ranges from 0.0 to `capacity`.
    tokens: f64,
    /// Tokens added per second; controls sustained throughput.
    rate_per_sec: f64,
    /// Timestamp of the last `refill` so we can compute elapsed time
    /// without needing a background task.
    last_refill: Instant,
}

impl RateLimiter {
    /// Create a new limiter with `capacity` tokens refilling at
    /// `rate_per_sec` tokens per second.
    pub fn new(capacity: f64, rate_per_sec: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            rate_per_sec,
            last_refill: Instant::now(),
        }
    }

    /// Convenience constructor: `limit_per_minute` calls per 60 seconds.
    pub fn per_minute(limit_per_minute: u32) -> Self {
        let rps = limit_per_minute as f64 / 60.0;
        Self::new(limit_per_minute as f64, rps)
    }

    /// Attempt to consume one token.  Returns the duration to sleep if tokens
    /// are exhausted, or `None` if the token was consumed immediately.
    pub fn try_acquire(&mut self) -> Option<Duration> {
        self.refill();
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            // How long until one token is available?
            let wait_secs = (1.0 - self.tokens) / self.rate_per_sec;
            Some(Duration::from_secs_f64(wait_secs))
        }
    }

    /// Add tokens based on elapsed time since the last call.
    fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        self.last_refill = now;
    }
}

/// A thread-safe, shared rate limiter.
#[derive(Clone, Debug)]
pub struct SharedRateLimiter(Arc<Mutex<RateLimiter>>);

impl SharedRateLimiter {
    /// Wrap a `RateLimiter` in an async `Arc<Mutex<…>>` so multiple Tokio
    /// tasks can share the same token bucket.
    pub fn new(capacity: f64, rate_per_sec: f64) -> Self {
        Self(Arc::new(Mutex::new(RateLimiter::new(
            capacity,
            rate_per_sec,
        ))))
    }

    /// Convenience constructor matching `RateLimiter::per_minute`.
    pub fn per_minute(limit_per_minute: u32) -> Self {
        Self(Arc::new(Mutex::new(RateLimiter::per_minute(
            limit_per_minute,
        ))))
    }

    /// Wait until a token is available, then return.
    ///
    /// The loop re-locks the mutex for each iteration so other tasks are not
    /// starved while one task sleeps.
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut guard = self.0.lock().await;
                guard.try_acquire()
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}

// ── Retry helper ──────────────────────────────────────────────────────────────

/// Retry a fallible async operation with exponential back-off.
///
/// `max_attempts` – total attempts (including the first).
/// `base_delay`   – initial back-off delay; doubles after each failure.
///
/// If the operation returns `ClawzError::RateLimited`, the retry waits for the
/// exact `retry_after_secs` instead of using exponential back-off.  This
/// respects upstream API guidance and avoids compounding throttling.
pub async fn retry_with_backoff<F, Fut, T>(
    max_attempts: u32,
    base_delay: Duration,
    mut f: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = base_delay;
    for attempt in 1..=max_attempts {
        match f().await {
            Ok(v) => return Ok(v),
            // Upstream told us exactly how long to wait; honour it.
            Err(ClawzError::RateLimited { retry_after_secs }) => {
                tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
            }
            // Transient failure — back off exponentially so we don't hammer
            // a struggling service.
            Err(e) if attempt < max_attempts => {
                log::warn!("Attempt {attempt}/{max_attempts} failed: {e}. Retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay *= 2;
            }
            // Final attempt failed — propagate the error to the caller.
            Err(e) => return Err(e),
        }
    }
    // Should be unreachable because the loop always returns inside, but we
    // keep a fallback for compile-time exhaustiveness.
    Err(ClawzError::Internal("retry exhausted".into()))
}

// ── Markdown conversion helpers ───────────────────────────────────────────────

/// Convert a plain-text or CommonMark snippet to Slack's `mrkdwn` format.
///
/// Slack uses `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<url|label>`.
pub fn markdown_to_slack_mrkdwn(text: &str) -> String {
    // **bold** -> *bold*
    let mut out = text
        .replace("**", "*")
        // __italic__ -> _italic_
        .replace("__", "_")
        // ~~strike~~ -> ~strike~
        .replace("~~", "~");

    // [label](url) -> <url|label>
    let mut result = String::with_capacity(out.len());
    let mut remaining = out.as_str();
    while let Some(start) = remaining.find('[') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + 1..];
        if let Some(end_label) = remaining.find("](") {
            let label = &remaining[..end_label];
            remaining = &remaining[end_label + 2..];
            if let Some(end_url) = remaining.find(')') {
                let url = &remaining[..end_url];
                result.push_str(&format!("<{url}|{label}>"));
                remaining = &remaining[end_url + 1..];
            } else {
                // Malformed link: preserve the opening bracket and label so
                // we don't silently swallow user content.
                result.push('[');
                result.push_str(label);
                result.push_str("](");
            }
        } else {
            // No closing bracket pair — treat '[' as literal text.
            result.push('[');
        }
    }
    result.push_str(remaining);
    out = result;

    out
}

/// Convert markdown to Discord's formatting.
///
/// Discord uses the same bold/italic syntax as CommonMark, so this is mostly
/// a passthrough; we only fix link syntax: `[label](url)` -> just `url` since
/// Discord does not support hyperlink text in normal messages (only embeds).
pub fn markdown_to_discord(text: &str) -> String {
    // Strip [label](url) -> url
    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    while let Some(start) = remaining.find('[') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + 1..];
        if let Some(end_label) = remaining.find("](") {
            remaining = &remaining[end_label + 2..];
            if let Some(end_url) = remaining.find(')') {
                let url = &remaining[..end_url];
                result.push_str(url);
                remaining = &remaining[end_url + 1..];
            } else {
                // Malformed link — preserve literal '['.
                result.push('[');
            }
        } else {
            result.push('[');
        }
    }
    result.push_str(remaining);
    result
}

/// Convert markdown to plain text (strip all formatting).
pub fn markdown_to_plain(text: &str) -> String {
    text.replace("**", "")
        .replace("__", "")
        .replace("~~", "")
        .replace(['*', '_', '`'], "")
}

// ── X-RateLimit header helpers ────────────────────────────────────────────────

/// Parse `X-RateLimit-Remaining` and `X-RateLimit-Reset` from HTTP response
/// headers.  Returns `Some((remaining, reset_epoch_secs))` if both headers are
/// present and valid.
///
/// These headers are the de-facto standard used by Discord, GitHub, and many
/// other APIs; parsing them lets the worker proactively throttle instead of
/// waiting for a 429.
pub fn parse_ratelimit_headers(headers: &reqwest::header::HeaderMap) -> Option<(u64, u64)> {
    let remaining = headers
        .get("X-RateLimit-Remaining")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())?;
    let reset = headers
        .get("X-RateLimit-Reset")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())?;
    Some((remaining, reset))
}

/// Interpret an HTTP status code from an API response and produce a
/// `ClawzError` if the response indicates failure.
///
/// This centralises HTTP-status-to-error mapping so every plugin does not
/// need to re-implement the same match arms.
pub fn map_http_error(status: reqwest::StatusCode, body: &str) -> ClawzError {
    match status.as_u16() {
        401 | 403 => ClawzError::Auth(format!("HTTP {status}: {body}")),
        429 => {
            // Try to extract retry-after from body (Slack embeds it in JSON)
            let retry_after_secs = serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|v| v.get("retry_after").and_then(|r| r.as_u64()))
                // Fallback: if the body is not JSON we still need a sensible
                // default so the caller can back off without crashing.
                .unwrap_or(60);
            ClawzError::RateLimited { retry_after_secs }
        }
        500..=599 => ClawzError::Internal(format!("HTTP {status}: {body}")),
        _ => ClawzError::Channel(format!("HTTP {status}: {body}")),
    }
}

/// Extract a string credential from `serde_json::Value` credentials map.
///
/// Most channel plugins store secrets (tokens, API keys, web-hook URLs) in a
/// JSON object inside `ChannelConfig.credentials`.  This helper turns the
/// common "get + cast to str" boilerplate into a single call with a clear
/// `Config` error when the key is missing.
pub fn cred_str<'a>(credentials: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    credentials
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| ClawzError::Config(format!("missing credential '{key}'")))
}

/// Same as `cred_str` but returns an owned `String`.
///
/// Use this when the credential needs to outlive the credentials map or
/// when passing it into an API client that requires `String`.
pub fn cred_string(credentials: &serde_json::Value, key: &str) -> Result<String> {
    cred_str(credentials, key).map(|s| s.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // Characterization tests pinning `retry_with_backoff` attempt-count and
    // RateLimited semantics BEFORE this helper is hoisted into clawz-core
    // (Phase 4 dedup). Delays are sub-millisecond so the tests stay fast.

    #[tokio::test]
    async fn retry_returns_on_first_success() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out: Result<u32> = retry_with_backoff(3, Duration::from_millis(1), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(7)
            }
        })
        .await;
        assert_eq!(out.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retry_recovers_after_transient_failures() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out: Result<u32> = retry_with_backoff(5, Duration::from_millis(1), move || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(ClawzError::Internal("transient".into()))
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_exhausts_and_returns_last_error() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out: Result<u32> = retry_with_backoff(3, Duration::from_millis(1), move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                Err(ClawzError::Internal("always".into()))
            }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retry_honors_rate_limited_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let out: Result<u32> = retry_with_backoff(3, Duration::from_millis(1), move || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                if n == 1 {
                    // retry_after 0 keeps the test instant.
                    Err(ClawzError::RateLimited {
                        retry_after_secs: 0,
                    })
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
