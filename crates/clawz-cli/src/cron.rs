//! `clawz cron` — manage scheduled agent jobs via the gateway API.

use anyhow::{Context, Result};
use clap::Subcommand;
use serde_json::json;

use crate::client::GatewayClient;
use crate::config;

#[derive(Subcommand)]
pub enum CronAction {
    /// List scheduled cron jobs.
    List,
    /// Add a cron job (`clawz cron add "0 9 * * *" "Daily summary" --agent-id my-agent`).
    Add {
        /// Cron expression (5-field: `min hour dom month dow`).
        cron_expr: String,
        /// Prompt sent to the agent each run.
        prompt: String,
        #[arg(long)]
        agent_id: Option<String>,
        #[arg(long)]
        name: Option<String>,
    },
    /// Run a job immediately by id.
    Run { job_id: String },
    /// Delete a cron job.
    Remove { job_id: String },
}

pub async fn run(action: CronAction) -> Result<()> {
    let cfg = config::resolve();
    let client = GatewayClient::new(&cfg);
    match action {
        CronAction::List => list(&client).await,
        CronAction::Add {
            cron_expr,
            prompt,
            agent_id,
            name,
        } => add(&client, &cron_expr, &prompt, agent_id, name).await,
        CronAction::Run { job_id } => run_job(&client, &job_id).await,
        CronAction::Remove { job_id } => remove(&client, &job_id).await,
    }
}

async fn list(client: &GatewayClient) -> Result<()> {
    let v = client.cron_list().await?;
    let jobs = v
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    if jobs.is_empty() {
        println!("No cron jobs.");
        return Ok(());
    }
    for job in jobs {
        let id = job.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let expr = job.get("cron_expr").and_then(|v| v.as_str()).unwrap_or("?");
        let agent = job.get("agent_id").and_then(|v| v.as_str()).unwrap_or("?");
        let enabled = job.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        println!("{id}  [{expr}]  agent={agent}  enabled={enabled}");
    }
    Ok(())
}

async fn add(
    client: &GatewayClient,
    cron_expr: &str,
    prompt: &str,
    agent_id: Option<String>,
    name: Option<String>,
) -> Result<()> {
    let agent_id = match agent_id {
        Some(id) => id,
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
    let body = json!({
        "cron_expr": cron_expr,
        "prompt": prompt,
        "agent_id": agent_id,
        "name": name,
        "enabled": true,
    });
    let job = client.cron_create(&body).await?;
    println!(
        "Created cron job {} ({})",
        job.get("id").and_then(|v| v.as_str()).unwrap_or("?"),
        cron_expr
    );
    Ok(())
}

async fn run_job(client: &GatewayClient, job_id: &str) -> Result<()> {
    let result = client.cron_run(job_id).await?;
    println!(
        "{}",
        result
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("(empty)")
    );
    Ok(())
}

async fn remove(client: &GatewayClient, job_id: &str) -> Result<()> {
    client.cron_delete(job_id).await?;
    println!("Deleted cron job {job_id}");
    Ok(())
}
