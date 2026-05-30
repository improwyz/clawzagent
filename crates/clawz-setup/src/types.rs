//! Core setup wizard domain types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Host platform running the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SetupPlatform {
    Linux,
    MacOs,
    Windows,
    Web,
    Mobile,
    #[default]
    Unknown,
}

/// Deployment topology selected during onboarding (phase 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeploymentChoice {
    Standalone,
    Micro,
    Elastic,
}

/// Image / install strategy (phase 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStrategy {
    Prebuilt,
    Build,
    Source,
}

/// Wizard phases 0–10 (see design spec §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum SetupStep {
    Welcome = 0,
    DeployMode = 1,
    InstallStrategy = 2,
    Stack = 3,
    WriteSecrets = 4,
    Llm = 5,
    AgentIdentity = 6,
    Skills = 7,
    AgentTopology = 8,
    Verify = 9,
    Complete = 10,
}

impl SetupStep {
    pub const FIRST: Self = Self::Welcome;
    pub const LAST: Self = Self::Complete;

    pub fn from_phase(phase: u8) -> Option<Self> {
        match phase {
            0 => Some(Self::Welcome),
            1 => Some(Self::DeployMode),
            2 => Some(Self::InstallStrategy),
            3 => Some(Self::Stack),
            4 => Some(Self::WriteSecrets),
            5 => Some(Self::Llm),
            6 => Some(Self::AgentIdentity),
            7 => Some(Self::Skills),
            8 => Some(Self::AgentTopology),
            9 => Some(Self::Verify),
            10 => Some(Self::Complete),
            _ => None,
        }
    }

    pub fn phase(self) -> u8 {
        self as u8
    }

    pub fn next(self) -> Option<Self> {
        Self::from_phase(self.phase().saturating_add(1))
    }

    pub fn requires_deploy_mode(self) -> bool {
        self.phase() >= Self::WriteSecrets.phase()
    }
}

/// Events emitted during the wizard (user input, tool results, doctor outcomes).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupEvent {
    StepEntered { step: SetupStep },
    DeploymentChosen { choice: DeploymentChoice },
    InstallStrategyChosen { strategy: InstallStrategy },
    UserAnswer { step: SetupStep, field: String, value: String },
    DoctorResult { passed: bool, summary: String },
    ToolResult { tool: String, ok: bool, message: String },
    Aborted { reason: Option<String> },
    Completed,
}

/// Artifacts produced by setup tools (`.env` patches, agent payloads, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SetupArtifact {
    EnvPatch { keys: Vec<String> },
    CliToml { path: String },
    AgentsMd { path: String },
    ProviderConfig { provider: String },
    AgentCreatePayload { name: String },
}

/// Resumable onboarding session persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupSession {
    pub id: Uuid,
    pub platform: SetupPlatform,
    pub current_step: SetupStep,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_strategy: Option<InstallStrategy>,
    pub aborted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<SetupEvent>,
}

impl SetupSession {
    pub fn new(platform: SetupPlatform) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            platform,
            current_step: SetupStep::Welcome,
            deployment: None,
            install_strategy: None,
            aborted: false,
            created_at: now,
            updated_at: now,
            events: Vec::new(),
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn deploy_mode_chosen(&self) -> bool {
        self.deployment.is_some()
    }
}
