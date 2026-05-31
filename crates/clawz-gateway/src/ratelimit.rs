//! In-memory per-actor token-bucket rate limiting (Axum middleware).
//!
//! Provides baseline abuse/DoS protection without external infrastructure. Each
//! actor (API key, then forwarded client IP, else a shared anonymous bucket) gets
//! a token bucket per endpoint class; auth/setup endpoints are budgeted more
//! strictly than general API traffic. Exceeding the budget returns `429` with a
//! `Retry-After` and `X-RateLimit-*` headers.
//!
//! This is the per-node "fast path". When a Postgres pool is registered (via
//! [`set_distributed_pool`]), a fixed-window counter additionally enforces a
//! shared cross-fleet quota (fail-open on DB error). Limits are env-tunable
//! (`CLAWZ_RATELIMIT_PER_MIN`, `CLAWZ_RATELIMIT_AUTH_PER_MIN`,
//! `CLAWZ_RATELIMIT_DISTRIBUTED_PER_MIN`) and the layer can be disabled with
//! `CLAWZ_RATELIMIT_DISABLED=1`.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use parking_lot::Mutex;
use sqlx::PgPool;

/// Optional Postgres pool for distributed (cross-fleet) quota enforcement.
/// Registered once at bootstrap when `DATABASE_URL` is configured; when unset
/// the limiter is per-node only.
static DISTRIBUTED_POOL: OnceLock<PgPool> = OnceLock::new();

/// Register the Postgres pool used for cross-fleet rate-limit counters.
pub fn set_distributed_pool(pool: PgPool) {
    let _ = DISTRIBUTED_POOL.set(pool);
}

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

/// Cross-fleet quota check via a Postgres fixed-window counter.
///
/// No-op (allows) when no pool is registered. The per-node bucket is the fast
/// path; this enforces a shared ceiling across all gateway nodes. On any DB
/// error it **fails open** (allows + logs) so a database blip can't take the
/// API down. `Err(retry_after_secs)` means the shared quota is exhausted.
async fn distributed_check(bucket: &str, policy: &Policy) -> Result<(), u64> {
    let Some(pool) = DISTRIBUTED_POOL.get() else {
        return Ok(());
    };
    // Default the cross-fleet ceiling well above the per-node cap so it only
    // catches egregious distributed abuse; fully env-tunable.
    let limit = env_f64(
        "CLAWZ_RATELIMIT_DISTRIBUTED_PER_MIN",
        policy.capacity * 10.0,
    ) as i64;
    let window = policy.window_secs as u64;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let window_start = (now / window * window) as i64;
    let res = sqlx::query_scalar::<_, i64>(
        "INSERT INTO rate_limit_counters (bucket, window_start, hits) VALUES ($1, $2, 1) \
         ON CONFLICT (bucket, window_start) DO UPDATE SET hits = rate_limit_counters.hits + 1 \
         RETURNING hits",
    )
    .bind(bucket)
    .bind(window_start)
    .fetch_one(pool)
    .await;
    match res {
        Ok(hits) if hits > limit => Err((window - (now % window)).max(1)),
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!("distributed rate-limit check failed (fail-open): {e}");
            Ok(())
        }
    }
}

/// Build the standard `429 Too Many Requests` response with rate-limit headers.
fn too_many(limit: u64, retry_after: u64) -> Response {
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded\n").into_response();
    let h = resp.headers_mut();
    h.insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    h.insert("x-ratelimit-limit", HeaderValue::from(limit));
    h.insert("x-ratelimit-remaining", HeaderValue::from(0u64));
    resp
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
            // Per-node bucket passed; enforce the cross-fleet quota too (no-op
            // when no DB pool is registered; fails open on DB error).
            if let Err(retry_after) = distributed_check(&store_key, &policy).await {
                return too_many(limit, retry_after);
            }
            let mut resp = next.run(request).await;
            let h = resp.headers_mut();
            h.insert("x-ratelimit-limit", HeaderValue::from(limit));
            h.insert("x-ratelimit-remaining", HeaderValue::from(remaining));
            resp
        }
        Err(retry_after) => too_many(limit, retry_after),
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

    /// Exercises the distributed (Postgres) counter end-to-end. Skips unless
    /// DATABASE_URL is set, so the normal lib test run is unaffected.
    #[tokio::test]
    async fn distributed_counter_enforces_shared_quota() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return; // no DB configured in this run; nothing to assert
        };
        let pool = PgPool::connect(&url).await.expect("connect");
        clawz_core::db::run_migrations(&pool)
            .await
            .expect("migrate");
        set_distributed_pool(pool);
        // A low ceiling + unique bucket keep the test isolated and fast.
        unsafe {
            std::env::set_var("CLAWZ_RATELIMIT_DISTRIBUTED_PER_MIN", "2");
        }
        let policy = Policy {
            capacity: 100.0,
            window_secs: 60.0,
        };
        let bucket = format!("test:{}", uuid::Uuid::new_v4());
        assert!(distributed_check(&bucket, &policy).await.is_ok()); // hits=1
        assert!(distributed_check(&bucket, &policy).await.is_ok()); // hits=2
        assert!(distributed_check(&bucket, &policy).await.is_err()); // hits=3 > 2
        unsafe {
            std::env::remove_var("CLAWZ_RATELIMIT_DISTRIBUTED_PER_MIN");
        }
    }
}
