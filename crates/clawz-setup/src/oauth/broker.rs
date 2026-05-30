//! OAuth broker for the install wizard — start flows, persist pending state, exchange codes.

use std::fs;
use std::path::PathBuf;

use chrono::Utc;
use uuid::Uuid;

use crate::error::{Result, SetupError};
use crate::oauth::cursor::try_import_local_cursor;
use crate::oauth::pkce::generate_pkce;
use crate::oauth::providers::{
    build_authorize_url, exchange_code, exchange_openai_device_grant, poll_openai_device_code,
    start_openai_device_code,
};
use crate::oauth::types::{
    OAuthStartResult, OAuthTokenBundle, PendingOAuth, SetupOAuthProvider,
};
use crate::paths::setup_dir;

fn pending_oauth_path() -> PathBuf {
    setup_dir().join("oauth-pending.json")
}

fn oauth_vault_path() -> PathBuf {
    setup_dir().join("oauth.json")
}

/// Resolve redirect URI for browser OAuth callbacks.
pub fn oauth_redirect_uri() -> String {
    if let Ok(uri) = std::env::var("CLAWZ_SETUP_OAUTH_REDIRECT_URI") {
        if !uri.trim().is_empty() {
            return uri;
        }
    }
    if let Ok(base) = std::env::var("CLAWZ_PUBLIC_URL") {
        let base = base.trim_end_matches('/');
        if !base.is_empty() {
            return format!("{base}/api/v1/setup/oauth/callback");
        }
    }
    let port = std::env::var("CLAWZ__SERVER__PORT")
        .or_else(|_| std::env::var("PORT"))
        .unwrap_or_else(|_| "3000".into());
    format!("http://127.0.0.1:{port}/api/v1/setup/oauth/callback")
}

/// Start an OAuth flow for the given provider.
pub fn oauth_start(provider: SetupOAuthProvider) -> Result<OAuthStartResult> {
    if provider == SetupOAuthProvider::Skip {
        return Ok(OAuthStartResult {
            provider: provider.as_str().into(),
            state: Uuid::new_v4().to_string(),
            message: "OAuth skipped — continue with manual API keys.".into(),
            auth_url: None,
            flow: Some("skip".into()),
            device_code: None,
            verification_uri: None,
            expires_in: None,
        });
    }

    if provider == SetupOAuthProvider::Cursor {
        if let Some(bundle) = try_import_local_cursor() {
            save_oauth_tokens(&bundle)?;
            return Ok(OAuthStartResult {
                provider: "cursor".into(),
                state: Uuid::new_v4().to_string(),
                message: "Imported Cursor credentials from your local Cursor install.".into(),
                auth_url: None,
                flow: Some("imported".into()),
                device_code: None,
                verification_uri: None,
                expires_in: None,
            });
        }
        return Ok(OAuthStartResult {
            provider: "cursor".into(),
            state: Uuid::new_v4().to_string(),
            message: "Generate a Cursor API key at Dashboard → Integrations, then paste it in the LLM step or POST /setup/oauth/callback with token.".into(),
            auth_url: Some("https://cursor.com/dashboard/integrations".into()),
            flow: Some("api_key".into()),
            device_code: None,
            verification_uri: None,
            expires_in: None,
        });
    }

    let state = Uuid::new_v4().to_string();
    let redirect_uri = oauth_redirect_uri();
    let pkce = generate_pkce();

    if provider == SetupOAuthProvider::Codex
        && std::env::var("CLAWZ_SETUP_OAUTH_DEVICE")
            .ok()
            .as_deref()
            == Some("1")
    {
        let device = start_openai_device_code()?;
        save_pending(
            &state,
            &PendingOAuth {
                provider: provider.as_str().into(),
                code_verifier: String::new(),
                redirect_uri: redirect_uri.clone(),
                created_at: Utc::now().to_rfc3339(),
            },
        )?;
        save_device_pending(&state, &device)?;
        return Ok(OAuthStartResult {
            provider: provider.as_str().into(),
            state,
            message: format!(
                "Enter code {} at {}",
                device.user_code, device.verification_uri
            ),
            auth_url: Some(device.verification_uri.clone()),
            flow: Some("device_code".into()),
            device_code: Some(device.user_code),
            verification_uri: Some(device.verification_uri),
            expires_in: Some(900),
        });
    }

    let auth_url = build_authorize_url(provider, &redirect_uri, &state, &pkce)?;
    save_pending(
        &state,
        &PendingOAuth {
            provider: provider.as_str().into(),
            code_verifier: pkce.verifier,
            redirect_uri,
            created_at: Utc::now().to_rfc3339(),
        },
    )?;

    let label = match provider {
        SetupOAuthProvider::Anthropic => "Anthropic",
        SetupOAuthProvider::Openai => "OpenAI",
        SetupOAuthProvider::Codex => "Codex (ChatGPT)",
        _ => "provider",
    };

    Ok(OAuthStartResult {
        provider: provider.as_str().into(),
        state,
        message: format!("Complete sign-in with {label} in your browser."),
        auth_url: Some(auth_url),
        flow: Some("authorization_code".into()),
        device_code: None,
        verification_uri: None,
        expires_in: None,
    })
}

/// Complete OAuth after browser redirect or manual code paste.
pub fn oauth_complete(
    state: &str,
    code: Option<&str>,
    token: Option<&str>,
) -> Result<OAuthTokenBundle> {
    if let Some(raw_token) = token {
        let provider = load_pending_provider(state).unwrap_or_else(|| "unknown".into());
        let bundle = OAuthTokenBundle {
            provider: provider.clone(),
            access_token: raw_token.to_string(),
            refresh_token: None,
            id_token: None,
            account_id: None,
            expires_at: None,
            source: if provider == "cursor" {
                "api_key".into()
            } else {
                "manual".into()
            },
        };
        save_oauth_tokens(&bundle)?;
        clear_pending(state)?;
        return Ok(bundle);
    }

    let code = code.ok_or_else(|| SetupError::Internal("authorization code required".into()))?;

    if let Some(device) = take_device_pending(state)? {
        if let Some(grant) = poll_openai_device_code(&device.device_auth_id, &device.user_code)? {
            let provider = SetupOAuthProvider::parse(&device.provider)
                .unwrap_or(SetupOAuthProvider::Codex);
            let bundle = exchange_openai_device_grant(&grant)?;
            let mut bundle = bundle;
            bundle.provider = provider.as_str().into();
            save_oauth_tokens(&bundle)?;
            clear_pending(state)?;
            clear_device_pending(state)?;
            return Ok(bundle);
        }
        return Err(SetupError::Internal(
            "device authorization not complete yet — enter the code at auth.openai.com/codex/device and retry".into(),
        ));
    }

    let pending = load_pending(state)?
        .ok_or_else(|| SetupError::Internal("unknown or expired OAuth state".into()))?;

    let provider = SetupOAuthProvider::parse(&pending.provider).ok_or_else(|| {
        SetupError::Internal(format!("unsupported oauth provider: {}", pending.provider))
    })?;

    let bundle = exchange_code(provider, code, &pending.code_verifier, &pending.redirect_uri)?;
    save_oauth_tokens(&bundle)?;
    clear_pending(state)?;
    Ok(bundle)
}

/// Load stored OAuth tokens for a provider.
pub fn load_oauth_tokens(provider: &str) -> Result<Option<OAuthTokenBundle>> {
    let path = oauth_vault_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
    let vault: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&data)?;
    vault
        .get(provider)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .map(Ok)
        .unwrap_or(Ok(None))
}

pub fn save_oauth_tokens(bundle: &OAuthTokenBundle) -> Result<()> {
    crate::session::ensure_setup_dir()?;
    let path = oauth_vault_path();
    let mut vault: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    vault.insert(
        bundle.provider.clone(),
        serde_json::to_value(bundle)?,
    );
    let json = serde_json::to_string_pretty(&vault)?;
    fs::write(&path, json).map_err(SetupError::Io)?;
    Ok(())
}

fn save_pending(state: &str, pending: &PendingOAuth) -> Result<()> {
    crate::session::ensure_setup_dir()?;
    let path = pending_oauth_path();
    let mut map: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    map.insert(
        state.to_string(),
        serde_json::to_value(pending)?,
    );
    let json = serde_json::to_string_pretty(&map)?;
    fs::write(&path, json).map_err(SetupError::Io)
}

fn load_pending(state: &str) -> Result<Option<PendingOAuth>> {
    let path = pending_oauth_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&data)?;
    Ok(map
        .get(state)
        .and_then(|v| serde_json::from_value(v.clone()).ok()))
}

fn clear_pending(state: &str) -> Result<()> {
    let path = pending_oauth_path();
    if !path.exists() {
        return Ok(());
    }
    let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&data).unwrap_or_default();
    map.remove(state);
    let json = serde_json::to_string_pretty(&map)?;
    fs::write(&path, json).map_err(SetupError::Io)
}

fn load_pending_provider(state: &str) -> Option<String> {
    load_pending(state).ok().flatten().map(|p| p.provider)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DevicePending {
    provider: String,
    device_auth_id: String,
    user_code: String,
}

fn device_pending_path() -> PathBuf {
    setup_dir().join("oauth-device-pending.json")
}

fn save_device_pending(state: &str, device: &crate::oauth::providers::DeviceCodeStart) -> Result<()> {
    crate::session::ensure_setup_dir()?;
    let path = device_pending_path();
    let mut map: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
        serde_json::from_str(&data).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    map.insert(
        state.to_string(),
        serde_json::to_value(DevicePending {
            provider: "codex".into(),
            device_auth_id: device.device_auth_id.clone(),
            user_code: device.user_code.clone(),
        })
?,
    );
    let json = serde_json::to_string_pretty(&map)?;
    fs::write(&path, json).map_err(SetupError::Io)
}

fn take_device_pending(state: &str) -> Result<Option<DevicePending>> {
    let path = device_pending_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = fs::read_to_string(&path).map_err(SetupError::Io)?;
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&data).unwrap_or_default();
    let entry = map.remove(state);
    let json = serde_json::to_string_pretty(&map)?;
    fs::write(&path, json).map_err(SetupError::Io)?;
    Ok(entry.and_then(|v| serde_json::from_value(v).ok()))
}

fn clear_device_pending(state: &str) -> Result<()> {
    let _ = take_device_pending(state)?;
    Ok(())
}
