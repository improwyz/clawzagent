//! Canonical error type for the entire ClawZ platform.
//!
//! Every subsystem returns `Result<T>` which resolves to `ClawzError` on failure.
//! This module also provides `From` impls for common external error types
//! (serde_json, sqlx, reqwest, …) so `?` works seamlessly across crate
//! boundaries.
//!
//! // Dependency: used by every module in core, worker, and gateway.

use thiserror::Error;

/// The canonical result type for all clawz-core operations.
pub type Result<T, E = ClawzError> = std::result::Result<T, E>;

/// Comprehensive error enum covering every subsystem in the ClawZ platform.
/// Each variant carries a human-readable message suitable for logs and
/// API error responses.
/// // Used by: worker::scheduler, gateway::handlers, db::repos
#[derive(Debug, Error)]
pub enum ClawzError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("provider error: {0}")]
    Provider(String),

    #[error("channel error: {0}")]
    Channel(String),

    #[error("tool error: {0}")]
    Tool(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("governance error: {0}")]
    Governance(String),

    #[error("authentication error: {0}")]
    Auth(String),

    #[error("budget error: {0}")]
    Budget(String),

    #[error("orchestration error: {0}")]
    Orchestration(String),

    #[error("system is shutting down")]
    ShuttingDown,

    #[error("rate limited — retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },

    #[error("{entity} not found: {id}")]
    NotFound { entity: String, id: String },

    #[error("validation error: {0}")]
    Validation(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("mesh error: {0}")]
    Mesh(String),

    #[error("memory error: {0}")]
    Memory(String),

    #[error("hardware error: {0}")]
    Hardware(String),

    #[error("deploy error: {0}")]
    Deploy(String),
}

// ── From impls ────────────────────────────────────────────────────────────────

// Map serde_json errors to Serialization so callers don't need manual conversions.
impl From<serde_json::Error> for ClawzError {
    fn from(e: serde_json::Error) -> Self {
        ClawzError::Serialization(e.to_string())
    }
}

// TOML deserialization failures are treated as Config errors because they
// usually mean a malformed `clawz.toml`.
impl From<toml::de::Error> for ClawzError {
    fn from(e: toml::de::Error) -> Self {
        ClawzError::Config(e.to_string())
    }
}

impl From<toml::ser::Error> for ClawzError {
    fn from(e: toml::ser::Error) -> Self {
        ClawzError::Serialization(e.to_string())
    }
}

// Distinguish HTTP 429 (rate limit) from generic transport errors so
/// the worker can apply backoff logic.
impl From<reqwest::Error> for ClawzError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_status() {
            if let Some(status) = e.status() {
                if status.as_u16() == 429 {
                    return ClawzError::RateLimited { retry_after_secs: 60 };
                }
            }
        }
        ClawzError::Transport(e.to_string())
    }
}

// sqlx::Error::RowNotFound is a very common case — map it to our NotFound
// variant so repository callers get a structured error instead of a raw
// database string.
impl From<sqlx::Error> for ClawzError {
    fn from(e: sqlx::Error) -> Self {
        match &e {
            sqlx::Error::RowNotFound => ClawzError::NotFound {
                entity: "row".into(),
                id: "unknown".into(),
            },
            _ => ClawzError::Database(e.to_string()),
        }
    }
}

impl From<uuid::Error> for ClawzError {
    fn from(e: uuid::Error) -> Self {
        ClawzError::Validation(format!("invalid UUID: {e}"))
    }
}

impl From<base64::DecodeError> for ClawzError {
    fn from(e: base64::DecodeError) -> Self {
        ClawzError::Serialization(format!("base64 decode error: {e}"))
    }
}
