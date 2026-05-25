//! Cost-tracking and budget-control types.
//!
//! `CostRecord` is produced by provider adapters after every LLM call.
//! `BudgetConfig` is checked by the governance engine before allowing
//! expensive operations. `ModelPricing` provides the built-in pricing table.
//!
//! // Dependency: used by worker::provider, worker::governance, gateway::billing_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── CostRecord ────────────────────────────────────────────────────────────────

/// A single chargeable LLM request.
/// // Dependency: stored in db::CostRepo, emitted in metrics::record_request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostRecord {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Total cost in US dollars for this single request.
    pub cost_usd: f64,
    pub timestamp: DateTime<Utc>,
}

impl CostRecord {
    pub fn new(
        agent_id: Uuid,
        provider: impl Into<String>,
        model: impl Into<String>,
        input_tokens: u64,
        output_tokens: u64,
        cost_usd: f64,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            agent_id,
            provider: provider.into(),
            model: model.into(),
            input_tokens,
            output_tokens,
            cost_usd,
            timestamp: Utc::now(),
        }
    }

    /// Compute cost from pricing table.
    pub fn from_pricing(
        agent_id: Uuid,
        provider: impl Into<String>,
        pricing: &ModelPricing,
        input_tokens: u64,
        output_tokens: u64,
    ) -> Self {
        let cost = pricing.compute_cost(input_tokens, output_tokens);
        Self::new(agent_id, provider, &pricing.model, input_tokens, output_tokens, cost)
    }
}

// ── BudgetConfig ──────────────────────────────────────────────────────────────

/// Spend limits and alerting thresholds for an agent or tenant.
/// // Dependency: checked by worker::governance before approving actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_daily_usd: f64,
    pub max_monthly_usd: f64,
    /// Percentage (0–100) at which to send an alert before hitting the limit.
    pub alert_threshold_pct: f32,
    /// Whether to hard-stop agent activity when the limit is reached.
    pub hard_limit: bool,
}

impl BudgetConfig {
    pub fn new(max_daily_usd: f64, max_monthly_usd: f64) -> Self {
        Self {
            max_daily_usd,
            max_monthly_usd,
            alert_threshold_pct: 80.0,
            hard_limit: true,
        }
    }

    /// Returns `true` if `spent` exceeds the alert threshold for `budget`.
    pub fn is_alert_threshold(&self, spent: f64, budget: f64) -> bool {
        if budget <= 0.0 {
            return false;
        }
        (spent / budget) * 100.0 >= self.alert_threshold_pct as f64
    }

    /// Returns `true` if `spent` has reached the hard limit for `budget`.
    pub fn is_over_budget(&self, spent: f64, budget: f64) -> bool {
        self.hard_limit && spent >= budget
    }
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self::new(10.0, 200.0)
    }
}

// ── ModelPricing ──────────────────────────────────────────────────────────────

/// Per-model pricing information.
/// // Dependency: used by CostRecord::from_pricing and worker::provider::estimate_cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model: String,
    pub provider: String,
    /// Cost per 1 000 input (prompt) tokens in USD.
    pub input_cost_per_1k: f64,
    /// Cost per 1 000 output (completion) tokens in USD.
    pub output_cost_per_1k: f64,
    /// Maximum context window in tokens.
    pub context_window: u32,
}

impl ModelPricing {
    pub fn new(
        model: impl Into<String>,
        provider: impl Into<String>,
        input_cost_per_1k: f64,
        output_cost_per_1k: f64,
        context_window: u32,
    ) -> Self {
        Self {
            model: model.into(),
            provider: provider.into(),
            input_cost_per_1k,
            output_cost_per_1k,
            context_window,
        }
    }

    /// Compute the dollar cost for a given token usage.
    pub fn compute_cost(&self, input_tokens: u64, output_tokens: u64) -> f64 {
        let input_cost = (input_tokens as f64 / 1_000.0) * self.input_cost_per_1k;
        let output_cost = (output_tokens as f64 / 1_000.0) * self.output_cost_per_1k;
        input_cost + output_cost
    }
}

/// Built-in pricing table for well-known models.
/// // Updated manually when providers change pricing; can be overridden via config.
pub fn builtin_pricing() -> Vec<ModelPricing> {
    vec![
        ModelPricing::new("claude-opus-4-5", "anthropic", 0.015, 0.075, 200_000),
        ModelPricing::new("claude-sonnet-4-5", "anthropic", 0.003, 0.015, 200_000),
        ModelPricing::new("claude-haiku-3", "anthropic", 0.00025, 0.00125, 200_000),
        ModelPricing::new("gpt-4o", "openai", 0.005, 0.015, 128_000),
        ModelPricing::new("gpt-4o-mini", "openai", 0.00015, 0.0006, 128_000),
        ModelPricing::new("gpt-4-turbo", "openai", 0.01, 0.03, 128_000),
        ModelPricing::new("gpt-3.5-turbo", "openai", 0.0005, 0.0015, 16_385),
        ModelPricing::new("gemini-1.5-pro", "google", 0.00125, 0.005, 1_000_000),
        ModelPricing::new("gemini-1.5-flash", "google", 0.000075, 0.0003, 1_000_000),
        ModelPricing::new("mistral-large", "mistral", 0.004, 0.012, 128_000),
        ModelPricing::new("mistral-small", "mistral", 0.001, 0.003, 128_000),
    ]
}
