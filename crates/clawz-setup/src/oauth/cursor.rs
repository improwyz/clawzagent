//! Cursor authentication: import tokens from local Cursor IDE / CLI installs.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{Result, SetupError};
use crate::oauth::types::OAuthTokenBundle;
use crate::paths::clawz_home;

/// Try to load Cursor access + refresh tokens from a local installation.
pub fn try_import_local_cursor() -> Option<OAuthTokenBundle> {
    for path in cursor_state_db_paths() {
        if let Some(bundle) = read_tokens_from_sqlite(&path) {
            return Some(bundle);
        }
    }
    None
}

fn cursor_state_db_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        paths.push(
            PathBuf::from(&home)
                .join(".config/Cursor/User/globalStorage/state.vscdb"),
        );
        paths.push(
            PathBuf::from(&home)
                .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
        );
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        paths.push(
            PathBuf::from(appdata)
                .join("Cursor/User/globalStorage/state.vscdb"),
        );
    }
    let _ = clawz_home();
    paths
}

fn read_tokens_from_sqlite(db_path: &Path) -> Option<OAuthTokenBundle> {
    if !db_path.is_file() {
        return None;
    }
    let access = query_sqlite_value(db_path, "cursorAuth/accessToken")?;
    let refresh = query_sqlite_value(db_path, "cursorAuth/refreshToken");
    if access.is_empty() {
        return None;
    }
    Some(OAuthTokenBundle {
        provider: "cursor".into(),
        access_token: access,
        refresh_token: refresh.filter(|t| !t.is_empty()),
        id_token: None,
        account_id: None,
        expires_at: None,
        source: "import".into(),
    })
}

fn query_sqlite_value(db_path: &Path, key: &str) -> Option<String> {
    let output = Command::new("sqlite3")
        .arg(db_path)
        .arg(format!(
            "SELECT value FROM ItemTable WHERE key = '{key}' LIMIT 1;"
        ))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// Refresh a Cursor access token using the public refresh endpoint.
pub fn refresh_cursor_access_token(refresh_token: &str) -> Result<OAuthTokenBundle> {
    const CLIENT_ID: &str = "KbZUR41cY7W6zRSdpSUJ7I7mLYBKOCmB";
    const TOKEN_URL: &str = "https://api2.cursor.sh/oauth/token";

    let client = reqwest::blocking::Client::new();
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": CLIENT_ID,
        "refresh_token": refresh_token,
    });
    let response = client
        .post(TOKEN_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| SetupError::Internal(format!("cursor token refresh: {e}")))?;

    if !response.status().is_success() {
        let text = response.text().unwrap_or_default();
        return Err(SetupError::Internal(format!(
            "cursor token refresh failed: {text}"
        )));
    }

    let json: serde_json::Value = response
        .json()
        .map_err(|e| SetupError::Internal(format!("cursor refresh parse: {e}")))?;

    if json
        .get("shouldLogout")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Err(SetupError::Internal(
            "cursor refresh token expired; sign in via Cursor IDE or paste an API key".into(),
        ));
    }

    let access = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if access.is_empty() {
        return Err(SetupError::Internal(
            "cursor refresh returned empty access_token".into(),
        ));
    }

    Ok(OAuthTokenBundle {
        provider: "cursor".into(),
        access_token: access,
        refresh_token: refresh_token.to_string().into(),
        id_token: json
            .get("id_token")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        account_id: None,
        expires_at: None,
        source: "oauth".into(),
    })
}
