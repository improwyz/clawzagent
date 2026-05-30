//! Plain stdin/stdout prompts for headless CLI onboarding.

use std::io::{self, BufRead, Write};

use serde::Serialize;

use crate::spec::HostSpecReport;
use crate::types::{DeploymentChoice, InstallStrategy};

/// Values suggested for shell / `.env` export (matches legacy `clawz-tui` lines).
#[derive(Debug, Clone, Serialize)]
pub struct EnvRecommendations {
    pub port: String,
    pub jwt_secret: String,
    pub api_keys: String,
    pub worker_token: String,
    pub worker_url: String,
    pub mode: String,
    pub log_level: String,
    pub anthropic_key: String,
    pub openai_key: String,
}

impl EnvRecommendations {
    pub fn from_session(
        deployment: DeploymentChoice,
        port: Option<&str>,
    ) -> Self {
        let mode = match deployment {
            DeploymentChoice::Standalone => "standalone",
            DeploymentChoice::Micro => "micro",
            DeploymentChoice::Elastic => "elastic",
        };
        Self {
            port: port.unwrap_or("3000").to_string(),
            jwt_secret: generate_hex_secret(32),
            api_keys: "dev-key".to_string(),
            worker_token: generate_hex_secret(24),
            worker_url: "http://127.0.0.1:50051".to_string(),
            mode: mode.to_string(),
            log_level: "info".to_string(),
            anthropic_key: String::new(),
            openai_key: String::new(),
        }
    }
}

/// Print backward-compatible `export` lines for shell or `.env`.
pub fn print_env_recommendations(rec: &EnvRecommendations) {
    println!("\nAdd to `.env` or your shell:\n");
    println!("  export CLAWZ__SERVER__PORT={}", rec.port);
    println!("  export CLAWZ_JWT_SECRET={}", rec.jwt_secret);
    println!("  export VALID_API_KEYS=\"{}\"", rec.api_keys);
    println!("  export CLAWZ_WORKER_TOKEN={}", rec.worker_token);
    println!("  export WORKER_URL={}", rec.worker_url);
    println!("  export CLAWZ_MODE={}", rec.mode);
    println!("  export RUST_LOG={}", rec.log_level);
    if !rec.anthropic_key.is_empty() {
        println!("  export ANTHROPIC_API_KEY={}", rec.anthropic_key);
    }
    if !rec.openai_key.is_empty() {
        println!("  export OPENAI_API_KEY={}", rec.openai_key);
    }
}

/// Interactive env block (optional secrets / keys).
pub fn prompt_env_recommendations(
    reader: &mut impl BufRead,
    deployment: DeploymentChoice,
) -> EnvRecommendations {
    let default_mode = match deployment {
        DeploymentChoice::Standalone => "standalone",
        DeploymentChoice::Micro => "micro",
        DeploymentChoice::Elastic => "elastic",
    };
    let port = prompt(reader, "Gateway port", "3000");
    let jwt_secret = prompt_secret(reader, "JWT secret (blank = auto-generate)");
    let jwt_secret = if jwt_secret.is_empty() {
        generate_hex_secret(32)
    } else {
        jwt_secret
    };
    let api_keys = prompt(reader, "API keys (comma-separated)", "dev-key");
    let worker_token = prompt_secret(reader, "Worker internal token (blank = auto)");
    let worker_token = if worker_token.is_empty() {
        generate_hex_secret(24)
    } else {
        worker_token
    };
    let anthropic_key = prompt(reader, "Anthropic API key (optional)", "");
    let openai_key = prompt(reader, "OpenAI API key (optional)", "");
    let log_level = prompt(
        reader,
        "Log level (trace/debug/info/warn/error)",
        "info",
    );
    let mode = prompt(
        reader,
        "Deployment mode (standalone/micro/elastic)",
        default_mode,
    );

    EnvRecommendations {
        port,
        jwt_secret,
        api_keys,
        worker_token,
        worker_url: "http://127.0.0.1:50051".to_string(),
        mode,
        log_level,
        anthropic_key,
        openai_key,
    }
}

pub fn prompt_deployment(reader: &mut impl BufRead) -> DeploymentChoice {
    loop {
        let raw = prompt(
            reader,
            "Deployment mode (standalone/micro/elastic)",
            "micro",
        );
        if let Some(choice) = parse_deployment(&raw) {
            return choice;
        }
        println!("  Invalid choice — use standalone, micro, or elastic.");
    }
}

pub fn prompt_install_strategy(
    reader: &mut impl BufRead,
    deployment: DeploymentChoice,
    report: &HostSpecReport,
) -> InstallStrategy {
    let default = if report.docker_available {
        "prebuilt"
    } else {
        "source"
    };
    let hint = match deployment {
        DeploymentChoice::Standalone => "prebuilt/build/source",
        DeploymentChoice::Micro | DeploymentChoice::Elastic => {
            "prebuilt (GHCR) / build (local Docker) / source (cargo)"
        }
    };
    loop {
        let raw = prompt(reader, &format!("Install strategy ({hint})"), default);
        if let Some(s) = parse_install_strategy(&raw) {
            return s;
        }
        println!("  Invalid choice — use prebuilt, build, or source.");
    }
}

pub fn parse_deployment(raw: &str) -> Option<DeploymentChoice> {
    match raw.trim().to_lowercase().as_str() {
        "standalone" | "stand-alone" | "local" => Some(DeploymentChoice::Standalone),
        "micro" | "docker" | "compose" => Some(DeploymentChoice::Micro),
        "elastic" | "mesh" | "fleet" => Some(DeploymentChoice::Elastic),
        _ => None,
    }
}

pub fn parse_install_strategy(raw: &str) -> Option<InstallStrategy> {
    match raw.trim().to_lowercase().as_str() {
        "prebuilt" | "pull" | "ghcr" => Some(InstallStrategy::Prebuilt),
        "build" | "local" => Some(InstallStrategy::Build),
        "source" | "cargo" => Some(InstallStrategy::Source),
        _ => None,
    }
}

pub fn prompt(reader: &mut impl BufRead, label: &str, default: &str) -> String {
    if default.is_empty() {
        print!("  {label} : ");
    } else {
        print!("  {label} [{default}]: ");
    }
    let _ = io::stdout().flush();
    let mut input = String::new();
    let _ = reader.read_line(&mut input);
    let trimmed = input.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn prompt_secret(reader: &mut impl BufRead, label: &str) -> String {
    print!("  {label} (hidden): ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    let _ = reader.read_line(&mut input);
    input.trim().to_string()
}

fn generate_hex_secret(bytes: usize) -> String {
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
    use std::io::Cursor;

    #[test]
    fn parse_deployment_values() {
        assert_eq!(
            parse_deployment("elastic"),
            Some(DeploymentChoice::Elastic)
        );
        assert_eq!(parse_deployment("nope"), None);
    }

    #[test]
    fn prompt_uses_default() {
        let mut reader = Cursor::new(b"\n".as_ref());
        assert_eq!(prompt(&mut reader, "Port", "3000"), "3000");
    }
}
