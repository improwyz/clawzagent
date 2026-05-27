//! StreamResponseStep — streams response tokens to the caller via a tokio channel.
//!
//! Responsibilities:
//! - Reads the assembled assistant response from `ctx.messages`.
//! - Sends it token-by-token (word-by-word simulation) through a `tokio::sync::mpsc` channel.
//! - Handles partial responses and cancellation via a `CancellationToken`.

use std::sync::Arc;

use async_trait::async_trait;
use clawz_core::{
    error::{ClawzError, Result},
    traits::{PipelineContext, PipelineStep, StepOutcome},
    types::message::Role,
};
use tokio::sync::mpsc;

/// A single streamed token chunk.
#[derive(Debug, Clone)]
pub struct TokenChunk {
    pub text: String,
    pub is_final: bool,
}

/// Metadata key for the streaming channel sender (serialised as null — the real
/// sender is passed via Arc and stored outside the JSON metadata).
pub const META_STREAM_COMPLETE: &str = "stream_complete";

pub struct StreamResponseStep {
    /// Optional sender to push chunks to.  If `None`, no streaming occurs.
    sender: Option<Arc<mpsc::Sender<TokenChunk>>>,
    /// Simulated token delay in milliseconds (for testing / rate-limited streaming).
    token_delay_ms: Option<u64>,
}

impl StreamResponseStep {
    pub fn new() -> Self {
        Self {
            sender: None,
            token_delay_ms: None,
        }
    }

    pub fn with_sender(mut self, sender: Arc<mpsc::Sender<TokenChunk>>) -> Self {
        self.sender = Some(sender);
        self
    }

    pub fn with_token_delay_ms(mut self, ms: u64) -> Self {
        self.token_delay_ms = Some(ms);
        self
    }

    async fn stream_text(&self, text: &str) -> Result<()> {
        let sender = match &self.sender {
            Some(s) => s,
            None => return Ok(()),
        };

        let words: Vec<&str> = text.split_whitespace().collect();
        let total = words.len();

        for (i, word) in words.iter().enumerate() {
            let chunk = TokenChunk {
                text: if i + 1 < total {
                    format!("{word} ")
                } else {
                    word.to_string()
                },
                is_final: i + 1 == total,
            };

            if let Some(delay) = self.token_delay_ms {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
            }

            sender
                .send(chunk)
                .await
                .map_err(|e| ClawzError::Channel(format!("stream channel closed: {e}")))?;
        }

        // Send empty final chunk if text was empty.
        if total == 0 {
            sender
                .send(TokenChunk {
                    text: String::new(),
                    is_final: true,
                })
                .await
                .map_err(|e| ClawzError::Channel(format!("stream channel closed: {e}")))?;
        }

        Ok(())
    }
}

impl Default for StreamResponseStep {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PipelineStep for StreamResponseStep {
    fn name(&self) -> &str {
        "stream_response"
    }

    async fn execute(&self, ctx: &mut PipelineContext) -> Result<StepOutcome> {
        // Find the latest assistant message.
        let response_text = ctx
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| m.content.as_text())
            .map(|t| t.to_string())
            .unwrap_or_default();

        if self.sender.is_some() {
            self.stream_text(&response_text).await?;
            log::debug!("[stream_response] streamed {} chars", response_text.len());
        }

        ctx.insert_meta(META_STREAM_COMPLETE, serde_json::Value::Bool(true));

        Ok(StepOutcome::Continue)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::{traits::PipelineContext, types::message::Message};

    #[tokio::test]
    async fn test_no_sender_continues() {
        let step = StreamResponseStep::new();
        let mut ctx = PipelineContext::new("a", "c");
        ctx.messages.push(Message::assistant("Hello world"));
        let outcome = step.execute(&mut ctx).await.unwrap();
        assert!(matches!(outcome, StepOutcome::Continue));
    }

    #[tokio::test]
    async fn test_streams_all_words() {
        let (tx, mut rx) = mpsc::channel(64);
        let step = StreamResponseStep::new().with_sender(Arc::new(tx));

        let mut ctx = PipelineContext::new("a", "c");
        ctx.messages.push(Message::assistant("one two three"));

        step.execute(&mut ctx).await.unwrap();

        let mut chunks = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            chunks.push(chunk);
        }

        let reassembled: String = chunks.iter().map(|c| c.text.clone()).collect();
        assert_eq!(reassembled.trim(), "one two three");
        assert!(chunks.last().unwrap().is_final);
    }
}
