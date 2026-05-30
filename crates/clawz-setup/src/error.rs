//! Setup wizard error types.

use thiserror::Error;

pub type Result<T, E = SetupError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum SetupError {
    #[error("invalid setup step: {0}")]
    InvalidStep(u8),

    #[error("transition not allowed: {0}")]
    InvalidTransition(String),

    #[error("deployment mode must be chosen before step {step:?}")]
    DeployModeRequired { step: String },

    #[error("session was aborted")]
    Aborted,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("session not found at {path}")]
    SessionNotFound { path: String },

    #[error("{0}")]
    Internal(String),
}

impl SetupError {
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }
}

impl From<serde_json::Error> for SetupError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}
