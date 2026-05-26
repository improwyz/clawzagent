//! Tauri IPC command handlers — bridge between frontend and ClawZ agent.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

/// Chat request from frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub message: String,
    pub agent_id: Option<String>,
}

/// Chat response from agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub message: String,
    pub agent_id: String,
    pub tier: String,
}

/// Error type for IPC commands
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("Agent error: {0}")]
    AgentError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
}

impl From<String> for CommandError {
    fn from(s: String) -> Self {
        CommandError::AgentError(s)
    }
}

impl From<anyhow::Error> for CommandError {
    fn from(e: anyhow::Error) -> Self {
        CommandError::AgentError(e.to_string())
    }
}

/// Handle a chat message from the frontend UI.
/// This is the primary IPC bridge — the frontend calls this via `invoke()`.
#[tauri::command]
pub async fn agent_chat(
    _app: AppHandle,
    req: ChatRequest,
) -> Result<ChatResponse, String> {
    // In a full implementation, this would get ClawzWorker from app state
    // and call worker.chat(req.message, agent_id).await

    Ok(ChatResponse {
        message: format!("ClawZ received: {}", req.message),
        agent_id: req.agent_id.unwrap_or_else(|| "default".to_string()),
        tier: "T3".to_string(),
    })
}

/// Get current platform tier
#[tauri::command]
pub fn get_platform_tier() -> String {
    "T3".to_string()
}

/// Health check endpoint for Tauri
#[tauri::command]
pub fn health_check() -> Result<String, String> {
    Ok("ClawZ Tauri shell healthy".to_string())
}