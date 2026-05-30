//! Host capability probe for wizard phase 0 (welcome / spec check).

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::types::DeploymentChoice;

/// Minimum supported Rust toolchain (matches `scripts/install-deps.sh`).
pub const CLAWZ_MIN_RUST: &str = "1.87.0";

/// Minimum RAM (MB) recommended per deployment mode.
const RAM_MB_STANDALONE: u64 = 4 * 1024;
const RAM_MB_MICRO: u64 = 8 * 1024;
const RAM_MB_ELASTIC: u64 = 16 * 1024;

/// Minimum free disk (GB) for Docker build / image pull paths.
const DISK_GB_MIN: u64 = 10;

/// Collected host facts and derived warnings for the setup wizard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSpecReport {
    pub os: String,
    pub arch: String,
    pub ram_mb: Option<u64>,
    pub disk_free_gb: Option<u64>,
    pub cpu_count: Option<u32>,
    pub docker_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docker_detail: Option<String>,
    pub compose_v2_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_version: Option<String>,
    pub rust_meets_minimum: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,
    pub github_token_present: bool,
    pub summary: String,
    pub warnings: Vec<String>,
}

/// Probes the local machine for install prerequisites.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostSpecChecker;

impl HostSpecChecker {
    /// Gather OS, resource, toolchain, and registry-auth signals.
    pub fn collect() -> HostSpecReport {
        let os = os_description();
        let arch = std::env::consts::ARCH.to_string();
        let ram_mb = total_memory_mb();
        let disk_free_gb = disk_free_gb(Path::new("."));
        let cpu_count = cpu_count();
        let (docker_available, docker_detail) = docker_info();
        let (compose_v2_available, compose_detail) = compose_v2_info();
        let (rust_version, rust_meets_minimum) = rust_toolchain();
        let node_version = node_toolchain();
        let github_token_present = github_token_present();

        let mut warnings = Vec::new();
        if !docker_available {
            warnings.push(
                "Docker is not available (install Docker or ensure `docker info` succeeds)."
                    .into(),
            );
        }
        if docker_available && !compose_v2_available {
            warnings.push(
                "Docker Compose v2 is not available (`docker compose version` failed)."
                    .into(),
            );
        }
        if rust_version.is_none() {
            warnings.push(format!(
                "Rust toolchain not found on PATH (need rustc >= {CLAWZ_MIN_RUST} for source builds)."
            ));
        } else if rust_meets_minimum == Some(false) {
            warnings.push(format!(
                "Rust version is below minimum {CLAWZ_MIN_RUST} (upgrade via rustup)."
            ));
        }
        if !github_token_present {
            warnings.push(
                "GITHUB_TOKEN is not set (required for GHCR prebuilt images in private registry)."
                    .into(),
            );
        }

        let summary = build_summary(
            &os,
            &arch,
            ram_mb,
            disk_free_gb,
            cpu_count,
            docker_available,
            compose_v2_available,
            rust_version.as_deref(),
            node_version.as_deref(),
            github_token_present,
        );

        HostSpecReport {
            os,
            arch,
            ram_mb,
            disk_free_gb,
            cpu_count,
            docker_available,
            docker_detail,
            compose_v2_available,
            compose_detail,
            rust_version,
            rust_meets_minimum,
            node_version,
            github_token_present,
            summary,
            warnings,
        }
    }
}

impl HostSpecReport {
    /// Deployment-specific resource threshold warnings (RAM, disk).
    pub fn threshold_warnings(&self, deployment: DeploymentChoice) -> Vec<String> {
        let mut out = Vec::new();
        let min_ram = match deployment {
            DeploymentChoice::Standalone => RAM_MB_STANDALONE,
            DeploymentChoice::Micro => RAM_MB_MICRO,
            DeploymentChoice::Elastic => RAM_MB_ELASTIC,
        };
        let mode = match deployment {
            DeploymentChoice::Standalone => "standalone",
            DeploymentChoice::Micro => "micro",
            DeploymentChoice::Elastic => "elastic",
        };

        if let Some(ram) = self.ram_mb {
            if ram < min_ram {
                out.push(format!(
                    "{mode} mode recommends at least {} GB RAM (detected {} MB).",
                    min_ram / 1024,
                    ram
                ));
            }
        } else {
            out.push(format!(
                "Could not detect RAM; {mode} mode recommends at least {} GB.",
                min_ram / 1024
            ));
        }

        if let Some(disk) = self.disk_free_gb {
            if disk < DISK_GB_MIN {
                out.push(format!(
                    "Free disk space is low ({} GB free; recommend >= {DISK_GB_MIN} GB for images/builds).",
                    disk
                ));
            }
        }

        match deployment {
            DeploymentChoice::Micro | DeploymentChoice::Elastic => {
                if !self.docker_available {
                    out.push(format!("{mode} mode requires Docker."));
                }
                if self.docker_available && !self.compose_v2_available {
                    out.push(format!("{mode} mode requires Docker Compose v2."));
                }
            }
            DeploymentChoice::Standalone => {}
        }

        out
    }
}

fn build_summary(
    os: &str,
    arch: &str,
    ram_mb: Option<u64>,
    disk_gb: Option<u64>,
    cpu_count: Option<u32>,
    docker: bool,
    compose: bool,
    rust: Option<&str>,
    node: Option<&str>,
    gh_token: bool,
) -> String {
    let ram = ram_mb
        .map(|m| format!("{m} MB"))
        .unwrap_or_else(|| "unknown".into());
    let disk = disk_gb
        .map(|g| format!("{g} GB free"))
        .unwrap_or_else(|| "unknown disk".into());
    let cpus = cpu_count
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".into());
    let docker_s = if docker { "ok" } else { "missing" };
    let compose_s = if compose { "ok" } else { "missing" };
    let rust_s = rust.unwrap_or("not installed");
    let node_s = node.unwrap_or("not installed");
    let ghcr_s = if gh_token {
        "GITHUB_TOKEN set"
    } else {
        "no GITHUB_TOKEN"
    };

    format!(
        "{os} ({arch}) · {ram} RAM · {disk} · {cpus} CPUs · Docker {docker_s} · Compose v2 {compose_s} · Rust {rust_s} · Node {node_s} · {ghcr_s}"
    )
}

fn os_description() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
            for line in content.lines() {
                if let Some(name) = line.strip_prefix("PRETTY_NAME=") {
                    return name.trim_matches('"').to_string();
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = Command::new("sw_vers").arg("-productVersion").output() {
            if out.status.success() {
                let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !ver.is_empty() {
                    return format!("macOS {ver}");
                }
            }
        }
    }
    std::env::consts::OS.to_string()
}

fn total_memory_mb() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let content = std::fs::read_to_string("/proc/meminfo").ok()?;
        return parse_meminfo_kb(&content).map(|kb| kb / 1024);
    }
    #[cfg(target_os = "macos")]
    {
        return sysctl_u64("hw.memsize").map(|bytes| bytes / (1024 * 1024));
    }
    #[cfg(target_os = "windows")]
    {
        return windows_memory_mb();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = ();
        None
    }
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    let out = Command::new("sysctl").args(["-n", name]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "windows")]
fn windows_memory_mb() -> Option<u64> {
    // wmic is deprecated but widely available; failure leaves ram_mb unset.
    let out = Command::new("wmic")
        .args(["OS", "get", "TotalVisibleMemorySize", "/Value"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if let Some(kb) = line.strip_prefix("TotalVisibleMemorySize=") {
            let kb: u64 = kb.trim().parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

fn parse_meminfo_kb(content: &str) -> Option<u64> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest
                .split_whitespace()
                .next()?
                .parse()
                .ok()?;
            return Some(kb);
        }
    }
    None
}

fn disk_free_gb(path: &Path) -> Option<u64> {
    let path_str = path.to_str()?;
    let output = Command::new("df").args(["-Pk", path_str]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    let avail_k = line.split_whitespace().nth(3)?;
    let kb: u64 = avail_k.parse().ok()?;
    Some(kb / (1024 * 1024))
}

fn cpu_count() -> Option<u32> {
    std::thread::available_parallelism()
        .ok()
        .map(|n| n.get() as u32)
}

fn run_command(program: &str, args: &[&str]) -> Option<std::process::Output> {
    Command::new(program).args(args).output().ok()
}

fn docker_info() -> (bool, Option<String>) {
    let out = match run_command("docker", &["info"]) {
        Some(o) => o,
        None => return (false, Some("docker executable not found".into())),
    };
    if out.status.success() {
        (true, None)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let detail = if err.trim().is_empty() {
            format!("exit code {}", out.status)
        } else {
            err.trim().to_string()
        };
        (false, Some(detail))
    }
}

fn compose_v2_info() -> (bool, Option<String>) {
    let out = match run_command("docker", &["compose", "version"]) {
        Some(o) => o,
        None => return (false, Some("docker executable not found".into())),
    };
    if out.status.success() {
        let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (true, Some(ver))
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        (false, Some(err.trim().to_string()))
    }
}

fn rust_toolchain() -> (Option<String>, Option<bool>) {
    let Some(out) = run_command("rustc", &["--version"]) else {
        return (None, None);
    };
    if !out.status.success() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(version) = parse_rustc_version(&text) else {
        return (None, None);
    };
    let ok = version_ge(&version, CLAWZ_MIN_RUST);
    (Some(version), Some(ok))
}

fn parse_rustc_version(output: &str) -> Option<String> {
    // `rustc 1.87.0 (d9a3e6f25 2025-04-15)`
    let token = output.split_whitespace().nth(1)?;
    Some(token.to_string())
}

fn node_toolchain() -> Option<String> {
    let out = run_command("node", &["--version"])?;
    if !out.status.success() {
        return None;
    }
    let ver = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if ver.is_empty() {
        None
    } else {
        Some(ver)
    }
}

fn github_token_present() -> bool {
    std::env::var("GITHUB_TOKEN")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

/// Compare dotted numeric semver prefixes (`1.87.0` >= `1.87`).
pub fn version_ge(actual: &str, minimum: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split(|c| c == '.' || c == '-')
            .filter_map(|p| {
                let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
                if digits.is_empty() {
                    None
                } else {
                    digits.parse().ok()
                }
            })
            .collect()
    };
    let a = parse(actual);
    let m = parse(minimum);
    let len = a.len().max(m.len());
    for i in 0..len {
        let av = a.get(i).copied().unwrap_or(0);
        let mv = m.get(i).copied().unwrap_or(0);
        if av > mv {
            return true;
        }
        if av < mv {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DeploymentChoice;

    #[test]
    fn version_ge_matches_install_script_expectations() {
        assert!(version_ge("1.87.0", CLAWZ_MIN_RUST));
        assert!(version_ge("1.88.0", CLAWZ_MIN_RUST));
        assert!(!version_ge("1.86.9", CLAWZ_MIN_RUST));
        assert!(version_ge("1.87", "1.87.0"));
    }

    #[test]
    fn parse_meminfo_kb_reads_total() {
        let sample = "MemTotal:       16384000 kB\nMemFree:        8000000 kB\n";
        assert_eq!(parse_meminfo_kb(sample), Some(16_384_000));
    }

    #[test]
    fn parse_rustc_version_extracts_release() {
        assert_eq!(
            parse_rustc_version("rustc 1.87.0 (abc 2025-01-01)"),
            Some("1.87.0".into())
        );
    }

    #[test]
    fn threshold_warnings_micro_requires_ram_and_docker() {
        let report = HostSpecReport {
            os: "Linux".into(),
            arch: "x86_64".into(),
            ram_mb: Some(2048),
            disk_free_gb: Some(5),
            cpu_count: Some(2),
            docker_available: false,
            docker_detail: None,
            compose_v2_available: false,
            compose_detail: None,
            rust_version: None,
            rust_meets_minimum: None,
            node_version: None,
            github_token_present: false,
            summary: String::new(),
            warnings: vec![],
        };
        let w = report.threshold_warnings(DeploymentChoice::Micro);
        assert!(w.iter().any(|s| s.contains("8 GB RAM")));
        assert!(w.iter().any(|s| s.contains("disk")));
        assert!(w.iter().any(|s| s.contains("Docker")));
    }

    #[test]
    fn threshold_warnings_standalone_allows_no_docker() {
        let report = HostSpecReport {
            os: "Linux".into(),
            arch: "x86_64".into(),
            ram_mb: Some(8 * 1024),
            disk_free_gb: Some(50),
            cpu_count: Some(4),
            docker_available: false,
            docker_detail: None,
            compose_v2_available: false,
            compose_detail: None,
            rust_version: Some("1.87.0".into()),
            rust_meets_minimum: Some(true),
            node_version: None,
            github_token_present: true,
            summary: String::new(),
            warnings: vec![],
        };
        let w = report.threshold_warnings(DeploymentChoice::Standalone);
        assert!(w.is_empty());
    }

    #[test]
    fn build_summary_includes_key_fields() {
        let s = build_summary(
            "Ubuntu 24.04",
            "x86_64",
            Some(8192),
            Some(100),
            Some(4),
            true,
            true,
            Some("1.87.0"),
            Some("v22.0.0"),
            true,
        );
        assert!(s.contains("Ubuntu"));
        assert!(s.contains("8192 MB"));
        assert!(s.contains("GITHUB_TOKEN"));
    }
}
