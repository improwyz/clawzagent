//! Provider-specific OAuth authorize URLs and token exchanges.

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::Utc;
use reqwest::blocking::Client;
use serde::Deserialize;
use url::Url;

use crate::error::{Result, SetupError};
use crate::oauth::pkce::PkcePair;
use crate::oauth::types::{OAuthTokenBundle, SetupOAuthProvider};

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTH_URL: &str = "https://claude.ai/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const ANTHROPIC_SCOPE: &str = "org:create_api_key user:profile user:inference";

const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_SCOPE: &str = "openid profile email offline_access";

const OPENAI_DEVICE_USER_CODE_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/usercode";
const OPENAI_DEVICE_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/deviceauth/token";
const OPENAI_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";

/// Build the provider authorization URL (PKCE).
pub fn build_authorize_url(
    provider: SetupOAuthProvider,
    redirect_uri: &str,
    state: &str,
    pkce: &PkcePair,
) -> Result<String> {
    match provider {
        SetupOAuthProvider::Anthropic => build_anthropic_authorize(redirect_uri, state, pkce),
        SetupOAuthProvider::Openai | SetupOAuthProvider::Codex => {
            build_openai_authorize(redirect_uri, state, pkce, "clawz-setup")
        }
        SetupOAuthProvider::Cursor | SetupOAuthProvider::Skip => Err(SetupError::Internal(
            "authorize URL not used for cursor/skip".into(),
        )),
    }
}

fn build_anthropic_authorize(redirect_uri: &str, state: &str, pkce: &PkcePair) -> Result<String> {
    let mut url = Url::parse(ANTHROPIC_AUTH_URL)
        .map_err(|e| SetupError::Internal(format!("anthropic auth url: {e}")))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("code", "true");
        qp.append_pair("response_type", "code");
        qp.append_pair("client_id", ANTHROPIC_CLIENT_ID);
        qp.append_pair("redirect_uri", redirect_uri);
        qp.append_pair("scope", ANTHROPIC_SCOPE);
        qp.append_pair("code_challenge", &pkce.challenge);
        qp.append_pair("code_challenge_method", "S256");
        qp.append_pair("state", state);
    }
    Ok(url.to_string())
}

fn build_openai_authorize(
    redirect_uri: &str,
    state: &str,
    pkce: &PkcePair,
    originator: &str,
) -> Result<String> {
    let mut url =
        Url::parse(OPENAI_AUTH_URL).map_err(|e| SetupError::Internal(format!("openai auth: {e}")))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("response_type", "code");
        qp.append_pair("client_id", &openai_client_id());
        qp.append_pair("redirect_uri", redirect_uri);
        qp.append_pair("scope", OPENAI_SCOPE);
        qp.append_pair("code_challenge", &pkce.challenge);
        qp.append_pair("code_challenge_method", "S256");
        qp.append_pair("state", state);
        qp.append_pair("id_token_add_organizations", "true");
        qp.append_pair("codex_cli_simplified_flow", "true");
        qp.append_pair("originator", originator);
    }
    Ok(url.to_string())
}

/// Start OpenAI device-code flow (headless / SSH).
pub fn start_openai_device_code() -> Result<DeviceCodeStart> {
    let client = Client::new();
    let response = client
        .post(OPENAI_DEVICE_USER_CODE_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", "clawz-setup")
        .json(&serde_json::json!({
            "client_id": openai_client_id(),
            "originator": "clawz-setup",
        }))
        .send()
        .map_err(|e| SetupError::Internal(format!("openai device start: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().unwrap_or_default();
        return Err(SetupError::Internal(format!(
            "openai device start failed: {text}"
        )));
    }

    let body: DeviceUserCodeResponse = response
        .json()
        .map_err(|e| SetupError::Internal(format!("openai device parse: {e}")))?;

    let interval_secs = body.interval_secs();
    let user_code = body
        .user_code
        .clone()
        .or(body.usercode.clone())
        .ok_or_else(|| SetupError::Internal("openai device missing user_code".into()))?;
    let device_auth_id = body
        .device_auth_id
        .ok_or_else(|| SetupError::Internal("openai device missing device_auth_id".into()))?;

    Ok(DeviceCodeStart {
        device_auth_id,
        user_code,
        verification_uri: "https://auth.openai.com/codex/device".into(),
        interval_secs,
    })
}

/// Poll device authorization; returns authorization code + verifier when ready.
pub fn poll_openai_device_code(
    device_auth_id: &str,
    user_code: &str,
) -> Result<Option<DeviceCodeGrant>> {
    let client = Client::new();
    let response = client
        .post(OPENAI_DEVICE_TOKEN_URL)
        .header("Content-Type", "application/json")
        .header("User-Agent", "clawz-setup")
        .json(&serde_json::json!({
            "device_auth_id": device_auth_id,
            "user_code": user_code,
        }))
        .send()
        .map_err(|e| SetupError::Internal(format!("openai device poll: {e}")))?;

    if response.status().is_success() {
        let data: DeviceTokenResponse = response
            .json()
            .map_err(|e| SetupError::Internal(format!("openai device token parse: {e}")))?;
        let code = data
            .authorization_code
            .ok_or_else(|| SetupError::Internal("device poll missing authorization_code".into()))?;
        let verifier = data
            .code_verifier
            .ok_or_else(|| SetupError::Internal("device poll missing code_verifier".into()))?;
        return Ok(Some(DeviceCodeGrant { code, verifier }));
    }

    let status = response.status();
    if status == 403 || status == 404 {
        return Ok(None);
    }

    let text = response.text().unwrap_or_default();
    Err(SetupError::Internal(format!(
        "openai device poll error: {status} {text}"
    )))
}

/// Exchange an authorization code for tokens.
pub fn exchange_code(
    provider: SetupOAuthProvider,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenBundle> {
    match provider {
        SetupOAuthProvider::Anthropic => exchange_anthropic(code, code_verifier, redirect_uri),
        SetupOAuthProvider::Openai => exchange_openai(code, code_verifier, redirect_uri),
        SetupOAuthProvider::Codex => exchange_codex(code, code_verifier, redirect_uri),
        SetupOAuthProvider::Cursor | SetupOAuthProvider::Skip => Err(SetupError::Internal(
            "code exchange not supported for cursor/skip".into(),
        )),
    }
}

/// Exchange device-flow grant (OpenAI uses a fixed redirect for device auth).
pub fn exchange_openai_device_grant(grant: &DeviceCodeGrant) -> Result<OAuthTokenBundle> {
    exchange_openai(&grant.code, &grant.verifier, OPENAI_DEVICE_REDIRECT_URI)
}

fn exchange_anthropic(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenBundle> {
    let client = Client::new();
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "code_verifier": code_verifier,
        "client_id": ANTHROPIC_CLIENT_ID,
        "redirect_uri": redirect_uri,
    });
    let response = client
        .post(ANTHROPIC_TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| SetupError::Internal(format!("anthropic token: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().unwrap_or_default();
        return Err(SetupError::Internal(format!(
            "anthropic token exchange failed: {text}"
        )));
    }

    let json: StandardTokenJson = response
        .json()
        .map_err(|e| SetupError::Internal(format!("anthropic token parse: {e}")))?;

    let expires_at = json.expires_at();
    Ok(OAuthTokenBundle {
        provider: "anthropic".into(),
        access_token: json.access_token,
        refresh_token: json.refresh_token,
        id_token: json.id_token,
        account_id: None,
        expires_at,
        source: "oauth".into(),
    })
}

fn exchange_openai(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenBundle> {
    let client = Client::new();
    let client_id = openai_client_id();
    let mut params = HashMap::new();
    params.insert("grant_type", "authorization_code");
    params.insert("client_id", client_id.as_str());
    params.insert("code", code);
    params.insert("code_verifier", code_verifier);
    params.insert("redirect_uri", redirect_uri);

    let response = client
        .post(OPENAI_TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&params)
        .send()
        .map_err(|e| SetupError::Internal(format!("openai token: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().unwrap_or_default();
        return Err(SetupError::Internal(format!(
            "openai token exchange failed: {text}"
        )));
    }

    let json: StandardTokenJson = response
        .json()
        .map_err(|e| SetupError::Internal(format!("openai token parse: {e}")))?;

    let expires_at = json.expires_at();
    let id_token = json.id_token;
    let account_id = extract_openai_account_id(id_token.as_deref());
    Ok(OAuthTokenBundle {
        provider: "openai".into(), // caller may override for codex
        access_token: json.access_token,
        refresh_token: json.refresh_token,
        id_token,
        account_id,
        expires_at,
        source: "oauth".into(),
    })
}

/// Same token endpoint as OpenAI; tags provider as `codex`.
pub fn exchange_codex(
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<OAuthTokenBundle> {
    let mut bundle = exchange_openai(code, code_verifier, redirect_uri)?;
    bundle.provider = "codex".into();
    Ok(bundle)
}

fn openai_client_id() -> String {
    std::env::var("CLAWZ_SETUP_OPENAI_OAUTH_CLIENT_ID")
        .unwrap_or_else(|_| OPENAI_CODEX_CLIENT_ID.to_string())
}

fn extract_openai_account_id(id_token: Option<&str>) -> Option<String> {
    let token = id_token?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let padded = parts[1]
        .replace('-', "+")
        .replace('_', "/");
    let padded = format!(
        "{}{}",
        padded,
        "=".repeat((4 - padded.len() % 4) % 4)
    );
    let decoded = STANDARD.decode(padded).ok()?;
    let payload: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    payload
        .get("chatgpt_account_id")
        .or_else(|| {
            payload
                .get("https://api.openai.com/auth")
                .and_then(|v| v.get("chatgpt_account_id"))
        })
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[derive(Debug, Clone)]
pub struct DeviceCodeStart {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
}

#[derive(Debug, Clone)]
pub struct DeviceCodeGrant {
    pub code: String,
    pub verifier: String,
}

#[derive(Debug, Deserialize)]
struct DeviceUserCodeResponse {
    device_auth_id: Option<String>,
    user_code: Option<String>,
    usercode: Option<String>,
    interval: Option<serde_json::Value>,
}

impl DeviceUserCodeResponse {
    fn interval_secs(&self) -> u64 {
        match &self.interval {
            Some(serde_json::Value::Number(n)) => n.as_u64().unwrap_or(5),
            Some(serde_json::Value::String(s)) => s.parse().unwrap_or(5),
            _ => 5,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    authorization_code: Option<String>,
    code_verifier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StandardTokenJson {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl StandardTokenJson {
    fn expires_at(&self) -> Option<String> {
        self.expires_in.map(|secs| {
            (Utc::now() + chrono::Duration::seconds(secs as i64)).to_rfc3339()
        })
    }
}
