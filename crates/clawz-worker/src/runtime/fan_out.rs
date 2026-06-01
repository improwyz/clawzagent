//! Parallel fan-out execution — send the same prompt to multiple providers
//! and aggregate results (Mixture-of-Agents).
//!
//! # Fan-out pattern
//! [`FanOut`] clones a [`ChatRequest`] for each configured model, routes
//! them concurrently through [`ProviderRouter`], and merges the successful
//! responses using a configurable [`AggregationStrategy`].
//!
//! # Mixture-of-Agents (MoA)
//! [`MixtureOfAgents`] extends fan-out with a "judge" model: after collecting
//! responses from several workers, it forwards them to a synthesis model
//! that produces a single best answer.
//!
//! # Why this module exists
//! Running the same prompt against multiple models improves robustness
//! (one provider may be down or rate-limited) and quality (different models
//! have different strengths).  The aggregation layer lets callers choose
//! the trade-off between latency and answer quality.
//!
//! # Dependencies
//! - `crate::providers::router::ProviderRouter` — routes requests to concrete providers.
//! - `clawz_core::types::message::{ChatRequest, ChatResponse, Message}` — request/response types.

use std::sync::Arc;
use std::time::Duration;

// Dependency: core error types and message primitives.
use clawz_core::{
    error::{ClawzError, Result},
    types::message::{ChatRequest, ChatResponse, Message},
};
// Dependency: concurrent future joining for parallel provider calls.
use futures_util::future::join_all;

// Dependency: provider router from the worker crate's provider module.
use crate::providers::router::ProviderRouter;

// ── AggregationStrategy ───────────────────────────────────────────────────────

/// How to combine multiple provider responses into a single [`Message`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationStrategy {
    /// Use the first successful response.
    /// Fastest but ignores diversity; good when latency matters most.
    First,
    /// Concatenate all successful response texts.
    /// Preserves every model's output; the caller or downstream steps
    /// can re-process the combined text.
    Concatenate,
    /// Pick the longest response.
    /// Heuristic: longer answers often contain more detail.
    Longest,
    /// Pick the response with the most tokens.
    /// Uses the provider-reported `usage.total_tokens` as the signal.
    MostTokens,
}

// ── FanOutConfig ──────────────────────────────────────────────────────────────

/// Configuration for a [`FanOut`] run.
#[derive(Debug, Clone)]
pub struct FanOutConfig {
    /// Models to fan-out to. Each model is called in parallel.
    pub models: Vec<String>,
    /// Maximum number of concurrent calls.
    ///
    /// A semaphore enforces this limit; extra requests wait for a permit.
    pub max_concurrency: usize,
    /// Per-call timeout.
    ///
    /// If a single provider hangs, the overall fan-out still completes
    /// because `tokio::time::timeout` isolates each call.
    pub timeout: Duration,
    /// Aggregation strategy applied to successful responses.
    pub strategy: AggregationStrategy,
}

impl Default for FanOutConfig {
    fn default() -> Self {
        Self {
            models: Vec::new(),
            max_concurrency: 4,
            timeout: Duration::from_secs(30),
            strategy: AggregationStrategy::First,
        }
    }
}

// ── FanOutResult ──────────────────────────────────────────────────────────────

/// Outcome of a [`FanOut::execute`] call.
#[derive(Debug)]
pub struct FanOutResult {
    /// Raw results per model, preserving errors so callers can inspect them.
    pub model_results: Vec<(String, Result<ChatResponse>)>,
    /// Aggregated message, if at least one model succeeded.
    pub aggregated: Option<Message>,
}

impl FanOutResult {
    /// Return only the successful `(model, response)` pairs.
    pub fn successful_responses(&self) -> Vec<(&str, &ChatResponse)> {
        self.model_results
            .iter()
            .filter_map(|(m, r)| r.as_ref().ok().map(|resp| (m.as_str(), resp)))
            .collect()
    }
}

// ── FanOut ─────────────────────────────────────────────────────────────────────

/// Parallel multi-model executor.
///
/// Holds an `Arc<ProviderRouter>` so the same router instance can be shared
/// across many fan-out calls without duplication.
pub struct FanOut {
    /// Shared provider router; routes each cloned request to the correct backend.
    router: Arc<ProviderRouter>,
    /// Configuration for this fan-out (models, concurrency, timeout, strategy).
    config: FanOutConfig,
}

impl FanOut {
    /// Create a new fan-out executor.
    pub fn new(router: Arc<ProviderRouter>, config: FanOutConfig) -> Self {
        Self { router, config }
    }

    /// Fan out `base_request` to all configured models in parallel.
    ///
    /// # Concurrency control
    /// A `tokio::sync::Semaphore` limits the number of in-flight requests.
    /// This protects both the remote providers and the local tokio runtime
    /// from unbounded parallelism.
    ///
    /// # Timeout behaviour
    /// Each individual call is wrapped in `tokio::time::timeout`.  A timeout
    /// is treated as an `Err` for that model but does not abort the others.
    pub async fn execute(&self, base_request: ChatRequest) -> FanOutResult {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrency));

        let futures: Vec<_> = self
            .config
            .models
            .iter()
            .map(|model| {
                let router = self.router.clone();
                let sem = semaphore.clone();
                let timeout = self.config.timeout;
                let mut req = base_request.clone();
                req.model = model.clone();
                let model_clone = model.clone();

                async move {
                    let _permit = sem.acquire().await;
                    let result = tokio::time::timeout(timeout, router.route(req))
                        .await
                        .map_err(|_| {
                            ClawzError::Internal(format!(
                                "model '{model_clone}' timed out after {timeout:?}"
                            ))
                        })
                        .and_then(|r| r);
                    (model_clone, result)
                }
            })
            .collect();

        let model_results = join_all(futures).await;
        let aggregated = self.aggregate(&model_results);

        FanOutResult {
            model_results,
            aggregated,
        }
    }

    /// Aggregate the responses according to the configured strategy.
    ///
    /// Returns `None` when every model failed, so callers must handle the
    /// empty-success case explicitly.
    fn aggregate(&self, results: &[(String, Result<ChatResponse>)]) -> Option<Message> {
        let successes: Vec<(&str, &ChatResponse)> = results
            .iter()
            .filter_map(|(m, r)| r.as_ref().ok().map(|resp| (m.as_str(), resp)))
            .collect();

        if successes.is_empty() {
            return None;
        }

        let text = match self.config.strategy {
            AggregationStrategy::First => successes
                .first()
                .and_then(|(_, r)| r.first_text())
                .map(|t| t.to_string())?,

            AggregationStrategy::Concatenate => {
                let parts: Vec<String> = successes
                    .iter()
                    .filter_map(|(model, resp)| {
                        resp.first_text().map(|t| format!("[{model}]: {t}"))
                    })
                    .collect();
                parts.join("\n\n")
            }

            AggregationStrategy::Longest => successes
                .iter()
                .filter_map(|(_, r)| r.first_text())
                .max_by_key(|t| t.len())
                .map(|t| t.to_string())?,

            AggregationStrategy::MostTokens => successes
                .iter()
                .max_by_key(|(_, r)| r.usage.total_tokens)
                .and_then(|(_, r)| r.first_text())
                .map(|t| t.to_string())?,
        };

        Some(Message::assistant(text))
    }
}

// ── MixtureOfAgents ────────────────────────────────────────────────────────────

/// Mixture-of-Agents: fan out, then synthesise the results with a "judge" model.
///
/// Implements the paper-style MoA pattern: multiple "worker" models
/// generate candidate answers in parallel, then a single "judge" model
/// (usually a stronger, slower model) reads all candidates and produces
/// the final response.
pub struct MixtureOfAgents {
    /// Underlying fan-out executor for the worker models.
    fan_out: FanOut,
    /// Model identifier used for the synthesis pass.
    judge_model: String,
}

impl MixtureOfAgents {
    /// Create a new MoA executor.
    pub fn new(
        router: Arc<ProviderRouter>,
        config: FanOutConfig,
        judge_model: impl Into<String>,
    ) -> Self {
        Self {
            fan_out: FanOut::new(router.clone(), config),
            judge_model: judge_model.into(),
        }
    }

    /// Run fan-out and synthesise with the judge model.
    ///
    /// # Errors
    /// - Returns [`ClawzError::Provider`] if every worker model fails.
    /// - Returns [`ClawzError::Internal`] if the judge model returns an empty response.
    pub async fn run(&self, base_request: ChatRequest) -> Result<Message> {
        let fan_result = self.fan_out.execute(base_request.clone()).await;

        let successes = fan_result.successful_responses();
        if successes.is_empty() {
            return Err(ClawzError::Provider("all fan-out models failed".into()));
        }

        // Build synthesis prompt.
        // We prepend an explicit role instruction so the judge model knows
        // it is performing a meta-synthesis rather than answering raw.
        let mut synthesis_parts: Vec<String> = vec![
            "You are a synthesis model. Given multiple model responses below, \
             produce a single best answer:\n"
                .to_string(),
        ];

        for (i, (model, resp)) in successes.iter().enumerate() {
            if let Some(text) = resp.first_text() {
                synthesis_parts.push(format!("Response {} (from {}):\n{}", i + 1, model, text));
            }
        }

        synthesis_parts.push("\nSynthesise these into the best possible response:".to_string());

        let synthesis_prompt = synthesis_parts.join("\n\n");
        let mut synthesis_req = base_request;
        synthesis_req.model = self.judge_model.clone();
        synthesis_req.messages = vec![Message::user(synthesis_prompt)];

        let resp = self.fan_out.router.route(synthesis_req).await?;
        let text = resp
            .first_text()
            .ok_or_else(|| ClawzError::Internal("judge model returned empty response".into()))?
            .to_string();

        Ok(Message::assistant(text))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::message::{ChatResponse, Message, Usage};

    #[test]
    fn test_aggregation_first() {
        let resp = mock_response("hello", 5);
        let results: Vec<(String, Result<ChatResponse>)> = vec![("gpt-4".into(), Ok(resp))];

        let config = FanOutConfig {
            strategy: AggregationStrategy::First,
            ..Default::default()
        };

        let router = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(ProviderRouter::new(
                    crate::providers::ProviderRouterConfig::default(),
                ))
                .unwrap(),
        );

        let fan = FanOut::new(router, config);
        let agg = fan.aggregate(&results);
        assert_eq!(agg.unwrap().content.as_text().unwrap(), "hello");
    }

    #[test]
    fn test_aggregation_longest() {
        let short = mock_response("hi", 2);
        let long = mock_response("hello world this is longer", 10);
        let results: Vec<(String, Result<ChatResponse>)> =
            vec![("m1".into(), Ok(short)), ("m2".into(), Ok(long))];

        let config = FanOutConfig {
            strategy: AggregationStrategy::Longest,
            ..Default::default()
        };

        let router = Arc::new(
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(ProviderRouter::new(
                    crate::providers::ProviderRouterConfig::default(),
                ))
                .unwrap(),
        );

        let fan = FanOut::new(router, config);
        let agg = fan.aggregate(&results);
        assert_eq!(
            agg.unwrap().content.as_text().unwrap(),
            "hello world this is longer"
        );
    }

    fn mock_response(text: &str, tokens: usize) -> ChatResponse {
        ChatResponse {
            id: "test".into(),
            model: "test".into(),
            choices: vec![clawz_core::types::message::ChatChoice {
                index: 0,
                message: Message::assistant(text),
                finish_reason: Some("stop".into()),
            }],
            usage: Usage::new(tokens, tokens),
            ..Default::default()
        }
    }
}
