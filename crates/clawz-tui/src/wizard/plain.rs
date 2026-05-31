//! Line-oriented setup wizard (`CLAWZ_TUI=plain` or non-TTY stdin).

use clawz_setup::{HostSpecChecker, Result, SetupEvent, SetupStateMachine, SetupStep};
use std::io::{self, BufRead, Write};

use super::context::WizardAnswers;
use super::{
    deployment_label, ensure_secrets, install_label, parse_deployment, parse_install_strategy,
};

pub fn run(sm: &mut SetupStateMachine, answers: &mut WizardAnswers) -> Result<()> {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    println!("=== ClawZ Setup Wizard ===\n");
    println!("Press Enter to accept defaults in [brackets].\n");

    answers.port = "3000".into();
    answers.log_level = "info".into();

    while sm.current_step() != SetupStep::Complete {
        match sm.current_step() {
            SetupStep::Welcome => step_welcome(sm, &mut reader)?,
            SetupStep::DeployMode => step_deploy_mode(sm, &mut reader)?,
            SetupStep::InstallStrategy => step_install_strategy(sm, &mut reader)?,
            SetupStep::Stack => {
                println!("\n--- Stack ---");
                println!("Dependencies will be installed when you apply this configuration.");
                sm.advance()?;
            }
            SetupStep::WriteSecrets => step_secrets(sm, answers, &mut reader)?,
            SetupStep::Llm => step_llm(sm, answers, &mut reader)?,
            SetupStep::AgentIdentity => step_identity(sm, answers, &mut reader)?,
            SetupStep::Skills => {
                println!("\n--- Skills ---");
                println!(
                    "Optional workspace skills can be added later under ~/.clawz/workspace/skills/"
                );
                sm.advance()?;
            }
            SetupStep::AgentTopology => {
                println!("\n--- Agent topology ---");
                if let Some(d) = sm.session().deployment {
                    println!("Using {d:?} deployment template.");
                }
                sm.advance()?;
            }
            SetupStep::Verify => {
                println!("\n--- Verify ---");
                let report = HostSpecChecker::collect();
                if report.warnings.is_empty() {
                    println!("Host checks passed.");
                } else {
                    println!("Warnings:");
                    for w in &report.warnings {
                        println!("  - {w}");
                    }
                }
                sm.advance()?;
            }
            SetupStep::Complete => break,
        }
    }

    sm.complete()?;
    Ok(())
}

fn step_welcome(sm: &mut SetupStateMachine, reader: &mut impl BufRead) -> Result<()> {
    println!("\n--- Welcome (step 0/10) ---");
    let report = HostSpecChecker::collect();
    println!("{}", report.summary);
    if !report.warnings.is_empty() {
        println!("\nWarnings:");
        for w in &report.warnings {
            println!("  - {w}");
        }
    }
    let _ = prompt(reader, "Continue", "");
    sm.advance()?;
    Ok(())
}

fn step_deploy_mode(sm: &mut SetupStateMachine, reader: &mut impl BufRead) -> Result<()> {
    println!("\n--- Deployment mode (step 1/10) ---");
    println!("  1. standalone — single binary, local SQLite/Postgres");
    println!("  2. micro      — Docker Compose fleet (recommended)");
    println!("  3. elastic    — full mesh, leader election");
    let raw = prompt(reader, "Choice", "micro");
    let choice = parse_deployment(&raw).ok_or_else(|| {
        clawz_setup::SetupError::InvalidTransition(format!("unknown deployment: {raw}"))
    })?;
    sm.set_deployment(choice)?;
    record_answer(
        sm,
        SetupStep::DeployMode,
        "deployment",
        deployment_label(choice),
    );
    sm.advance()?;
    Ok(())
}

fn step_install_strategy(sm: &mut SetupStateMachine, reader: &mut impl BufRead) -> Result<()> {
    println!("\n--- Install strategy (step 2/10) ---");
    println!("  1. prebuilt — pull images from GHCR (fast)");
    println!("  2. build    — docker compose build locally");
    println!("  3. source   — cargo build, no Docker");
    let raw = prompt(reader, "Choice", "prebuilt");
    let strategy = parse_install_strategy(&raw).ok_or_else(|| {
        clawz_setup::SetupError::InvalidTransition(format!("unknown strategy: {raw}"))
    })?;
    sm.set_install_strategy(strategy)?;
    record_answer(
        sm,
        SetupStep::InstallStrategy,
        "install_strategy",
        install_label(strategy),
    );
    sm.advance()?;
    Ok(())
}

fn step_secrets(
    sm: &mut SetupStateMachine,
    answers: &mut WizardAnswers,
    reader: &mut impl BufRead,
) -> Result<()> {
    println!("\n--- Secrets summary (step 4/10) ---");
    answers.jwt_secret = prompt_secret(reader, "JWT secret (blank = auto-generate)");
    answers.api_keys = prompt(reader, "API keys (comma-separated)", "dev-key");
    answers.worker_token = prompt_secret(reader, "Worker token (blank = auto-generate)");
    ensure_secrets(answers);
    println!("\nGenerated / confirmed secrets:");
    println!("  JWT:    {}", crate::mask_key(&answers.jwt_secret));
    println!("  API:    {}", crate::mask_key(&answers.api_keys));
    println!("  Worker: {}", crate::mask_key(&answers.worker_token));
    let confirm = prompt(reader, "Write these to .env on apply? [Y/n]", "y");
    if confirm.eq_ignore_ascii_case("n") {
        println!("Skipping .env write (export lines printed at end).");
    }
    sm.advance()?;
    Ok(())
}

fn step_llm(
    sm: &mut SetupStateMachine,
    answers: &mut WizardAnswers,
    reader: &mut impl BufRead,
) -> Result<()> {
    println!("\n--- LLM providers (step 5/10) ---");
    answers.anthropic_key = prompt(reader, "Anthropic API key (optional)", "");
    answers.openai_key = prompt(reader, "OpenAI API key (optional)", "");
    sm.advance()?;
    Ok(())
}

fn step_identity(
    sm: &mut SetupStateMachine,
    answers: &mut WizardAnswers,
    reader: &mut impl BufRead,
) -> Result<()> {
    println!("\n--- Agent identity (step 6/10) ---");
    answers.agent_name = prompt(reader, "Agent name", "clawz-assistant");
    answers.agent_who = prompt(reader, "Who am I? (one line)", "ClawZ setup assistant");
    answers.agent_role = prompt(reader, "Role", "help users install and operate ClawZ");
    record_answer(sm, SetupStep::AgentIdentity, "name", &answers.agent_name);
    sm.advance()?;
    Ok(())
}

fn record_answer(sm: &mut SetupStateMachine, step: SetupStep, field: &str, value: &str) {
    sm.session_mut().events.push(SetupEvent::UserAnswer {
        step,
        field: field.into(),
        value: value.into(),
    });
}

fn prompt(reader: &mut impl BufRead, label: &str, default: &str) -> String {
    if default.is_empty() {
        print!("  {label} : ");
    } else {
        print!("  {label} [{default}]: ");
    }
    io::stdout().flush().unwrap();
    let mut input = String::new();
    let _ = reader.read_line(&mut input);
    let trimmed = input.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn prompt_secret(reader: &mut impl BufRead, label: &str) -> String {
    print!("  {label} (hidden): ");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    let _ = reader.read_line(&mut input);
    input.trim().to_string()
}
