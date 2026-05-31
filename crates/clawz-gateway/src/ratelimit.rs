//! In-memory per-actor token-bucket rate limiting (Axum middleware).
//!
//! Provides baseline abuse/DoS protection without external infrastructure. Each
//! actor (API key, then forwarded client IP, else a shared anonymous bucket) gets
//! a token bucket per endpoint class; auth/setup endpoints are budgeted more
//! strictly than general API traffic. Exceeding the budget returns `429` with a
//! `Retry-After` and `X-RateLimit-*` headers.
//!
//! This is the per-node "fast path"; a distributed (Postgres-backed) counter for
//! cross-fleet quota enforcement is a planned follow-up. Limits are env-tunable
//! (`CLAWZ_RATELIMIT_PER_MIN`, `CLAWZ_RATELIMIT_AUTH_PER_MIN`) and the layer can
//! be disabled with `CLAWZ_RATELIMIT_DISABLED=1`.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;

/// A single actor's token bucket.
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Per-actor+class buckets, keyed by `"{class}:{actor}"`.
static STORE: LazyLock<Mutex<HashMap<String, Bucket>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Token-bucket parameters for an endpoint class.
struct Policy {
    capacity: f64,
    window_secs: f64,
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(default)
}

/// Resolve the policy and class label for a request path.
fn policy_for(path: &str) -> (&'static str, Policy) {
    let sensitive = path.contains("/auth/")
        || path.starts_with("/api/v1/setup")
        || path.ends_with("/login")
        || path.ends_with("/register");
    if sensitive {
        (
            "auth",
            Policy {
                capacity: env_f64("CLAWZ_RATELIMIT_AUTH_PER_MIN", 30.0),
                window_secs: 60.0,
            },
        )
    } else {
        (
            "api",
            Policy {
                capacity: env_f64("CLAWZ_RATELIMIT_PER_MIN", 600.0),
                window_secs: 60.0,
            },
        )
    }
}

/// Identify the caller: API key (hashed), then forwarded client IP, else anon.
fn actor_key(headers: &HeaderMap) -> String {
    if let Some(k) = headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        return format!("key:{}", short_hash(k));
    }
    if let Some(tok) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|a| a.strip_prefix("Bearer "))
    {
        return format!("key:{}", short_hash(tok));
    }
    for h in ["x-forwarded-for", "x-real-ip"] {
        if let Some(v) = headers.get(h).and_then(|v| v.to_str().ok()) {
            let ip = v.split(',').next().unwrap_or(v).trim();
            if !ip.is_empty() {
                return format!("ip:{ip}");
            }
        }
    }
    "anon".to_string()
}

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(s.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Consume one token. `Ok(remaining)` if allowed, `Err(retry_after_secs)` if not.
fn check(store_key: &str, policy: &Policy) -> Result<u64, u64> {
    let rate = policy.capacity / policy.window_secs;
    let now = Instant::now();
    let mut store = STORE.lock();
    let bucket = store.entry(store_key.to_string()).or_insert(Bucket {
        tokens: policy.capacity,
        last: now,
    });
    let elapsed = now.duration_since(bucket.last).as_secs_f64();
    bucket.tokens = (bucket.tokens + elapsed * rate).min(policy.capacity);
    bucket.last = now;
    if bucket.tokens >= 1.0 {
        bucket.tokens -= 1.0;
        Ok(bucket.tokens as u64)
    } else {
        let retry = ((1.0 - bucket.tokens) / rate).ceil() as u64;
        Err(retry.max(1))
    }
}

/// Paths exempt from rate limiting (liveness + docs).
fn exempt(path: &str) -> bool {
    path == "/health" || path.starts_with("/api/docs")
}

/// Axum middleware enforcing per-actor token-bucket rate limits.
pub async fn rate_limit(request: Request, next: Next) -> Response {
    if std::env::var("CLAWZ_RATELIMIT_DISABLED").as_deref() == Ok("1") {
        return next.run(request).await;
    }
    // In debug, dev-auth mode skips limiting so local/test flows are unaffected.
    #[cfg(debug_assertions)]
    if std::env::var("CLAWZ_DISABLE_AUTH").as_deref() == Ok("1") {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();
    if exempt(&path) {
        return next.run(request).await;
    }

    let (class, policy) = policy_for(&path);
    let store_key = format!("{class}:{}", actor_key(request.headers()));
    let limit = policy.capacity as u64;

    match check(&store_key, &policy) {
        Ok(remaining) => {
            let mut resp = next.run(request).await;
            let h = resp.headers_mut();
            h.insert("x-ratelimit-limit", HeaderValue::from(limit));
            h.insert("x-ratelimit-remaining", HeaderValue::from(remaining));
            resp
        }
        Err(retry_after) => {
            let mut resp = (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded\n").into_response();
            let h = resp.headers_mut();
            h.insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
            h.insert("x-ratelimit-limit", HeaderValue::from(limit));
            h.insert("x-ratelimit-remaining", HeaderValue::from(0u64));
            resp
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_up_to_capacity_then_blocks() {
        let policy = Policy {
            capacity: 2.0,
            window_secs: 60.0,
        };
        let key = "test:unique-actor-abc";
        assert!(check(key, &policy).is_ok());
        assert!(check(key, &policy).is_ok());
        assert!(check(key, &policy).is_err());
    }

    #[test]
    fn auth_paths_are_stricter() {
        let (class, _) = policy_for("/api/v1/system/auth/login");
        assert_eq!(class, "auth");
        let (class, _) = policy_for("/api/v1/agents");
        assert_eq!(class, "api");
    }
}
