//! HTTP client for gateway and worker health checks.

use anyhow::{Context, Result};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

use crate::config::{self, CliConfig};

pub struct GatewayClient {
    http: reqwest::Client,
    api_base: String,
    api_key: Option<String>,
}

impl GatewayClient {
    pub fn new(cfg: &CliConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_base: config::api_v1_base(cfg),
            api_key: cfg.api_key.clone(),
        }
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            req.header(AUTHORIZATION, format!("Bearer {key}"))
        } else {
            req
        }
    }

    pub async fn system_health(&self) -> Result<Value> {
        let api_url = format!("{}/system/health", self.api_base);
        if let Ok(v) = self.get_json(&api_url).await {
            return Ok(v);
        }

        let origin = self
            .api_base
            .trim_end_matches("/api/v1")
            .trim_end_matches('/');
        let root_url = format!("{origin}/health");
        self.get_json(&root_url)
            .await
            .context("gateway health (/api/v1/system/health and /health)")
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let res = self
            .auth(self.http.get(url))
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = body.chars().take(120).collect();
            anyhow::bail!("HTTP {status}: {snippet}");
        }
        serde_json::from_str(&body).context("parse JSON")
    }

    pub async fn run_agent_turn(
        &self,
        agent_id: &str,
        message: &str,
        conversation_id: Option<&str>,
    ) -> Result<Value> {
        let url = format!("{}/agents/{agent_id}/run", self.api_base);
        let mut body = serde_json::json!({ "message": message });
        if let Some(cid) = conversation_id {
            body["conversation_id"] = Value::String(cid.to_string());
        }
        let res = self
            .auth(
                self.http
                    .post(&url)
                    .header(CONTENT_TYPE, "application/json")
                    .json(&body),
            )
            .send()
            .await
            .context("agent run request")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("agent run HTTP {status}: {text}");
        }
        serde_json::from_str(&text).context("parse agent run response")
    }

    pub async fn cron_list(&self) -> Result<Value> {
        let url = format!("{}/cron/jobs", self.api_base);
        self.get_json(&url).await
    }

    pub async fn cron_create(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/cron/jobs", self.api_base);
        let res = self
            .auth(
                self.http
                    .post(&url)
                    .header(CONTENT_TYPE, "application/json")
                    .json(body),
            )
            .send()
            .await
            .context("cron create")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("cron create HTTP {status}: {text}");
        }
        serde_json::from_str(&text).context("parse cron create")
    }

    pub async fn cron_run(&self, job_id: &str) -> Result<Value> {
        let url = format!("{}/cron/jobs/{job_id}/run", self.api_base);
        let res = self
            .auth(self.http.post(&url))
            .send()
            .await
            .context("cron run")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("cron run HTTP {status}: {text}");
        }
        serde_json::from_str(&text).context("parse cron run")
    }

    pub async fn cron_delete(&self, job_id: &str) -> Result<()> {
        let url = format!("{}/cron/jobs/{job_id}", self.api_base);
        let res = self
            .auth(self.http.delete(&url))
            .send()
            .await
            .context("cron delete")?;
        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            anyhow::bail!("cron delete HTTP {status}: {text}");
        }
        Ok(())
    }

    pub async fn list_agents(&self) -> Result<Vec<Value>> {
        let url = format!("{}/agents", self.api_base);
        let res = self
            .auth(self.http.get(&url))
            .send()
            .await
            .context("list agents")?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("list agents HTTP {status}: {text}");
        }
        let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        Ok(match parsed {
            Value::Array(a) => a,
            Value::Object(o) if o.get("data").and_then(|d| d.as_array()).is_some() => {
                o["data"].as_array().cloned().unwrap_or_default()
            }
            _ => Vec::new(),
        })
    }
}

pub async fn worker_health(worker_url: &str) -> Result<Value> {
    let base = worker_url.trim_end_matches('/');
    let url = format!("{base}/health");
    let res = reqwest::get(&url)
        .await
        .with_context(|| format!("worker health GET {url}"))?;
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!("worker health HTTP {status}: {body}");
    }
    serde_json::from_str(&body).context("parse worker health")
}
