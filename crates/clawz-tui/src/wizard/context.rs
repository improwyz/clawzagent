//! Collected wizard answers (env exports, identity fields).

/// User input gathered across wizard phases.
#[derive(Debug, Clone, Default)]
pub struct WizardAnswers {
    pub port: String,
    pub jwt_secret: String,
    pub api_keys: String,
    pub worker_token: String,
    pub anthropic_key: String,
    pub openai_key: String,
    pub log_level: String,
    pub agent_name: String,
    pub agent_who: String,
    pub agent_role: String,
}

impl WizardAnswers {
    pub fn print_env_exports(&self) {
        let mode = std::env::var("CLAWZ_MODE").unwrap_or_else(|_| "micro".into());
        println!("\n┌──────────────────────────────────────────────────┐");
        println!("│           ClawZ configuration summary             │");
        println!("├──────────────────────────────────────────────────┤");
        println!("│  Port         : {:<33} │", self.port);
        println!("│  Log level    : {:<33} │", self.log_level);
        println!(
            "│  API keys     : {:<33} │",
            crate::mask_key(&self.api_keys)
        );
        println!(
            "│  Anthropic    : {:<33} │",
            crate::mask_key(&self.anthropic_key)
        );
        println!(
            "│  OpenAI       : {:<33} │",
            crate::mask_key(&self.openai_key)
        );
        println!("│  Agent        : {:<33} │", self.agent_name);
        println!("└──────────────────────────────────────────────────┘");

        println!("\nAdd to `.env` or your shell:\n");
        println!("  export CLAWZ__SERVER__PORT={}", self.port);
        println!("  export CLAWZ_JWT_SECRET={}", self.jwt_secret);
        println!("  export VALID_API_KEYS=\"{}\"", self.api_keys);
        println!("  export CLAWZ_WORKER_TOKEN={}", self.worker_token);
        println!("  export WORKER_URL=http://127.0.0.1:50051");
        println!("  export CLAWZ_MODE={mode}");
        println!("  export RUST_LOG={}", self.log_level);
        if !self.anthropic_key.is_empty() {
            println!("  export ANTHROPIC_API_KEY={}", self.anthropic_key);
        }
        if !self.openai_key.is_empty() {
            println!("  export OPENAI_API_KEY={}", self.openai_key);
        }
        if !self.agent_name.is_empty() {
            println!(
                "  # Agent identity: {} — {} ({})",
                self.agent_name, self.agent_who, self.agent_role
            );
        }
        println!("\nDocker Compose: run `./scripts/install.sh` from the ClawZ repo.");
        println!("Source:         `cargo run -p clawz-gateway` (gateway) + worker on :50051");
    }
}
