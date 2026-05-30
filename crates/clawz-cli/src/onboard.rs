//! `clawz onboard` — first-run wizard + optional Docker stack.

use anyhow::{Context, Result};

use crate::config::{self, CliConfig};
use crate::gateway;

pub async fn run(install_daemon: bool) -> Result<()> {
    clawz_tui::run_onboarding();

    let mut cfg = CliConfig::default();
    if let Ok(port) = std::env::var("CLAWZ__SERVER__PORT") {
        if let Ok(p) = port.parse::<u16>() {
            cfg.gateway_url = format!("http://127.0.0.1:{p}");
        }
    }
    println!("\nSaving defaults to {}", config::config_path().display());
    config::save(&cfg).context("save cli.toml")?;

    if install_daemon {
        gateway::start().await?;
    } else {
        println!("\nNext: `clawz gateway start` (Docker) or `./scripts/install.sh`");
        println!("Then: `clawz doctor`");
    }
    Ok(())
}
