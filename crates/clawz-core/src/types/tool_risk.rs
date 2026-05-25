//! Risk taxonomy and execution policy types for the Infrastructure dimension.
//!
//! These types classify tool actions by their fundamental primitive and risk level,
//! enabling the PRISM-G Infrastructure dimension to enforce appropriate
//! approval, retry, and composition policies.

use serde::{Deserialize, Serialize};

/// The fundamental action primitive that a tool performs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionPrimitive {
    Read,
    Write,
    Transform,
    Analyze,
    Notify,
    Execute,
    Decide,
    Wait,
}

/// Risk classification for a tool action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

impl RiskLevel {
    /// Returns the approval mode required for this risk level.
    pub fn approval(&self) -> ApprovalMode {
        match self {
            RiskLevel::Low => ApprovalMode::Automatic,
            RiskLevel::Medium => ApprovalMode::Logged,
            RiskLevel::High => ApprovalMode::HumanRequired,
        }
    }
}

/// How a tool action must be approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Automatic,
    Logged,
    HumanRequired,
}

/// Retry policy for a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Backoff,
    pub retryable_errors: Vec<String>,
}

/// Backoff strategy for retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backoff {
    Fixed { ms: u64 },
    Exponential { base_ms: u64, max_ms: u64 },
}

/// Composition strategy for multi-step tool workflows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Composition {
    Sequential,
    Parallel,
    Conditional,
    Iterative,
}
