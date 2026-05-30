//! ClawZ install & onboarding wizard — shared setup engine.
//!
//! Consumed by `clawz onboard`, gateway setup API, and web/TUI hosts.
//! Persists resumable state at `~/.clawz/setup/session.json` and
//! `setup_complete` in `~/.clawz/config.json`.

mod bootstrap;
mod config;
pub mod doctor;
mod deploy;
mod error;
mod paths;
pub mod prompts;
mod session;
mod spec;
mod state_machine;
mod tools;
mod types;

pub use bootstrap::{AgentBootstrap, AgentConfigPayload, IdentityArtifacts, IdentityInput};
pub use config::ClawzUserConfig;
pub use deploy::{
    ComposeOverlay, DeployPlan, DeployPlanError, DeployPlanner, StandaloneSourcePlan,
};
pub use doctor::{
    doctor_fix_tools, run_doctor, DoctorCheck, DoctorConfig, DoctorReport,
};
pub use error::{Result, SetupError};
pub use paths::{clawz_home, session_path, setup_dir, user_config_path};
pub use session::{
    ensure_setup_dir, load_session, load_session_from, save_session, save_session_to,
    session_to_json,
};
pub use spec::{HostSpecChecker, HostSpecReport, CLAWZ_MIN_RUST};
pub use state_machine::SetupStateMachine;
pub use tools::{
    ConfirmGate, InitWorkspaceTool, SetupTool, SetupToolRegistry, SpecCheckTool, ToolContext,
    ToolInput, ToolResult, WriteEnvTool,
};
pub use tools::{init_workspace_at, workspace_root};
pub use types::{
    DeploymentChoice, InstallStrategy, SetupArtifact, SetupEvent, SetupPlatform, SetupSession,
    SetupStep,
};
