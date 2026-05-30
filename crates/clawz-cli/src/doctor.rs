//! `clawz doctor` — environment and connectivity checks.

use anyhow::Result;
use serde::Serialize;

use crate::client::{worker_health, GatewayClient};
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
    let report = collect(&cfg).await;
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

async fn collect(cfg: &CliConfig) -> DoctorReport {
    let mut checks = Vec::new();

    checks.push(env_check(
        "CLAWZ_MODE",
        std::env::var("CLAWZ_MODE").ok(),
        false,
    ));
    checks.push(env_check(
        "VALID_API_KEYS or CLAWZ_DISABLE_AUTH",
        std::env::var("VALID_API_KEYS")
            .ok()
            .or_else(|| {
                if std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1") {
                    Some("auth disabled".to_string())
                } else {
                    None
                }
            }),
        false,
    ));
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

    let gw = GatewayClient::new(cfg);
    match gw.system_health().await {
        Ok(v) => {
            let status = v
                .get("status")
                .and_then(|s| s.as_str())
                .unwrap_or("unknown");
            checks.push(Check {
                name: "gateway".into(),
                ok: true,
                detail: format!("{} — {}", cfg.gateway_url, status),
            });
        }
        Err(e) => checks.push(Check {
            name: "gateway".into(),
            ok: false,
            detail: format!("{} — {e}", cfg.gateway_url),
        }),
    }

    match worker_health(&cfg.worker_url).await {
        Ok(v) => {
            let svc = v
                .get("service")
                .and_then(|s| s.as_str())
                .unwrap_or("worker");
            checks.push(Check {
                name: "worker".into(),
                ok: true,
                detail: format!("{} — {svc}", cfg.worker_url),
            });
        }
        Err(e) => checks.push(Check {
            name: "worker".into(),
            ok: false,
            detail: format!("{} — {e}", cfg.worker_url),
        }),
    }

    checks.push(docker_check().await);

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

async fn docker_check() -> Check {
    let output = tokio::process::Command::new("docker")
        .args(["compose", "ps", "--format", "json"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let n = String::from_utf8_lossy(&out.stdout).lines().count();
            Check {
                name: "docker compose".into(),
                ok: true,
                detail: format!("{n} service line(s) reported"),
            }
        }
        Ok(out) => Check {
            name: "docker compose".into(),
            ok: false,
            detail: format!(
                "exit {} — {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        },
        Err(e) => Check {
            name: "docker compose".into(),
            ok: true,
            detail: format!("skipped ({e})"),
        },
    }
}
