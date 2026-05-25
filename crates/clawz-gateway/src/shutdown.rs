//! Graceful shutdown coordination for the Clawz Gateway.
//!
//! Provides a [`ShutdownCoordinator`] that orchestrates a clean shutdown
//! sequence across the gateway process. It uses [`tokio_util::sync::CancellationToken`]
//! to propagate shutdown signals to all concurrent tasks (HTTP server, WebSocket
//! handlers, background schedulers, fleet heartbeat loops, etc.).
//!
//! # Design
//! 1. Every long-lived task holds a cloned [`CancellationToken`].
//! 2. When the process receives `SIGTERM` / `SIGINT`, the coordinator calls
//!    [`ShutdownCoordinator::initiate`], which sets the `draining` flag and
//!    cancels the token.
//! 3. Tasks that are mid-request finish naturally; tasks awaiting the token
//!    wake up and begin their own teardown.
//! 4. The coordinator waits up to `drain_timeout` before forcing exit.
//!
//! # Cross-module dependencies
//! - [`crate::server::GatewayServer::serve`] — the HTTP server task should select
//!   on the cancellation token so it stops accepting new connections once
//!   draining begins.
//! - [`crate::ws`] — WebSocket background loops should listen to the token
//!   to close connections gracefully.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Coordinates graceful shutdown across all asynchronous components of the gateway.
///
/// [`ShutdownCoordinator`] is typically created once at startup and shared with
/// every subsystem that needs to participate in graceful teardown. It provides a
/// unified cancellation token and tracks whether the system is currently draining.
pub struct ShutdownCoordinator {
    /// Shared cancellation token cloned by every long-lived task.
    ///
    /// When [`initiate`](Self::initiate) is called this token is cancelled,
    /// causing all downstream `tokio::select!` branches to resolve.
    token: CancellationToken,
    /// Atomic flag indicating whether shutdown has been requested.
    ///
    /// Uses [`Ordering::Relaxed`] because the flag is only used for observability
    /// (metrics, logging) and does not guard critical memory safety invariants.
    draining: AtomicBool,
    /// Maximum duration to wait for in-flight requests and background tasks
    /// before forcing process exit.
    drain_timeout: Duration,
}

impl ShutdownCoordinator {
    /// Create a new coordinator with the specified drain timeout.
    ///
    /// # Arguments
    /// - `drain_timeout` — how long the process will wait for active work to finish
    ///   after shutdown is initiated. Should be tuned to the slowest endpoint
    ///   (e.g., long-running agent streaming responses).
    pub fn new(drain_timeout: Duration) -> Self {
        Self {
            token: CancellationToken::new(),
            draining: AtomicBool::new(false),
            drain_timeout,
        }
    }

    /// Return a cloned [`CancellationToken`] for use by a subsystem.
    ///
    /// Each caller gets an independent child token; cancelling the root token
    /// automatically cancels all clones.
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Query whether shutdown has been initiated.
    ///
    /// Useful for health-check endpoints that want to report `"draining"`
    /// while the process is shutting down.
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::Relaxed)
    }

    /// Initiate graceful shutdown.
    ///
    /// Sets `draining` to `true` and cancels the token, unblocking all tasks
    /// that are awaiting shutdown. This method is idempotent: multiple calls
    /// have no additional effect beyond the first.
    pub async fn initiate(&self) {
        self.draining.store(true, Ordering::Relaxed);
        self.token.cancel();
    }

    /// Return the configured drain timeout.
    ///
    /// Callers (e.g., the top-level `main` function) can use this value when
    /// setting up `tokio::time::timeout` around the final shutdown await.
    pub fn drain_timeout(&self) -> Duration {
        self.drain_timeout
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Verifies that cloning a token before initiation and then calling
    /// [`ShutdownCoordinator::initiate`] propagates cancellation to the clone.
    ///
    /// This is the core contract of the coordinator: every subsystem must see
    /// the cancellation signal regardless of when it cloned the token.
    #[tokio::test]
    async fn shutdown_signals_cancellation() {
        let coordinator = ShutdownCoordinator::new(Duration::from_secs(5));
        let token = coordinator.token();
        assert!(!token.is_cancelled());
        coordinator.initiate().await;
        assert!(token.is_cancelled());
    }

    /// Verifies that [`is_draining`](ShutdownCoordinator::is_draining) flips
    /// from `false` to `true` after [`initiate`](ShutdownCoordinator::initiate).
    #[tokio::test]
    async fn is_draining_after_initiate() {
        let coordinator = ShutdownCoordinator::new(Duration::from_secs(5));
        assert!(!coordinator.is_draining());
        coordinator.initiate().await;
        assert!(coordinator.is_draining());
    }
}
