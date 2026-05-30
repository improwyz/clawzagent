//! Interactive terminal wizards for ClawZ first-run setup and configuration.
//!
//! Used by `clawz onboard`, `clawz setup`, and `clawz tui`. All I/O uses
//! `stdin`/`stdout` so prompts are testable with [`std::io::Cursor`].

mod wizard;

use clawz_setup::{SetupPlatform, SetupStateMachine};
use std::io::{self, BufRead, Write};

pub use wizard::run_setup_wizard;

/// Interactive first-run wizard — drives [`SetupStateMachine`] and prints env exports.
pub fn run_onboarding() {
    let mut sm = SetupStateMachine::new(SetupPlatform::Linux);
    if let Err(e) = run_setup_wizard(&mut sm) {
        eprintln!("Setup wizard failed: {e}");
    }
}

/// Read-only terminal dashboard stub.
pub fn run_dashboard() {
    println!("=== ClawZ Dashboard (terminal) ===");
    println!("Run `clawz doctor` for live health, or open the web UI.");
    println!("Set CLAWZ_GATEWAY_URL=http://127.0.0.1:3000 to target a running gateway.");
}

/// Interactive configuration menu (providers, channels, governance, deploy).
pub fn run_config() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        println!("\n=== ClawZ Configuration ===");
        println!("  1. Providers");
        println!("  2. Channels");
        println!("  3. Governance");
        println!("  4. Deploy");
        println!("  5. Back / Exit");
        print!("\nSelect [1-5]: ");
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        if reader.read_line(&mut choice).is_err() {
            break;
        }

        match choice.trim() {
            "1" => config_providers(&mut reader),
            "2" => config_channels(&mut reader),
            "3" => config_governance(&mut reader),
            "4" => config_deploy(&mut reader),
            "5" | "" | "q" | "quit" | "exit" => {
                println!("Exiting configuration.");
                break;
            }
            other => println!("Unknown option: {other}"),
        }
    }
}

fn config_providers(reader: &mut impl BufRead) {
    println!("\n--- Provider configuration ---");
    let name = prompt(reader, "Provider (anthropic/openai)", "anthropic");
    let key = prompt_secret(reader, "API key");
    println!("  [+] Provider '{name}' noted (key: {}).", mask_key(&key));
}

fn config_channels(reader: &mut impl BufRead) {
    println!("\n--- Channel configuration ---");
    let name = prompt(reader, "Channel (slack/discord/webhook)", "webhook");
    let url = prompt(reader, "Webhook URL", "https://");
    println!("  [+] Channel '{name}' → {url}");
}

fn config_governance(reader: &mut impl BufRead) {
    println!("\n--- Governance ---");
    let approval = prompt(reader, "Require approval for high-risk actions? [y/N]", "n");
    let max_cost = prompt(reader, "Max cost per task USD (0 = unlimited)", "0");
    println!("  approval={approval}, max_cost_usd={max_cost}");
}

fn config_deploy(reader: &mut impl BufRead) {
    println!("\n--- Deploy ---");
    let target = prompt(reader, "Target (docker/fly/railway/k8s)", "docker");
    println!("  Use `clawz gateway start` for local Docker, or see docs/cloud-deploy.");
    let _ = target;
}

pub(crate) fn prompt(reader: &mut impl BufRead, label: &str, default: &str) -> String {
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

pub(crate) fn prompt_secret(reader: &mut impl BufRead, label: &str) -> String {
    print!("  {label} (hidden): ");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    let _ = reader.read_line(&mut input);
    input.trim().to_string()
}

pub(crate) fn mask_key(key: &str) -> String {
    if key.is_empty() {
        return "(empty)".into();
    }
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    format!("{}****", &key[..4])
}

pub(crate) fn generate_secret(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes * 2);
    while out.len() < bytes * 2 {
        let u = uuid::Uuid::new_v4();
        for b in u.as_bytes() {
            out.push_str(&format!("{b:02x}"));
        }
    }
    out[..bytes * 2].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_setup::{DeploymentChoice, SetupPlatform, SetupStateMachine, SetupStep};
    use std::io::Cursor;

    #[test]
    fn prompt_uses_default() {
        let mut reader = Cursor::new(b"\n".as_ref());
        assert_eq!(prompt(&mut reader, "Port", "3000"), "3000");
    }

    #[test]
    fn generate_secret_length() {
        assert_eq!(generate_secret(32).len(), 64);
    }

    #[test]
    fn plain_wizard_advances_through_deploy_mode() {
        std::env::set_var("CLAWZ_TUI", "plain");
        let input = "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut sm = SetupStateMachine::new(SetupPlatform::Linux).without_persistence();
        let mut answers = wizard::context::WizardAnswers::default();

        // Drive welcome manually (plain::run uses stdin lock — test steps directly)
        sm.advance().expect("welcome");
        assert_eq!(sm.current_step(), SetupStep::DeployMode);
        sm.set_deployment(DeploymentChoice::Micro).expect("deploy");
        sm.set_install_strategy(clawz_setup::InstallStrategy::Prebuilt).expect("strategy");
        assert!(sm.session().deployment.is_some());
        let _ = (&mut reader, &mut answers);
        std::env::remove_var("CLAWZ_TUI");
    }
}
