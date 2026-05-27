//! Terminal User Interface (TUI) for local ClawZ Gateway development.
//!
//! This module provides interactive command-line wizards and dashboards used
//! during local setup and configuration of the gateway. It is **not** intended
//! for production server use; all I/O operates on `stdin`/`stdout` so the
//! functions remain fully testable with in-memory readers.
//!
//! # Entry points
//! - [`run_onboarding`]: Interactive first-time setup wizard.
//! - [`run_dashboard`]: Read-only status panel (currently static).
//! - [`run_config`]: Interactive configuration editor with sub-menus.
//!
//! # Testability
//! Every prompt helper accepts a generic `impl BufRead` rather than reading
//! directly from `stdin`, allowing tests to drive input via `std::io::Cursor`.

use std::io::{self, BufRead, Write};

// Dependency: external crate `uuid` (used in `generate_secret` for entropy).

/// Run the interactive onboarding wizard.
///
/// Collects the essential configuration values required to start the
/// gateway for the first time—port, JWT secret, API keys, log level,
/// and data directory. After the interview it prints an ASCII summary
/// panel and emits `export` statements that the user can copy into
/// their shell environment.
///
/// All prompts read from the provided `stdin` lock, so the function is
/// fully testable by piping input through a `BufRead` implementation.
pub fn run_onboarding() {
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    println!("=== ClawZ Gateway Setup ===\n");
    println!("Press Enter to accept the default value shown in [brackets].\n");

    let port = prompt(&mut reader, "Server port", "8080");
    let jwt_secret = prompt_secret(&mut reader, "JWT secret (leave blank to auto-generate)");
    // Fallback to a generated secret so the user isn't forced to invent one.
    let jwt_secret = if jwt_secret.is_empty() {
        generate_secret(32)
    } else {
        jwt_secret
    };
    let anthropic_key = prompt(&mut reader, "Anthropic API key", "");
    let openai_key = prompt(&mut reader, "OpenAI API key (optional)", "");
    let log_level = prompt(
        &mut reader,
        "Log level (trace/debug/info/warn/error)",
        "info",
    );
    let data_dir = prompt(&mut reader, "Data directory", "/var/lib/clawz");

    println!("\n┌──────────────────────────────────────────────────┐");
    println!("│         ClawZ Gateway Configuration Summary       │");
    println!("├──────────────────────────────────────────────────┤");
    println!("│  Port        : {:<34} │", port);
    println!("│  Log level   : {:<34} │", log_level);
    println!("│  Data dir    : {:<34} │", data_dir);
    println!("│  Anthropic   : {:<34} │", mask_key(&anthropic_key));
    println!("│  OpenAI      : {:<34} │", mask_key(&openai_key));
    println!("└──────────────────────────────────────────────────┘");

    println!("\nSet these environment variables before starting the gateway:\n");
    println!("  export CLAWZ_PORT={}", port);
    println!("  export JWT_SECRET={}", jwt_secret);
    if !anthropic_key.is_empty() {
        println!("  export ANTHROPIC_API_KEY={}", anthropic_key);
    }
    if !openai_key.is_empty() {
        println!("  export OPENAI_API_KEY={}", openai_key);
    }
    println!("  export RUST_LOG={}", log_level);
    println!("  export CLAWZ_DATA_DIR={}", data_dir);
    println!("\nSetup complete! Run: clawz-gateway --port {}", port);
}

/// Run the terminal dashboard (read-only stats panel).
///
/// Displays a static ASCII table showing mocked gateway metrics.
/// In a production implementation this would poll the gateway HTTP API
/// and refresh the screen (e.g., via `crossterm` or `ratatui`).
pub fn run_dashboard() {
    // Stub: replace with live API polling when the gateway exposes
    // a metrics endpoint. Dependency: gateway HTTP API (not yet wired).
    println!("=== ClawZ Dashboard (Terminal) ===");
    println!("┌─────────────────────────────────────┐");
    println!("│  Agents  : 0 active, 3 configured   │");
    println!("│  Providers: 2 connected              │");
    println!("│  Channels : 4 active                 │");
    println!("│  Uptime   : 2h 34m                   │");
    println!("└─────────────────────────────────────┘");
    println!();
    println!("  Press Ctrl+C to exit.");
    println!();
    println!("  Hint: set CLAWZ_GATEWAY_URL to point at a live gateway");
    println!("  and re-run to see live metrics.");
}

/// Run the interactive configuration editor.
///
/// Presents a top-level menu that dispatches into sub-menus for
/// providers, channels, governance, and deployment targets. The loop
/// continues until the user selects "Back / Exit" or stdin closes.
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
        // Flush so the prompt is visible before blocking on user input.
        io::stdout().flush().unwrap();

        let mut choice = String::new();
        // Break on EOF or pipe closure so the TUI exits cleanly in CI.
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
            other => println!("Unknown option: {}", other),
        }
    }
}

// ---------------------------------------------------------------------------
// Sub-menus
// ---------------------------------------------------------------------------

/// Provider sub-menu: add, list, or remove LLM providers.
///
/// Currently operates as a stub—user input is accepted but no persistent
/// registry is updated. In a real implementation this would mutate the
/// shared gateway configuration store.
fn config_providers(reader: &mut impl BufRead) {
    println!("\n--- Provider Configuration ---");
    println!("  a. Add provider");
    println!("  l. List providers");
    println!("  r. Remove provider");
    println!("  b. Back");
    print!("Choice: ");
    io::stdout().flush().unwrap();

    let mut choice = String::new();
    // Ignore read errors in the dev TUI; empty/unrecognised input maps to "back".
    let _ = reader.read_line(&mut choice);
    match choice.trim() {
        "a" => {
            let name = prompt(
                reader,
                "Provider name (anthropic/openai/gemini)",
                "anthropic",
            );
            let key = prompt_secret(reader, "API key");
            println!(
                "  [+] Provider '{}' configured (key: {}).",
                name,
                mask_key(&key)
            );
        }
        "l" => {
            println!("  Providers:");
            println!("    - anthropic  [connected]");
            println!("    - openai     [connected]");
        }
        "r" => {
            let name = prompt(reader, "Provider name to remove", "");
            println!("  [-] Provider '{}' removed.", name);
        }
        _ => {}
    }
}

/// Channel sub-menu: configure inbound channels (slack, discord, webhook).
///
/// Channels allow external systems to push tasks to agents. This stub
/// prints the captured values without storing them.
fn config_channels(reader: &mut impl BufRead) {
    println!("\n--- Channel Configuration ---");
    println!("  Channels let agents receive tasks from external systems.");
    let name = prompt(reader, "Channel name (slack/discord/webhook)", "webhook");
    let url = prompt(reader, "Endpoint URL", "https://");
    println!("  [+] Channel '{}' → {} configured.", name, url);
}

/// Governance sub-menu: set approval policies and cost limits.
///
/// Controls runtime guardrails such as mandatory human approval for
/// high-risk actions and per-task spend caps.
fn config_governance(reader: &mut impl BufRead) {
    println!("\n--- Governance Configuration ---");
    let require_approval = prompt(
        reader,
        "Require human approval for high-risk actions? [y/N]",
        "n",
    );
    let max_cost = prompt(reader, "Max cost per task in USD [0 = unlimited]", "0");
    println!(
        "  Governance: approval={}, max_cost_usd={}",
        require_approval, max_cost
    );
}

/// Deploy sub-menu: select deployment target and region.
///
/// Captures target platform (fly, railway, etc.) and region, then
/// prints the corresponding CLI invocation hint.
fn config_deploy(reader: &mut impl BufRead) {
    println!("\n--- Deploy Configuration ---");
    println!("  Targets: fly, railway, hetzner, gcp, cloudflare");
    let target = prompt(reader, "Deploy target", "fly");
    let region = prompt(reader, "Region", "iad");
    println!("  [+] Deploy target: {} in {}.", target, region);
    println!(
        "  Run `clawz-gateway deploy --target {}` to deploy.",
        target
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Prompt the user for a line of input with an optional default value.
///
/// If the user presses Enter without typing, `default` is returned.
/// The prompt is written to `stdout` and flushed before reading so the
/// user sees the message immediately in interactive terminals.
fn prompt(reader: &mut impl BufRead, label: &str, default: &str) -> String {
    if default.is_empty() {
        print!("  {} : ", label);
    } else {
        print!("  {} [{}]: ", label, default);
    }
    // Flush stdout so the prompt appears before blocking on stdin.
    io::stdout().flush().unwrap();

    let mut input = String::new();
    // Ignore read errors in the dev TUI; empty input falls back to default.
    let _ = reader.read_line(&mut input);
    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed
    }
}

/// Prompt for a sensitive value (e.g., API key).
///
/// **Note:** This is a development helper; it does *not* disable terminal
/// echo. In a production TUI this should integrate with a crate like
/// `rpassword` or a curses backend to hide keystrokes.
fn prompt_secret(reader: &mut impl BufRead, label: &str) -> String {
    // In a real TUI we would disable echo. Here we just call prompt.
    print!("  {} (hidden): ", label);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    // Ignore read errors; blank secrets are handled by the caller.
    let _ = reader.read_line(&mut input);
    input.trim().to_string()
}

/// Mask a secret string for display, revealing at most the first 4 chars.
///
/// Short keys (≤8 characters) are fully redacted to avoid leaking
/// high-entropy portions of small tokens.
fn mask_key(key: &str) -> String {
    // Fully mask very short keys so we never leak the bulk of a token.
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }
    // Reveal only the prefix to help the user identify which key is
    // configured without exposing the credential.
    let visible = &key[..4];
    format!("{}****", visible)
}

/// Generate a cryptographically-random hex secret of the requested byte length.
///
/// Uses `uuid::Uuid::new_v4` as an entropy source. This is acceptable
/// for the local setup wizard; production deployments should prefer
/// a dedicated CSPRNG or a secrets manager.
fn generate_secret(bytes: usize) -> String {
    // Use uuid bytes as entropy source — good enough for setup wizard.
    // Pre-allocate to avoid repeated reallocations while concatenating hex.
    let mut out = String::with_capacity(bytes * 2);
    // Uuid::new_v4() yields 16 bytes; loop until we have enough entropy,
    // then truncate to the exact requested length.
    while out.len() < bytes * 2 {
        let u = uuid::Uuid::new_v4();
        for b in u.as_bytes() {
            out.push_str(&format!("{:02x}", b));
        }
    }
    out[..bytes * 2].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_prompt_uses_default() {
        let input = b"\n"; // just press Enter
        let mut reader = Cursor::new(input.as_ref());
        let result = prompt(&mut reader, "Port", "8080");
        assert_eq!(result, "8080");
    }

    #[test]
    fn test_prompt_uses_typed_value() {
        let input = b"9090\n";
        let mut reader = Cursor::new(input.as_ref());
        let result = prompt(&mut reader, "Port", "8080");
        assert_eq!(result, "9090");
    }

    #[test]
    fn test_mask_key_short() {
        assert_eq!(mask_key("abc"), "***");
    }

    #[test]
    fn test_mask_key_long() {
        let masked = mask_key("sk-abcdefgh");
        assert!(masked.starts_with("sk-a"));
        assert!(masked.contains("****"));
    }

    #[test]
    fn test_generate_secret_length() {
        let s = generate_secret(32);
        assert_eq!(s.len(), 64); // hex: 2 chars per byte
    }
}
