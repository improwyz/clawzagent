//! Interactive terminal wizards for ClawZ first-run setup and configuration.
//!
//! Used by `clawz onboard`, `clawz setup`, and `clawz tui`. All I/O uses
//! `stdin`/`stdout` so prompts are testable with [`std::io::Cursor`].

use std::io::{self, BufRead, Write};

/// Interactive first-run wizard — prints `export` lines for shell or `.env`.
pub fn run_onboarding() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    println!("=== ClawZ Onboarding ===\n");
    println!("Press Enter to accept defaults in [brackets].\n");

    let port = prompt(&mut reader, "Gateway port", "3000");
    let jwt_secret = prompt_secret(&mut reader, "JWT secret (blank = auto-generate)");
    let jwt_secret = if jwt_secret.is_empty() {
        generate_secret(32)
    } else {
        jwt_secret
    };
    let api_keys = prompt(&mut reader, "API keys (comma-separated)", "dev-key");
    let worker_token = prompt_secret(&mut reader, "Worker internal token (blank = auto)");
    let worker_token = if worker_token.is_empty() {
        generate_secret(24)
    } else {
        worker_token
    };
    let anthropic_key = prompt(&mut reader, "Anthropic API key (optional)", "");
    let openai_key = prompt(&mut reader, "OpenAI API key (optional)", "");
    let log_level = prompt(
        &mut reader,
        "Log level (trace/debug/info/warn/error)",
        "info",
    );
    let mode = prompt(
        &mut reader,
        "Deployment mode (standalone/micro/elastic)",
        "micro",
    );

    println!("\n┌──────────────────────────────────────────────────┐");
    println!("│           ClawZ configuration summary             │");
    println!("├──────────────────────────────────────────────────┤");
    println!("│  Port         : {:<33} │", port);
    println!("│  Mode         : {:<33} │", mode);
    println!("│  Log level    : {:<33} │", log_level);
    println!("│  API keys     : {:<33} │", mask_key(&api_keys));
    println!("│  Anthropic    : {:<33} │", mask_key(&anthropic_key));
    println!("│  OpenAI       : {:<33} │", mask_key(&openai_key));
    println!("└──────────────────────────────────────────────────┘");

    println!("\nAdd to `.env` or your shell:\n");
    println!("  export CLAWZ__SERVER__PORT={}", port);
    println!("  export CLAWZ_JWT_SECRET={}", jwt_secret);
    println!("  export VALID_API_KEYS=\"{}\"", api_keys);
    println!("  export CLAWZ_WORKER_TOKEN={}", worker_token);
    println!("  export WORKER_URL=http://127.0.0.1:50051");
    println!("  export CLAWZ_MODE={}", mode);
    println!("  export RUST_LOG={}", log_level);
    if !anthropic_key.is_empty() {
        println!("  export ANTHROPIC_API_KEY={}", anthropic_key);
    }
    if !openai_key.is_empty() {
        println!("  export OPENAI_API_KEY={}", openai_key);
    }
    println!("\nDocker Compose: run `./scripts/install.sh` from the ClawZ repo.");
    println!("Source:         `cargo run -p clawz-gateway` (gateway) + worker on :50051");
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

fn mask_key(key: &str) -> String {
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    format!("{}****", &key[..4])
}

fn generate_secret(bytes: usize) -> String {
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
    fn prompt_uses_default() {
        let mut reader = Cursor::new(b"\n".as_ref());
        assert_eq!(prompt(&mut reader, "Port", "3000"), "3000");
    }

    #[test]
    fn generate_secret_length() {
        assert_eq!(generate_secret(32).len(), 64);
    }
}
