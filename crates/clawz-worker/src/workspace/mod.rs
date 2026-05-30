//! Operator workspace — `AGENTS.md`, `SOUL.md`, and skill directories.

pub mod loader;
pub mod skill_curator;

pub use loader::{SkillEntry, WorkspaceLoader, WorkspaceSnapshot};
pub use skill_curator::SkillCurator;
