//! `clawz doctor` — environment and connectivity checks.

use anyhow::Result;
use serde::Serialize;

use crate::config::{self, CliConfig};

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

pub struct DoctorReport {
    pub checks: Vec<Check>,
}

impl DoctorReport {
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }

    pub fn print_human(&self) {
        println!("ClawZ doctor\n");
        for c in &self.checks {
            let mark = if c.ok { "ok" } else { "FAIL" };
            println!("  [{mark:4}] {} — {}", c.name, c.detail);
        }
        println!();
        if self.all_ok() {
            println!("All checks passed.");
        } else {
            println!("Some checks failed. Run `clawz onboard` or see INSTALL.md.");
        }
    }
}

pub async fn run(json: bool) -> Result<()> {
    let cfg = config::resolve();
    let report = collect(&cfg);
    if json {
        println!("{}", serde_json::to_string_pretty(&report.checks)?);
    } else {
        report.print_human();
    }
    if !report.all_ok() {
        std::process::exit(1);
    }
    Ok(())
}

fn collect(cfg: &CliConfig) -> DoctorReport {
    let setup = clawz_setup::run_doctor(&clawz_setup::DoctorConfig {
        gateway_url: cfg.gateway_url.clone(),
        worker_url: cfg.worker_url.clone(),
        check_docker: true,
    });

    let mut checks: Vec<Check> = setup
        .checks
        .into_iter()
        .map(|c| Check {
            name: c.name,
            ok: c.ok,
            detail: c.detail,
        })
        .collect();

    checks.push(env_check(
        "CLAWZ_JWT_SECRET",
        std::env::var("CLAWZ_JWT_SECRET").ok(),
        false,
    ));

    let path = config::config_path();
    checks.push(Check {
        name: "cli.toml".into(),
        ok: true,
        detail: if path.exists() {
            path.display().to_string()
        } else {
            format!("optional — not found at {} (defaults apply)", path.display())
        },
    });

    DoctorReport { checks }
}

fn env_check(name: &str, value: Option<String>, required: bool) -> Check {
    let ok = value.is_some() || !required;
    Check {
        name: name.to_string(),
        ok,
        detail: value.unwrap_or_else(|| {
            if required {
                "not set".into()
            } else {
                "optional — not set".into()
            }
        }),
    }
}
