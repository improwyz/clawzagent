//! ClawZ Runtime Backend Implementations
//!
//! Provides pluggable async executor implementations.

pub mod tokio_backend;
pub mod tokio_st_backend;

// Re-exports: TokioBackend, TokioSingleThreadBackend are defined in this module.
// TokioBackend is exported via tokio_backend module's impl block on crate::TokioBackend.
// RuntimeBackend trait is defined in this module.

use async_trait::async_trait;
use std::future::Future;
use std::io;
use std::time::{Duration, Instant};

/// Tokio-backed RuntimeBackend — for T2/T3 (multi-threaded).
pub struct TokioBackend {
    _priv: (),
}

impl TokioBackend {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for TokioBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Single-threaded RuntimeBackend — for T1 (SBC, constrained).
pub struct TokioSingleThreadBackend {
    _priv: (),
}

impl TokioSingleThreadBackend {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

impl Default for TokioSingleThreadBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn spawn(&self, task: impl Future<Output = ()> + Send + 'static);
    async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static);
    async fn sleep(&self, duration: Duration);
    fn now(&self) -> Instant;
    fn try_read_fd(&self, fd: i32, buf: &mut [u8]) -> Result<usize, io::Error>;
}