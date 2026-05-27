//! Type re-exports for the `clawz_core::types` module.
//!
//! All submodules are flattened here so downstream crates can write
//! `use clawz_core::types::AgentConfig` instead of drilling into
//! `clawz_core::types::agent::AgentConfig`.
//!
//! // Dependency: imported by traits.rs, db.rs, worker, and gateway.

pub mod agent;
pub mod channel;
pub mod cost;
pub mod deploy;
pub mod governance;
pub mod mesh;
pub mod message;
pub mod orchestration;
pub mod purpose;
pub mod reality;
pub mod room;
pub mod tenant;
pub mod tool;
pub mod tool_risk;

// Explicit re-exports to avoid ambiguous glob conflicts (Rust issue #114095).
// message::Role and tenant::Role both exist — only message::Role is exported
// as the canonical "Role" to avoid breaking existing code.
pub use agent::*;
pub use channel::*;
pub use cost::*;
pub use deploy::*;
pub use governance::*;
pub use mesh::*;
pub use message::*;
pub use orchestration::*;
pub use purpose::*;
pub use reality::*;
pub use room::*;
// tenant exports — rename Role to avoid collision with message::Role
pub use tenant::BudgetLease;
pub use tenant::NetworkScope;
pub use tenant::PermissionSet;
pub use tenant::TenantContext;
pub use tenant::TenantId;
// Re-export tenant::Role under a non-ambiguous name
pub use tenant::Role as TenantRole;
pub use tool::*;
pub use tool_risk::*;
