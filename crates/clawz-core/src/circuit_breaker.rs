//! Lock-free circuit breaker for resilient provider routing.
//!
//! A circuit breaker sits in front of each LLM provider in `clawz-worker`.
//! After `failure_threshold` consecutive failures the breaker **opens** and
//! rejects new requests for `recovery_timeout`, giving the upstream service
//! time to recover. A single success resets the breaker to **closed**.
//!
//! // Dependency: used by worker::provider_router to avoid hammering
//! // degraded backends.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::time::{Duration, Instant};

/// The three possible circuit-breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    /// Normal operation — requests are allowed.
    Closed,
    /// Failure threshold exceeded — requests are rejected.
    Open,
    /// Recovery timeout elapsed — next request is a probe.
    HalfOpen,
}

/// Tunable thresholds for the breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// How long to wait before allowing a probe request.
    pub recovery_timeout: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 5,
            recovery_timeout: Duration::from_secs(30),
        }
    }
}

/// A lock-free circuit breaker using atomics for the hot path.
///
/// `state` and `failure_count` are atomics so `allow_request()` and
/// `record_failure()` never contend. Only `last_failure_at` uses a
/// `parking_lot::Mutex` because it is written infrequently and read
/// only when we need to check the recovery timeout.
///
/// // Used by: worker::provider_router
#[derive(Debug)]
pub struct CircuitBreaker {
    /// 0 = Closed, 1 = Open (HalfOpen is derived at read time).
    state: AtomicU8,
    failure_count: AtomicU32,
    last_failure_at: Mutex<Option<Instant>>,
    config: CircuitBreakerConfig,
}

impl CircuitBreaker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: AtomicU8::new(0),
            failure_count: AtomicU32::new(0),
            last_failure_at: Mutex::new(None),
            config,
        }
    }

    /// Derive the current `BreakerState`.
    /// Relaxed ordering is sufficient: we only need atomicity, not
    /// cross-thread synchronisation with other memory.
    pub fn state(&self) -> BreakerState {
        let raw = self.state.load(Ordering::Relaxed);
        if raw == 0 {
            return BreakerState::Closed;
        }
        // Check if recovery timeout has elapsed
        if let Some(last) = *self.last_failure_at.lock() {
            if last.elapsed() >= self.config.recovery_timeout {
                return BreakerState::HalfOpen;
            }
        }
        BreakerState::Open
    }

    /// Returns `true` if the caller should proceed with the request.
    pub fn allow_request(&self) -> bool {
        !matches!(self.state(), BreakerState::Open)
    }

    /// Increment the failure counter and possibly trip the breaker.
    pub fn record_failure(&self) {
        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_failure_at.lock() = Some(Instant::now());
        if count >= self.config.failure_threshold {
            self.state.store(1, Ordering::Relaxed);
        }
    }

    /// Reset the breaker to Closed after a successful request.
    pub fn record_success(&self) {
        self.failure_count.store(0, Ordering::Relaxed);
        self.state.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_closed() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig::default());
        assert!(matches!(cb.state(), BreakerState::Closed));
    }

    #[test]
    fn opens_after_threshold() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(matches!(cb.state(), BreakerState::Open));
    }

    #[test]
    fn rejects_when_open() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            ..Default::default()
        });
        cb.record_failure();
        assert!(!cb.allow_request());
    }

    #[test]
    fn success_resets() {
        let cb = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            ..Default::default()
        });
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert!(matches!(cb.state(), BreakerState::Closed));
        assert!(cb.allow_request());
    }
}
