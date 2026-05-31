//! Idempotency-key support for non-idempotent requests (Axum middleware).
//!
//! When a mutating request (`POST`/`PUT`/`PATCH`) carries an `Idempotency-Key`
//! header, the first successful JSON response is stored in Postgres keyed by
//! that value. A retried request with the same key **replays** the stored
//! response instead of running the handler again, so a client that retries
//! after a network blip doesn't create a resource twice.
//!
//! The layer is a no-op unless a pool is registered (set at bootstrap when
//! `DATABASE_URL` is configured) and the request actually carries the header,
//! so non-idempotent flows and reads are untouched. It **fails open** on any DB
//! error.

use std::sync::OnceLock;

use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sqlx::PgPool;

/// Postgres pool for the idempotency-key store; registered at bootstrap.
static POOL: OnceLock<PgPool> = OnceLock::new();

/// Register the Postgres pool used for idempotency-key storage.
pub fn set_pool(pool: PgPool) {
    let _ = POOL.set(pool);
}

/// Largest response body we will buffer in order to store/replay it.
const MAX_REPLAY_BYTES: usize = 1024 * 1024;

/// Axum middleware implementing `Idempotency-Key` replay for mutating requests.
pub async fn idempotency(request: Request, next: Next) -> Response {
    let Some(pool) = POOL.get() else {
        return next.run(request).await;
    };
    let is_mutating = matches!(
        *request.method(),
        Method::POST | Method::PUT | Method::PATCH
    );
    let key = request
        .headers()
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let (Some(key), true) = (key, is_mutating) else {
        return next.run(request).await;
    };
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    // Replay a previously stored response when the key is known (within TTL).
    if let Some((status, body)) = lookup(pool, &key).await {
        let code = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
        return (
            code,
            [(header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response();
    }

    // First time we've seen this key: run the handler and capture the response.
    let resp = next.run(request).await;
    let status = resp.status();
    let is_json = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|c| c.contains("application/json"))
        .unwrap_or(false);

    // Only JSON success responses are replayable; everything else passes through
    // untouched (no body buffering).
    if !status.is_success() || !is_json {
        return resp;
    }

    let (parts, body) = resp.into_parts();
    let bytes = match to_bytes(body, MAX_REPLAY_BYTES).await {
        Ok(b) => b,
        // Body too large or unreadable: don't store, return what we can.
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "response too large").into_response(),
    };
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        store(
            pool,
            &key,
            method.as_str(),
            &path,
            parts.status.as_u16() as i32,
            &value,
        )
        .await;
    }
    Response::from_parts(parts, Body::from(bytes))
}

/// Look up a stored response for `key` if it exists and is within TTL (24h).
/// Fails open (returns `None`) on any database error.
async fn lookup(pool: &PgPool, key: &str) -> Option<(i32, serde_json::Value)> {
    let row = sqlx::query_as::<_, (i32, serde_json::Value)>(
        "SELECT status_code, response FROM idempotency_keys \
         WHERE id = $1 AND created_at > NOW() - INTERVAL '24 hours'",
    )
    .bind(key)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(opt) => opt,
        Err(e) => {
            tracing::warn!("idempotency lookup failed (fail-open): {e}");
            None
        }
    }
}

/// Store the first response for `key`. First write wins; concurrent duplicates
/// are ignored. Best-effort: errors are logged, not propagated.
async fn store(
    pool: &PgPool,
    key: &str,
    method: &str,
    path: &str,
    status_code: i32,
    response: &serde_json::Value,
) {
    let res = sqlx::query(
        "INSERT INTO idempotency_keys (id, method, path, status_code, response) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (id) DO NOTHING",
    )
    .bind(key)
    .bind(method)
    .bind(path)
    .bind(status_code)
    .bind(response)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!("idempotency store failed (best-effort): {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, middleware, routing::post};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use tower::ServiceExt;

    /// End-to-end replay test. Skips unless DATABASE_URL is set.
    #[tokio::test]
    async fn replays_stored_response_and_runs_handler_once() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            return;
        };
        let pool = PgPool::connect(&url).await.expect("connect");
        clawz_core::db::run_migrations(&pool)
            .await
            .expect("migrate");
        set_pool(pool);

        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();
        let app = Router::new()
            .route(
                "/x",
                post(move || {
                    let c = c.clone();
                    async move {
                        let n = c.fetch_add(1, Ordering::SeqCst) + 1;
                        axum::Json(serde_json::json!({ "n": n }))
                    }
                }),
            )
            .layer(middleware::from_fn(idempotency));

        let key = format!("itest-{}", uuid::Uuid::new_v4());
        let make_req = || {
            Request::builder()
                .method("POST")
                .uri("/x")
                .header("idempotency-key", &key)
                .body(Body::empty())
                .unwrap()
        };

        let r1 = app.clone().oneshot(make_req()).await.unwrap();
        let b1 = to_bytes(r1.into_body(), MAX_REPLAY_BYTES).await.unwrap();
        let r2 = app.clone().oneshot(make_req()).await.unwrap();
        let b2 = to_bytes(r2.into_body(), MAX_REPLAY_BYTES).await.unwrap();

        assert_eq!(b1, b2, "second response should replay the first");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "handler must run only once for the same idempotency key"
        );
    }
}
