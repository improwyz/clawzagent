//! Exponential-backoff retry helper shared across the workspace.
//!
//! This is the canonical retry primitive: the worker's channel plugins and any
//! other crate that needs "retry a fallible async op with backoff, but honor an
//! upstream `Retry-After`" should call [`retry_with_backoff`] rather than
//! re-implementing the loop.
//!
//! Note: `clawz-worker`'s provider router intentionally keeps its *own*
//! retry loop (`execute_with_retry`) — it is a distinct resilience policy
//! (jittered exponential backoff, circuit-breaker integration, and
//! no-retry-on-auth classification) and is not a duplicate of this helper.

use std::time::Duration;

use crate::error::{ClawzError, Result};

/// Retry a fallible async operation with exponential back-off.
///
/// `max_attempts` – total attempts (including the first).
/// `base_delay`   – initial back-off delay; doubles after each failure.
///
/// If the operation returns [`ClawzError::RateLimited`], the retry waits for the
/// exact `retry_after_secs` instead of using exponential back-off. This
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
                tracing::warn!(
                    "Attempt {attempt}/{max_attempts} failed: {e}. Retrying in {delay:?}"
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

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
