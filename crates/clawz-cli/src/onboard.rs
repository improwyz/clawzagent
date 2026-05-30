//! `clawz onboard` — setup wizard via [`clawz_setup::SetupStateMachine`].

use anyhow::{anyhow, Context, Result};
use clawz_setup::{
    ensure_setup_dir, prompts, HostSpecChecker, SetupError, SetupPlatform, SetupStateMachine,
    SetupStep,
};

use clawz_setup::DeploymentChoice;

use crate::config::{self, CliConfig};
use crate::setup_cmd::{self, SetupStackOptions};

#[derive(Debug, Clone, Copy)]
pub struct OnboardOptions {
    pub resume: bool,
    pub json: bool,
    pub step: Option<u8>,
    pub install_daemon: bool,
}

pub async fn run(opts: OnboardOptions) -> Result<()> {
    ensure_setup_dir().map_err(setup_err)?;

    let platform = detect_platform();
    let mut sm = if opts.resume {
        SetupStateMachine::resume(platform)
            .map_err(setup_err)?
            .ok_or_else(|| anyhow!("no saved setup session — run without --resume"))?
    } else {
        SetupStateMachine::new(platform)
    };

    if let Some(phase) = opts.step {
        let step = SetupStep::from_phase(phase)
            .ok_or_else(|| anyhow!("invalid setup step {phase} (expected 0–10)"))?;
        sm.go_to(step).map_err(setup_err)?;
    }

    if opts.json {
        return run_json_onboard(&mut sm, opts).await;
    }

    clawz_tui::run_setup_wizard(&mut sm).map_err(setup_err)?;

    finish_cli_config(opts.install_daemon).await
}

async fn run_json_onboard(sm: &mut SetupStateMachine, opts: OnboardOptions) -> Result<()> {
    let report = HostSpecChecker::collect();
    match sm.current_step() {
        SetupStep::Welcome => {
            println!("{}", serde_json::to_string_pretty(&report)?);
            sm.advance().map_err(setup_err)?;
        }
        SetupStep::DeployMode if sm.session().deployment.is_none() => {
            return Err(anyhow!(
                "deployment not set — run interactively or resume a session with choices"
            ));
        }
        SetupStep::InstallStrategy if sm.session().install_strategy.is_none() => {
            return Err(anyhow!(
                "install strategy not set — run interactively or resume a saved session"
            ));
        }
        SetupStep::Complete => {}
        _ => {}
    }

    if sm.current_step() != SetupStep::Complete {
        if let Some(deployment) = sm.session().deployment {
            let rec = prompts::EnvRecommendations::from_session(deployment, None);
            println!("{}", serde_json::to_string_pretty(&rec)?);
            sm.complete().map_err(setup_err)?;
        }
    }

    println!("{}", sm.export_json().map_err(setup_err)?);
    finish_cli_config(opts.install_daemon).await
}

async fn finish_cli_config(install_daemon: bool) -> Result<()> {
    let mut cfg = CliConfig::default();
    if let Ok(port) = std::env::var("CLAWZ__SERVER__PORT") {
        if let Ok(p) = port.parse::<u16>() {
            cfg.gateway_url = format!("http://127.0.0.1:{p}");
        }
    }
    println!("\nSaving defaults to {}", config::config_path().display());
    config::save(&cfg).context("save cli.toml")?;

    if install_daemon {
        let deployment = clawz_setup::load_session()
            .ok()
            .and_then(|s| s.deployment)
            .unwrap_or(DeploymentChoice::Micro);
        let build = clawz_setup::load_session()
            .ok()
            .and_then(|s| s.install_strategy)
            .is_some_and(|s| matches!(s, clawz_setup::InstallStrategy::Build));
        setup_cmd::run_stack(SetupStackOptions {
            build,
            with_web: false,
            dry_run: false,
            deployment: Some(deployment),
        })?;
    } else {
        println!("\nNext: `clawz setup stack` or `./scripts/install.sh --docker`");
        println!("Then: `clawz doctor`");
    }
    Ok(())
}

fn detect_platform() -> SetupPlatform {
    if cfg!(target_os = "linux") {
        SetupPlatform::Linux
    } else if cfg!(target_os = "macos") {
        SetupPlatform::MacOs
    } else if cfg!(target_os = "windows") {
        SetupPlatform::Windows
    } else {
        SetupPlatform::Unknown
    }
}

fn setup_err(e: SetupError) -> anyhow::Error {
    anyhow!("{e}")
}
