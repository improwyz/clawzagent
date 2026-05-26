//! RuntimeBackend trait — abstracts async executor across T0-T3 platforms.

use async_trait::async_trait;
use std::future::Future;
use std::io;
use std::time::{Duration, Instant};

/// Errors from RuntimeBackend operations
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Spawn failed: {0}")]
    SpawnError(String),
    #[error("Timeout")]
    Timeout,
    #[error("Backend unavailable: {0}")]
    Unavailable(String),
}

/// Abstracts the async executor so the same agent loop can run on
/// tokio (T2/T3), embassy (T0/T1 embedded), or smol (T1).
///
/// This is the single highest-leverage abstraction needed for platform
/// agnosticism — it allows the same `AgentScheduler` to operate on
/// any executor without changes to agent logic.
#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    /// Spawn a `Send`-safe async task.
    async fn spawn(&self, task: impl Future<Output = ()> + Send + 'static);

    /// Spawn a blocking synchronous task (e.g., CPU-bound computation).
    async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static);

    /// Sleep for the given duration.
    async fn sleep(&self, duration: Duration);

    /// Get the current instant (monotonic clock).
    fn now(&self) -> Instant;

    /// Try to read from a file descriptor (for embedded UART/I2C).
    /// On std platforms this bridges to `read(2)`.
    /// On no_std platforms returns `io::ErrorKind::Unsupported` unless
    /// a platform-specific implementation is available.
    fn try_read_fd(&self, fd: i32, buf: &mut [u8]) -> Result<usize, io::Error>;
}