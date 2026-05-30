//! ClawZ install & onboarding wizard — shared setup engine.
//!
//! Consumed by `clawz onboard`, gateway setup API, and web/TUI hosts.
//! Persists resumable state at `~/.clawz/setup/session.json` and
//! `setup_complete` in `~/.clawz/config.json`.

mod config;
mod error;
mod paths;
mod session;
mod state_machine;
mod types;

pub use config::ClawzUserConfig;
pub use error::{Result, SetupError};
pub use paths::{clawz_home, session_path, setup_dir, user_config_path};
pub use session::{
    ensure_setup_dir, load_session, load_session_from, save_session, save_session_to,
    session_to_json,
};
pub use state_machine::SetupStateMachine;
pub use types::{
    DeploymentChoice, InstallStrategy, SetupArtifact, SetupEvent, SetupPlatform, SetupSession,
    SetupStep,
};
