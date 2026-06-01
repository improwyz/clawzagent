use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderType {
    Anthropic,
    OpenAiCompat,
}

#[derive(Debug, Clone, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    stream: bool,
    system: String,
    messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiRequest {
    model: String,
    max_tokens: u32,
    stream: bool,
    messages: Vec<Message>,
}

pub struct LocalLlmClient {
    provider: ProviderType,
    api_key: String,
    base_url: String,
    model: String,
    client: reqwest::Client,
    system_prompt: String,
}

impl LocalLlmClient {
    pub fn new(
        provider: ProviderType,
        api_key: String,
        base_url: String,
        provider_name: &str,
    ) -> Self {
        let model = match provider_name {
            "anthropic" => "claude-sonnet-4-5".to_string(),
            "openai" => "gpt-4o".to_string(),
            "openrouter" => "anthropic/claude-sonnet-4-5".to_string(),
            "groq" => "llama-3.3-70b-versatile".to_string(),
            "xai" => "grok-2".to_string(),
            "deepseek" => "deepseek-chat".to_string(),
            "together" => "meta-llama/Llama-3.3-70B-Instruct-Turbo".to_string(),
            "fireworks" => "accounts/fireworks/models/llama-v3p3-70b-instruct".to_string(),
            "ollama" => "llama3".to_string(),
            _ => "gpt-4o".to_string(),
        };

        Self {
            provider,
            api_key,
            base_url,
            model,
            client: reqwest::Client::new(),
            system_prompt: String::new(),
        }
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    pub fn has_key(&self) -> bool {
        !self.api_key.is_empty()
    }

    pub async fn send_streaming(
        &self,
        messages: &[Message],
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<(), String> {
        if !self.has_key() {
            let _ = tx.send("[No API key configured — running in offline mode. Setup will proceed with defaults.]\n".to_string());
            return Ok(());
        }

        match &self.provider {
            ProviderType::Anthropic => self.send_anthropic(messages, tx).await,
            ProviderType::OpenAiCompat => self.send_openai_compat(messages, tx).await,
        }
    }

    async fn send_anthropic(
        &self,
        messages: &[Message],
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<(), String> {
        let url = format!("{}/messages", self.base_url);
        let body = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 2048,
            stream: true,
            system: self.system_prompt.clone(),
            messages: messages.to_vec(),
        };

        let resp = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("API error {status}: {body}"));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Stream error: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf = buf[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        if event["type"] == "content_block_delta" {
                            if let Some(text) = event["delta"]["text"].as_str() {
                                let _ = tx.send(text.to_string());
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn send_openai_compat(
        &self,
        messages: &[Message],
        tx: mpsc::UnboundedSender<String>,
    ) -> Result<(), String> {
        let url = format!("{}/chat/completions", self.base_url);

        let mut all_messages = vec![Message {
            role: "system".to_string(),
            content: self.system_prompt.clone(),
        }];
        all_messages.extend_from_slice(messages);

        let body = OpenAiRequest {
            model: self.model.clone(),
            max_tokens: 2048,
            stream: true,
            messages: all_messages,
        };

        let resp = self
            .client
            .post(&url)
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("API error {status}: {body}"));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Stream error: {e}"))?;
            buf.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].to_string();
                buf = buf[pos + 1..].to_string();

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(text) =
                            event["choices"][0]["delta"]["content"].as_str()
                        {
                            let _ = tx.send(text.to_string());
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
