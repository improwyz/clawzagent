//! Embedding pipeline: providers, chunking, and batch embedding.
//!
//! This module abstracts over remote (OpenAI) and local (Ollama) embedding
//! models, and provides a sentence-aware text chunker for RAG ingestion.
//!
//! // Dependency: `clawz_core::error::ClawzError` for unified error reporting.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use clawz_core::error::{ClawzError, Result};

// ── EmbeddingProvider trait ───────────────────────────────────────────────────

/// Abstraction over any embedding model.
///
/// Implementors must be `Send + Sync` so they can be shared across async tasks.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string and return a float vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed multiple texts in one (potentially batched) API call.
    ///
    /// The default implementation falls back to sequential `embed` calls;
    /// adapters may override this to exploit provider batch endpoints.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// The dimension of the embedding vectors produced by this provider.
    fn dimensions(&self) -> usize;
}

// ── OpenAIEmbedding ───────────────────────────────────────────────────────────

/// Calls the OpenAI `/v1/embeddings` endpoint.
///
/// Supports both OpenAI and Azure OpenAI via [`with_base_url`].
pub struct OpenAIEmbedding {
    /// API key or token for authentication.
    api_key: String,
    /// Model identifier (e.g. `text-embedding-3-small`).
    model: String,
    /// Expected output dimension (must match the model).
    dimensions: usize,
    /// Reusable HTTP client.
    client: reqwest::Client,
    /// Base URL for the API (allows proxy or Azure overrides).
    base_url: String,
}

#[derive(Serialize)]
struct OpenAIEmbedRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

#[derive(Deserialize)]
struct OpenAIEmbedResponse {
    data: Vec<OpenAIEmbedData>,
}

#[derive(Deserialize)]
struct OpenAIEmbedData {
    embedding: Vec<f32>,
}

impl OpenAIEmbedding {
    /// Create a new provider with the given API key and default model.
    ///
    /// Defaults:
    /// - model: `text-embedding-3-small`
    /// - dimensions: `1536`
    /// - base_url: `https://api.openai.com`
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: "text-embedding-3-small".into(),
            dimensions: 1536,
            client: reqwest::Client::new(),
            base_url: "https://api.openai.com".into(),
        }
    }

    /// Select a different model and its corresponding dimension.
    pub fn with_model(mut self, model: impl Into<String>, dimensions: usize) -> Self {
        self.model = model.into();
        self.dimensions = dimensions;
        self
    }

    /// Override base URL (useful for Azure OpenAI or proxy setups).
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Shared HTTP helper for the embeddings endpoint.
    async fn embed_texts(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = OpenAIEmbedRequest {
            model: &self.model,
            input: texts.to_vec(),
        };
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "OpenAI embeddings error {status}: {body}"
            )));
        }

        let parsed: OpenAIEmbedResponse = response
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;

        Ok(parsed.data.into_iter().map(|d| d.embedding).collect())
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut results = self.embed_texts(&[text]).await?;
        results
            .pop()
            .ok_or_else(|| ClawzError::Provider("empty embedding response".into()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        // OpenAI accepts up to 2048 texts per request; chunk conservatively.
        const BATCH_SIZE: usize = 512;
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH_SIZE) {
            let batch = self.embed_texts(chunk).await?;
            all.extend(batch);
        }
        Ok(all)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ── LocalEmbedding (Ollama) ───────────────────────────────────────────────────

/// Calls a local [Ollama](https://ollama.ai) embedding endpoint.
///
/// Useful for fully offline or air-gapped deployments.
pub struct LocalEmbedding {
    /// Model name (e.g. `nomic-embed-text`).
    model: String,
    /// Expected vector dimension.
    dimensions: usize,
    /// Reusable HTTP client.
    client: reqwest::Client,
    /// Base URL for the Ollama server (default: `http://localhost:11434`).
    base_url: String,
}

#[derive(Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embedding: Vec<f32>,
}

impl LocalEmbedding {
    pub fn new(model: impl Into<String>, dimensions: usize) -> Self {
        Self {
            model: model.into(),
            dimensions,
            client: reqwest::Client::new(),
            base_url: "http://localhost:11434".into(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }
}

#[async_trait]
impl EmbeddingProvider for LocalEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.base_url);
        let body = OllamaEmbedRequest {
            model: &self.model,
            prompt: text,
        };
        let response = self.client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ClawzError::Provider(format!(
                "Ollama embeddings error {status}: {body}"
            )));
        }

        let parsed: OllamaEmbedResponse = response
            .json()
            .await
            .map_err(|e| ClawzError::Serialization(e.to_string()))?;
        Ok(parsed.embedding)
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ── Text chunker ─────────────────────────────────────────────────────────────

/// Splits text into overlapping chunks suitable for embedding.
///
/// Strategy:
/// 1. Split into sentences on `.`, `!`, `?`, `\n`.
/// 2. Accumulate sentences into chunks not exceeding `max_tokens`.
/// 3. When a chunk is full, carry `overlap_tokens` worth of text into the
///    next chunk for context continuity.
pub struct TextChunker {
    /// Maximum approximate token count per chunk (1 token ≈ 4 chars).
    pub max_tokens: usize,
    /// Overlap in approximate tokens between consecutive chunks.
    pub overlap_tokens: usize,
}

impl Default for TextChunker {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            overlap_tokens: 64,
        }
    }
}

impl TextChunker {
    pub fn new(max_tokens: usize, overlap_tokens: usize) -> Self {
        Self {
            max_tokens,
            overlap_tokens,
        }
    }

    /// Split `text` into a list of chunk strings.
    ///
    /// Strategy:
    /// 1. Split into sentences on `.`, `!`, `?`, `\n`.
    /// 2. Accumulate sentences into chunks not exceeding `max_tokens`.
    /// 3. When a chunk is full, carry `overlap_tokens` worth of text into the
    ///    next chunk for context continuity.
    pub fn chunk(&self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return vec![];
        }

        let max_chars = self.max_tokens * 4;
        let overlap_chars = self.overlap_tokens * 4;

        // Split on sentence boundaries.
        let sentences = split_sentences(text);

        let mut chunks: Vec<String> = Vec::new();
        let mut current = String::new();

        for sentence in &sentences {
            // If adding this sentence would exceed the limit, flush current chunk.
            if !current.is_empty() && current.len() + sentence.len() + 1 > max_chars {
                chunks.push(current.trim().to_string());
                // Carry the tail for overlap.
                let tail: String = current
                    .chars()
                    .rev()
                    .take(overlap_chars)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
                current = tail;
                if !current.is_empty() {
                    current.push(' ');
                }
            }
            // Hard-split sentences that are longer than max_chars.
            if sentence.len() > max_chars {
                for sub in sentence.as_bytes().chunks(max_chars) {
                    let s = String::from_utf8_lossy(sub).into_owned();
                    if !current.is_empty() {
                        chunks.push(current.trim().to_string());
                        current = String::new();
                    }
                    chunks.push(s.trim().to_string());
                }
                continue;
            }
            current.push_str(sentence);
            current.push(' ');
        }

        if !current.trim().is_empty() {
            chunks.push(current.trim().to_string());
        }

        chunks
    }
}

/// Split text into sentence-like units using punctuation heuristics.
///
/// This is intentionally simple; it does not handle abbreviations or nested
/// punctuation perfectly, but is sufficient for chunking.
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut buf = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        buf.push(ch);
        if matches!(ch, '.' | '!' | '?' | '\n') {
            // Look-ahead: if next char is whitespace or end, flush.
            let next_is_end = i + 1 >= chars.len();
            let next_is_ws = i + 1 < chars.len() && chars[i + 1].is_whitespace();
            if next_is_end || next_is_ws {
                let trimmed = buf.trim().to_string();
                if !trimmed.is_empty() {
                    sentences.push(trimmed);
                }
                buf.clear();
            }
        }
        i += 1;
    }
    if !buf.trim().is_empty() {
        sentences.push(buf.trim().to_string());
    }
    sentences
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunker_basic() {
        let chunker = TextChunker::new(10, 2); // 10 tokens = 40 chars
        let text = "Hello world. This is a test. Another sentence here. And one more.";
        let chunks = chunker.chunk(text);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.len() <= 44, "chunk too long: {chunk}");
        }
    }

    #[test]
    fn test_chunker_empty() {
        let chunker = TextChunker::default();
        let chunks = chunker.chunk("");
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunker_short_text() {
        let chunker = TextChunker::default();
        let text = "Short text.";
        let chunks = chunker.chunk(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Short text.");
    }

    #[test]
    fn test_split_sentences() {
        let sentences = split_sentences("Hello world. How are you? I am fine!");
        assert!(!sentences.is_empty());
        assert!(sentences.len() >= 2);
    }
}
