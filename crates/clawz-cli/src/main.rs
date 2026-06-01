//! ClawZ operator CLI — `clawz onboard`, `clawz doctor`, `clawz agent`, …

use anyhow::Result;
use clap::{Parser, Subcommand};

mod agent;
mod client;
mod config;
mod cron;
mod doctor;
mod gateway;
mod onboard;
mod setup_cmd;

#[derive(Parser)]
#[command(
    name = "clawz",
    version,
    about = "ClawZ operator CLI — onboard, health checks, gateway control"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Run in headless mode (plain text, no TUI).
    #[arg(long, global = true)]
    headless: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive first-run setup (recommended entry point).
    Onboard {
        /// Start Docker Compose stack after the wizard.
        #[arg(long)]
        install_daemon: bool,
        /// Resume from `~/.clawz/setup/session.json`.
        #[arg(long)]
        resume: bool,
        /// Machine-readable session / host-spec output.
        #[arg(long)]
        json: bool,
        /// Jump to setup phase 0–10 (see clawz-setup `SetupStep`).
        #[arg(long)]
        step: Option<u8>,
    },
    /// Same as onboard configuration menu.
    Setup {
        #[command(subcommand)]
        action: Option<SetupAction>,
    },
    /// Check gateway, worker, env, and Docker.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Gateway process control (Docker Compose).
    Gateway {
        #[command(subcommand)]
        action: GatewayAction,
    },
    /// Send one message to an agent via the gateway API.
    Agent {
        #[arg(short, long)]
        message: String,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        conversation_id: Option<String>,
    },
    /// Terminal UI (onboarding, config editor, dashboard stub).
    Tui {
        #[command(subcommand)]
        action: Option<TuiAction>,
    },
    /// Scheduled agent jobs (list, add, run, remove).
    Cron {
        #[command(subcommand)]
        action: cron::CronAction,
    },
}

#[derive(Subcommand)]
enum SetupAction {
    /// Install curl, git, Docker (and optional Node for web).
    Deps {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        with_web: bool,
    },
    /// Start micro Compose stack (prebuilt GHCR by default).
    Stack {
        #[arg(long)]
        build: bool,
        #[arg(long)]
        with_web: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum GatewayAction {
    Status,
    Start,
    Stop,
}

#[derive(Subcommand)]
enum TuiAction {
    Onboard,
    Config,
    Dashboard,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // No subcommand → launch the unified TUI installer
    let Some(command) = cli.command else {
        if cli.headless {
            std::env::set_var("CLAWZ_TUI", "plain");
        }
        if let Err(e) = clawz_tui::run() {
            eprintln!("TUI error: {e}");
            std::process::exit(1);
        }
        return Ok(());
    };

    match command {
        Commands::Onboard {
            install_daemon,
            resume,
            json,
            step,
        } => {
            onboard::run(onboard::OnboardOptions {
                resume,
                json,
                step,
                install_daemon,
            })
            .await?
        }
        Commands::Setup { action } => match action {
            None => clawz_tui::run_config(),
            Some(SetupAction::Deps { dry_run, with_web }) => {
                setup_cmd::run_deps(setup_cmd::SetupDepsOptions { dry_run, with_web })?
            }
            Some(SetupAction::Stack {
                build,
                with_web,
                dry_run,
            }) => setup_cmd::run_stack(setup_cmd::SetupStackOptions {
                build,
                with_web,
                dry_run,
                deployment: None,
            })?,
        },
        Commands::Doctor { json } => doctor::run(json).await?,
        Commands::Gateway { action } => match action {
            GatewayAction::Status => gateway::status().await?,
            GatewayAction::Start => gateway::start().await?,
            GatewayAction::Stop => gateway::stop().await?,
        },
        Commands::Agent {
            message,
            agent_id,
            conversation_id,
        } => agent::run(&message, agent_id.as_deref(), conversation_id.as_deref()).await?,
        Commands::Tui { action } => match action {
            None | Some(TuiAction::Onboard) => clawz_tui::run_onboarding(),
            Some(TuiAction::Config) => clawz_tui::run_config(),
            Some(TuiAction::Dashboard) => clawz_tui::run_dashboard(),
        },
        Commands::Cron { action } => cron::run(action).await?,
    }
    Ok(())
}
