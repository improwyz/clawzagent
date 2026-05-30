//! Shared environment and connectivity checks for `clawz doctor` and the setup wizard.

use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::ClawzUserConfig;
use crate::spec::HostSpecChecker;
use crate::HostSpecReport;

/// Configuration for [`run_doctor`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DoctorConfig {
    /// Gateway origin (e.g. `http://127.0.0.1:3000`). Empty skips gateway HTTP check.
    pub gateway_url: String,
    /// Worker control API origin. Empty skips worker HTTP check.
    pub worker_url: String,
    /// When true, run `docker compose ps`.
    pub check_docker: bool,
}

/// A single doctor check result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

/// Aggregated doctor output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// True when every check passed.
    pub fn all_ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
}

/// Run all configured doctor checks (blocking).
pub fn run_doctor(config: &DoctorConfig) -> DoctorReport {
    let mut checks = vec![
        host_spec_check(),
        setup_complete_check(),
        clawz_mode_check(),
        api_keys_check(),
    ];

    if !config.gateway_url.trim().is_empty() {
        checks.push(gateway_health_check(&config.gateway_url));
    }
    if !config.worker_url.trim().is_empty() {
        checks.push(worker_health_check(&config.worker_url));
    }
    if config.check_docker {
        checks.push(docker_compose_check());
    }

    DoctorReport { checks }
}

/// `doctor --fix` stub: suggested wizard tool names for failed checks (no execution).
pub fn doctor_fix_tools(report: &DoctorReport) -> Vec<&'static str> {
    let mut tools = Vec::new();
    for check in &report.checks {
        if check.ok {
            continue;
        }
        let suggested = match check.name.as_str() {
            "host spec" => &["spec_check", "install_deps"][..],
            "CLAWZ_MODE" => &["write_env"],
            "VALID_API_KEYS or CLAWZ_DISABLE_AUTH" => &["write_env"],
            "gateway" | "worker" => &["compose_up", "doctor_run"],
            "docker compose" => &["compose_up"],
            _ => &[],
        };
        for tool in suggested {
            if !tools.contains(tool) {
                tools.push(*tool);
            }
        }
    }
    tools
}

/// JSON-friendly host report for `clawz onboard --json`.
pub fn host_spec_json() -> Result<String, serde_json::Error> {
    let report = HostSpecChecker::collect();
    serde_json::to_string_pretty(&report)
}

pub fn host_spec_report() -> HostSpecReport {
    HostSpecChecker::collect()
}

fn host_spec_check() -> DoctorCheck {
    let spec = HostSpecChecker::collect();
    let ok = spec.warnings.is_empty();
    let mut detail = spec.summary.clone();
    if !spec.warnings.is_empty() {
        detail.push_str(" · warnings: ");
        detail.push_str(&spec.warnings.join("; "));
    }
    DoctorCheck {
        name: "host spec".into(),
        ok,
        detail,
        remediation: if ok {
            None
        } else {
            Some("Run install-deps.sh or `clawz onboard` dependency phase.".into())
        },
    }
}

fn setup_complete_check() -> DoctorCheck {
    let user_cfg = ClawzUserConfig::load().unwrap_or_default();
    DoctorCheck {
        name: "setup_complete".into(),
        ok: user_cfg.setup_complete,
        detail: if user_cfg.setup_complete {
            format!("true ({})", ClawzUserConfig::path().display())
        } else {
            "false — run `clawz onboard`".into()
        },
        remediation: if user_cfg.setup_complete {
            None
        } else {
            Some("Run `clawz onboard` to finish first-run setup.".into())
        },
    }
}

fn clawz_mode_check() -> DoctorCheck {
    env_check(
        "CLAWZ_MODE",
        std::env::var("CLAWZ_MODE").ok(),
        false,
        Some("Set CLAWZ_MODE=standalone|micro|elastic in .env or shell."),
    )
}

fn api_keys_check() -> DoctorCheck {
    let value = std::env::var("VALID_API_KEYS").ok().or_else(|| {
        if std::env::var("CLAWZ_DISABLE_AUTH").ok().as_deref() == Some("1") {
            Some("auth disabled (CLAWZ_DISABLE_AUTH=1)".to_string())
        } else {
            None
        }
    });
    env_check(
        "VALID_API_KEYS or CLAWZ_DISABLE_AUTH",
        value,
        false,
        Some("Set VALID_API_KEYS or CLAWZ_DISABLE_AUTH=1 for local dev."),
    )
}

fn env_check(
    name: &str,
    value: Option<String>,
    required: bool,
    remediation: Option<&str>,
) -> DoctorCheck {
    let ok = value.is_some() || !required;
    DoctorCheck {
        name: name.to_string(),
        ok,
        detail: value.unwrap_or_else(|| {
            if required {
                "not set".into()
            } else {
                "optional — not set".into()
            }
        }),
        remediation: if ok {
            None
        } else {
            remediation.map(str::to_string)
        },
    }
}

fn gateway_health_check(gateway_url: &str) -> DoctorCheck {
    let base = gateway_url.trim_end_matches('/');
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            return DoctorCheck {
                name: "gateway".into(),
                ok: false,
                detail: format!("{gateway_url} — {e}"),
                remediation: Some("Ensure gateway is running (`compose_up`).".into()),
            };
        }
    };

    let api_url = format!("{base}/api/v1/system/health");
    if let Ok(v) = get_json(&client, &api_url) {
        let status = v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("ok");
        return DoctorCheck {
            name: "gateway".into(),
            ok: true,
            detail: format!("{gateway_url} — {status}"),
            remediation: None,
        };
    }

    let root_url = format!("{base}/health");
    match get_text(&client, &root_url) {
        Ok(body) => {
            let snippet: String = body.chars().take(80).collect();
            DoctorCheck {
                name: "gateway".into(),
                ok: true,
                detail: format!("{gateway_url} — {snippet}"),
                remediation: None,
            }
        }
        Err(e) => DoctorCheck {
            name: "gateway".into(),
            ok: false,
            detail: format!("{gateway_url} — {e}"),
            remediation: Some("Start gateway stack (`compose_up`) and re-run doctor.".into()),
        },
    }
}

fn worker_health_check(worker_url: &str) -> DoctorCheck {
    let base = worker_url.trim_end_matches('/');
    let url = format!("{base}/health");
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            return DoctorCheck {
                name: "worker".into(),
                ok: false,
                detail: format!("{worker_url} — {e}"),
                remediation: Some("Ensure worker container is up (`compose_up`).".into()),
            };
        }
    };

    match get_json(&client, &url) {
        Ok(v) => {
            let svc = v
                .get("service")
                .and_then(|s| s.as_str())
                .unwrap_or("worker");
            DoctorCheck {
                name: "worker".into(),
                ok: true,
                detail: format!("{worker_url} — {svc}"),
                remediation: None,
            }
        }
        Err(e) => DoctorCheck {
            name: "worker".into(),
            ok: false,
            detail: format!("{worker_url} — {e}"),
            remediation: Some("Start worker (`compose_up`) and re-run doctor.".into()),
        },
    }
}

fn docker_compose_check() -> DoctorCheck {
    match Command::new("docker")
        .args(["compose", "ps", "--format", "json"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let n = String::from_utf8_lossy(&out.stdout).lines().count();
            DoctorCheck {
                name: "docker compose".into(),
                ok: true,
                detail: format!("{n} service line(s) reported"),
                remediation: None,
            }
        }
        Ok(out) => DoctorCheck {
            name: "docker compose".into(),
            ok: false,
            detail: format!(
                "exit {} — {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            remediation: Some(
                "Run `docker compose up -d` from the ClawZ install directory.".into(),
            ),
        },
        Err(e) => DoctorCheck {
            name: "docker compose".into(),
            ok: true,
            detail: format!("skipped ({e})"),
            remediation: None,
        },
    }
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())
}

fn get_text(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    let res = client
        .get(url)
        .send()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let status = res.status();
    let body = res.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        let snippet: String = body.chars().take(120).collect();
        return Err(format!("HTTP {status}: {snippet}"));
    }
    Ok(body)
}

fn get_json(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Result<serde_json::Value, String> {
    let body = get_text(client, url)?;
    serde_json::from_str(&body).map_err(|e| format!("parse JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_all_ok_when_every_check_passes() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "a".into(),
                    ok: true,
                    detail: "ok".into(),
                    remediation: None,
                },
                DoctorCheck {
                    name: "b".into(),
                    ok: true,
                    detail: "ok".into(),
                    remediation: None,
                },
            ],
        };
        assert!(report.all_ok());
    }

    #[test]
    fn env_check_optional_missing_is_ok() {
        let c = env_check("TEST_VAR", None, false, Some("hint"));
        assert!(c.ok);
        assert!(c.remediation.is_none());
    }

    #[test]
    fn env_check_required_missing_fails_with_remediation() {
        let c = env_check("TEST_VAR", None, true, Some("set it"));
        assert!(!c.ok);
        assert_eq!(c.remediation.as_deref(), Some("set it"));
    }

    #[test]
    fn doctor_fix_tools_maps_failed_checks() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "host spec".into(),
                    ok: false,
                    detail: "warn".into(),
                    remediation: None,
                },
                DoctorCheck {
                    name: "gateway".into(),
                    ok: false,
                    detail: "down".into(),
                    remediation: None,
                },
                DoctorCheck {
                    name: "worker".into(),
                    ok: true,
                    detail: "up".into(),
                    remediation: None,
                },
            ],
        };
        let tools = doctor_fix_tools(&report);
        assert!(tools.contains(&"spec_check"));
        assert!(tools.contains(&"install_deps"));
        assert!(tools.contains(&"compose_up"));
        assert!(tools.contains(&"doctor_run"));
        assert!(!tools.iter().any(|t| *t == "write_env"));
    }

    #[test]
    fn run_doctor_skips_http_when_urls_empty() {
        let report = run_doctor(&DoctorConfig::default());
        let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"host spec"));
        assert!(names.contains(&"CLAWZ_MODE"));
        assert!(!names.iter().any(|n| *n == "gateway"));
        assert!(!names.iter().any(|n| *n == "worker"));
        assert!(!names.iter().any(|n| *n == "docker compose"));
    }

    #[test]
    fn run_doctor_includes_docker_when_enabled() {
        let report = run_doctor(&DoctorConfig {
            check_docker: true,
            ..DoctorConfig::default()
        });
        assert!(
            report
                .checks
                .iter()
                .any(|c| c.name == "docker compose")
        );
    }

    #[test]
    fn gateway_health_unreachable_reports_fail() {
        let check = gateway_health_check("http://127.0.0.1:1");
        assert!(!check.ok);
        assert!(check.detail.contains("127.0.0.1:1"));
    }

    #[test]
    fn host_spec_check_has_nonempty_detail() {
        let check = host_spec_check();
        assert!(!check.detail.is_empty());
        assert_eq!(check.name, "host spec");
    }
}
