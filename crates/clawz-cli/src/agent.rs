//! `clawz agent --message "..."`

use anyhow::{Context, Result};

use crate::client::GatewayClient;
use crate::config;

pub async fn run(
    message: &str,
    agent_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Result<()> {
    let cfg = config::resolve();
    let client = GatewayClient::new(&cfg);

    let agent_id = match agent_id {
        Some(id) => id.to_string(),
        None => {
            let agents = client.list_agents().await.context("list agents")?;
            if let Some(first) = agents.first() {
                first
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .context("first agent missing id")?
            } else {
                anyhow::bail!(
                    "no agents found — create one via the dashboard or POST /api/v1/agents"
                );
            }
        }
    };

    let resp = client
        .run_agent_turn(&agent_id, message, conversation_id)
        .await?;

    let content = resp
        .get("content")
        .and_then(|v| v.as_str())
        .or_else(|| resp.get("message").and_then(|v| v.as_str()))
        .map(str::to_string)
        .unwrap_or_else(|| resp.to_string());

    let conv = resp
        .get("conversation_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    println!("{content}");
    if !conv.is_empty() {
        eprintln!("\n(conversation_id: {conv})");
    }
    Ok(())
}
