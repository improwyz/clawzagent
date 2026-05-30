//! Setup wizard driven by [`SetupStateMachine`] (phases 0–10).

mod fancy;
mod plain;

use clawz_setup::{DeploymentChoice, InstallStrategy, Result, SetupStateMachine};
use std::io::{self, IsTerminal};

use crate::wizard::context::WizardAnswers;

pub mod context;

/// `true` when stdin is not a TTY or `CLAWZ_TUI=plain`.
pub fn use_plain_ui() -> bool {
    if std::env::var("CLAWZ_TUI")
        .map(|v| v.eq_ignore_ascii_case("plain"))
        .unwrap_or(false)
    {
        return true;
    }
    !io::stdin().is_terminal()
}

/// Run the multi-step setup wizard (plain prompts or ratatui).
pub fn run_setup_wizard(sm: &mut SetupStateMachine) -> Result<()> {
    let mut answers = WizardAnswers::default();
    if use_plain_ui() {
        plain::run(sm, &mut answers)?;
    } else {
        fancy::run(sm, &mut answers)?;
    }
    answers.print_env_exports();
    Ok(())
}

pub(crate) fn parse_deployment(s: &str) -> Option<DeploymentChoice> {
    match s.trim().to_lowercase().as_str() {
        "standalone" | "1" | "s" => Some(DeploymentChoice::Standalone),
        "micro" | "2" | "m" => Some(DeploymentChoice::Micro),
        "elastic" | "3" | "e" => Some(DeploymentChoice::Elastic),
        _ => None,
    }
}

pub(crate) fn parse_install_strategy(s: &str) -> Option<InstallStrategy> {
    match s.trim().to_lowercase().as_str() {
        "prebuilt" | "pull" | "1" | "p" => Some(InstallStrategy::Prebuilt),
        "build" | "local" | "2" | "b" => Some(InstallStrategy::Build),
        "source" | "cargo" | "3" => Some(InstallStrategy::Source),
        _ => None,
    }
}

pub(crate) fn deployment_label(c: DeploymentChoice) -> &'static str {
    match c {
        DeploymentChoice::Standalone => "standalone",
        DeploymentChoice::Micro => "micro",
        DeploymentChoice::Elastic => "elastic",
    }
}

pub(crate) fn install_label(s: InstallStrategy) -> &'static str {
    match s {
        InstallStrategy::Prebuilt => "prebuilt",
        InstallStrategy::Build => "build",
        InstallStrategy::Source => "source",
    }
}

pub(crate) fn ensure_secrets(answers: &mut WizardAnswers) {
    if answers.jwt_secret.is_empty() {
        answers.jwt_secret = crate::generate_secret(32);
    }
    if answers.worker_token.is_empty() {
        answers.worker_token = crate::generate_secret(24);
    }
    if answers.api_keys.is_empty() {
        answers.api_keys = "dev-key".to_string();
    }
}
