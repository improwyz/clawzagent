//! OAuth types for the setup wizard.

use serde::{Deserialize, Serialize};

/// Setup wizard LLM OAuth providers (v1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupOAuthProvider {
    Cursor,
    Codex,
    Anthropic,
    Openai,
    Skip,
}

impl SetupOAuthProvider {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cursor" => Some(Self::Cursor),
            "codex" => Some(Self::Codex),
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::Openai),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cursor => "cursor",
            Self::Codex => "codex",
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Skip => "skip",
        }
    }
}

/// Result of starting an OAuth flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthStartResult {
    pub provider: String,
    pub state: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    /// `authorization_code` | `device_code` | `api_key` | `imported`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
}

/// Tokens stored after a successful OAuth exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenBundle {
    pub provider: String,
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// How the token was obtained (`oauth`, `import`, `api_key`).
    pub source: String,
}

/// Pending OAuth state persisted until callback completes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingOAuth {
    pub provider: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    pub created_at: String,
}
