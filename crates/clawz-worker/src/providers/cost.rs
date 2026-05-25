//! Cost tracking and budget enforcement for LLM providers.
//!
//! [`CostTracker`] records every provider call, calculates USD cost from
//! token usage, and enforces daily / monthly budget limits. It also supports
//! prefix-based model pricing lookup so that dated snapshot models
//! (e.g. `gpt-4o-2024-11-20`) match the base model price.
//!
//! // Dependency: `clawz_core::types::cost::{BudgetConfig, CostRecord, ModelPricing}`
//! // for the canonical pricing schema.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Datelike, Utc};
use clawz_core::{
    error::ClawzError,
    types::{
        cost::{BudgetConfig, CostRecord, ModelPricing},
        message::ChatResponse,
    },
};
use tokio::sync::RwLock;
use uuid::Uuid;

// ── CostEntry (internal time-bucketed record) ─────────────────────────────────

/// A single recorded cost event.
///
/// Kept in-memory; persisted externally via [`build_record`] if desired.
#[derive(Debug, Clone)]
struct CostEntry {
    /// Provider name (e.g. "openai").
    provider: String,
    /// Model identifier.
    model: String,
    /// Optional agent that initiated the call.
    agent_id: Option<Uuid>,
    /// Prompt tokens consumed.
    input_tokens: u64,
    /// Completion tokens consumed.
    output_tokens: u64,
    /// Computed cost in USD.
    cost_usd: f64,
    /// When the call was made.
    timestamp: DateTime<Utc>,
}

// ── CostTracker ───────────────────────────────────────────────────────────────

/// Thread-safe cost tracker with budget enforcement and aggregation.
///
/// All entries are held in a [`Vec`] protected by a [`RwLock`]; in
/// high-throughput scenarios this should be replaced with a ring buffer
/// or external database sink.
pub struct CostTracker {
    /// Append-only list of every recorded cost event.
    entries: Arc<RwLock<Vec<CostEntry>>>,
    /// Static pricing table (model prefix → [`ModelPricing`]).
    pricing: Arc<HashMap<String, ModelPricing>>,
    /// Optional daily / monthly budget constraints.
    budget: Option<BudgetConfig>,
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CostTracker {
    pub fn new() -> Self {
        Self::with_budget(None)
    }

    pub fn with_budget(budget: Option<BudgetConfig>) -> Self {
        let pricing = build_pricing_table();
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
            pricing: Arc::new(pricing),
            budget,
        }
    }

    // ── Cost calculation ──────────────────────────────────────────────────────

    /// Calculate the dollar cost for a model + token counts.
    ///
    /// Falls back to generic pricing ($0.01/1k input, $0.03/1k output)
    /// when the model is not found in the pricing table.
    pub fn calculate_cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        if let Some(pricing) = self.find_pricing(model) {
            pricing.compute_cost(input_tokens, output_tokens)
        } else {
            // Fallback: $0.01/1k input, $0.03/1k output
            let input_cost = (input_tokens as f64 / 1_000.0) * 0.01;
            let output_cost = (output_tokens as f64 / 1_000.0) * 0.03;
            input_cost + output_cost
        }
    }

    fn find_pricing(&self, model: &str) -> Option<&ModelPricing> {
        // Exact match first
        if let Some(p) = self.pricing.get(model) {
            return Some(p);
        }
        // Prefix match (e.g. "gpt-4o-2024-11-20" -> "gpt-4o")
        self.pricing
            .iter()
            .find(|(k, _)| model.starts_with(k.as_str()))
            .map(|(_, v)| v)
    }

    // ── Recording ─────────────────────────────────────────────────────────────

    /// Record a cost entry from a [`ChatResponse`].
    pub async fn record(
        &self,
        provider: &str,
        agent_id: Option<Uuid>,
        response: &ChatResponse,
    ) {
        let input = response.usage.prompt_tokens as u64;
        let output = response.usage.completion_tokens as u64;
        let cost = self.calculate_cost(&response.model, input, output);

        self.record_entry(CostEntry {
            provider: provider.to_string(),
            model: response.model.clone(),
            agent_id,
            input_tokens: input,
            output_tokens: output,
            cost_usd: cost,
            timestamp: Utc::now(),
        })
        .await;
    }

    /// Record a raw cost entry (useful for non-token-based charges).
    pub async fn record_cost(&self, provider: &str, cost_usd: f64) {
        self.record_entry(CostEntry {
            provider: provider.to_string(),
            model: String::new(),
            agent_id: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd,
            timestamp: Utc::now(),
        })
        .await;
    }

    async fn record_entry(&self, entry: CostEntry) {
        let mut entries = self.entries.write().await;
        entries.push(entry);
    }

    // ── Budget enforcement ────────────────────────────────────────────────────

    /// Returns `Ok(())` if under budget, `Err(BudgetExceeded)` otherwise.
    pub async fn check_budget(&self) -> Result<(), ClawzError> {
        let Some(budget) = &self.budget else {
            return Ok(());
        };

        let today_spent = self.daily_total().await;
        let month_spent = self.monthly_total().await;

        if budget.is_over_budget(today_spent, budget.max_daily_usd) {
            return Err(ClawzError::Provider(format!(
                "daily budget exceeded: ${:.4} >= ${:.2}",
                today_spent, budget.max_daily_usd
            )));
        }

        if budget.is_over_budget(month_spent, budget.max_monthly_usd) {
            return Err(ClawzError::Provider(format!(
                "monthly budget exceeded: ${:.4} >= ${:.2}",
                month_spent, budget.max_monthly_usd
            )));
        }

        Ok(())
    }

    /// Returns `true` if near the alert threshold.
    pub async fn is_near_limit(&self) -> bool {
        let Some(budget) = &self.budget else {
            return false;
        };
        let today_spent = self.daily_total().await;
        let month_spent = self.monthly_total().await;
        budget.is_alert_threshold(today_spent, budget.max_daily_usd)
            || budget.is_alert_threshold(month_spent, budget.max_monthly_usd)
    }

    // ── Aggregation ───────────────────────────────────────────────────────────

    /// Total cost for a specific provider across all time.
    pub async fn total_cost(&self, provider: &str) -> f64 {
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| e.provider == provider)
            .map(|e| e.cost_usd)
            .sum()
    }

    /// Total cost across all providers.
    pub async fn grand_total(&self) -> f64 {
        let entries = self.entries.read().await;
        entries.iter().map(|e| e.cost_usd).sum()
    }

    /// Cost per provider map.
    pub async fn all_costs(&self) -> HashMap<String, f64> {
        let entries = self.entries.read().await;
        let mut map: HashMap<String, f64> = HashMap::new();
        for e in entries.iter() {
            *map.entry(e.provider.clone()).or_insert(0.0) += e.cost_usd;
        }
        map
    }

    /// Today's total spend across all providers.
    pub async fn daily_total(&self) -> f64 {
        let now = Utc::now();
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| {
                e.timestamp.year() == now.year()
                    && e.timestamp.month() == now.month()
                    && e.timestamp.day() == now.day()
            })
            .map(|e| e.cost_usd)
            .sum()
    }

    /// This month's total spend.
    pub async fn monthly_total(&self) -> f64 {
        let now = Utc::now();
        let entries = self.entries.read().await;
        entries
            .iter()
            .filter(|e| {
                e.timestamp.year() == now.year() && e.timestamp.month() == now.month()
            })
            .map(|e| e.cost_usd)
            .sum()
    }

    /// Build a [`CostRecord`] for external use (e.g., database persistence).
    pub fn build_record(
        &self,
        agent_id: Uuid,
        provider: &str,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
    ) -> CostRecord {
        let cost = self.calculate_cost(model, input_tokens, output_tokens);
        CostRecord::new(agent_id, provider, model, input_tokens, output_tokens, cost)
    }
}

// ── Pricing table ─────────────────────────────────────────────────────────────

/// Static pricing data for known models.
///
/// Prices are in **USD per 1K tokens**.
fn build_pricing_table() -> HashMap<String, ModelPricing> {
    let entries = vec![
        // OpenAI
        ModelPricing::new("gpt-4o", "openai", 0.005, 0.015, 128_000),
        ModelPricing::new("gpt-4o-mini", "openai", 0.00015, 0.0006, 128_000),
        ModelPricing::new("gpt-4-turbo", "openai", 0.01, 0.03, 128_000),
        ModelPricing::new("gpt-4", "openai", 0.03, 0.06, 8_192),
        ModelPricing::new("gpt-3.5-turbo", "openai", 0.0005, 0.0015, 16_385),
        ModelPricing::new("o1", "openai", 0.015, 0.060, 200_000),
        ModelPricing::new("o1-mini", "openai", 0.003, 0.012, 128_000),
        ModelPricing::new("o3", "openai", 0.010, 0.040, 200_000),
        ModelPricing::new("o3-mini", "openai", 0.0011, 0.0044, 200_000),
        // Anthropic
        ModelPricing::new("claude-opus-4-5", "anthropic", 0.015, 0.075, 200_000),
        ModelPricing::new("claude-sonnet-4-5", "anthropic", 0.003, 0.015, 200_000),
        ModelPricing::new("claude-haiku-3", "anthropic", 0.00025, 0.00125, 200_000),
        ModelPricing::new("claude-3-5-sonnet-20241022", "anthropic", 0.003, 0.015, 200_000),
        ModelPricing::new("claude-3-5-haiku-20241022", "anthropic", 0.00025, 0.00125, 200_000),
        ModelPricing::new("claude-3-opus-20240229", "anthropic", 0.015, 0.075, 200_000),
        ModelPricing::new("claude-3-sonnet-20240229", "anthropic", 0.003, 0.015, 200_000),
        ModelPricing::new("claude-3-haiku-20240307", "anthropic", 0.00025, 0.00125, 200_000),
        // Google
        ModelPricing::new("gemini-1.5-pro", "google", 0.00125, 0.005, 1_000_000),
        ModelPricing::new("gemini-1.5-flash", "google", 0.000075, 0.0003, 1_000_000),
        ModelPricing::new("gemini-2.0-flash", "google", 0.000075, 0.0003, 1_000_000),
        ModelPricing::new("gemini-pro", "google", 0.0005, 0.0015, 32_768),
        // Mistral
        ModelPricing::new("mistral-large", "mistral", 0.004, 0.012, 128_000),
        ModelPricing::new("mistral-small", "mistral", 0.001, 0.003, 128_000),
        ModelPricing::new("mistral-7b", "mistral", 0.00025, 0.00025, 32_768),
        // DeepSeek
        ModelPricing::new("deepseek-chat", "deepseek", 0.00014, 0.00028, 64_000),
        ModelPricing::new("deepseek-coder", "deepseek", 0.00014, 0.00028, 16_000),
        ModelPricing::new("deepseek-reasoner", "deepseek", 0.00055, 0.00219, 64_000),
        // Ollama (free/local)
        ModelPricing::new("llama3", "ollama", 0.0, 0.0, 8_192),
        ModelPricing::new("mistral", "ollama", 0.0, 0.0, 32_768),
        ModelPricing::new("codellama", "ollama", 0.0, 0.0, 16_384),
    ];

    entries.into_iter().map(|p| (p.model.clone(), p)).collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_cost_exact_match() {
        let tracker = CostTracker::new();
        let cost = tracker.calculate_cost("gpt-4o", 1_000, 500);
        // 1000 input @ $0.005/1k + 500 output @ $0.015/1k
        let expected = 0.005 + 0.0075;
        assert!((cost - expected).abs() < 0.0001);
    }

    #[test]
    fn test_calculate_cost_prefix_match() {
        let tracker = CostTracker::new();
        // "gpt-4o-2024-11-20" should prefix-match "gpt-4o"
        let cost = tracker.calculate_cost("gpt-4o-2024-11-20", 1_000, 1_000);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_calculate_cost_fallback() {
        let tracker = CostTracker::new();
        let cost = tracker.calculate_cost("unknown-model-xyz", 1_000, 1_000);
        // Fallback pricing applies
        assert!(cost > 0.0);
    }

    #[test]
    fn test_ollama_zero_cost() {
        let tracker = CostTracker::new();
        let cost = tracker.calculate_cost("llama3", 100_000, 50_000);
        assert_eq!(cost, 0.0);
    }

    #[tokio::test]
    async fn test_record_cost() {
        let tracker = CostTracker::new();
        tracker.record_cost("openai", 0.05).await;
        tracker.record_cost("openai", 0.03).await;
        let total = tracker.total_cost("openai").await;
        assert!((total - 0.08).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_all_costs_map() {
        let tracker = CostTracker::new();
        tracker.record_cost("openai", 0.10).await;
        tracker.record_cost("anthropic", 0.05).await;
        tracker.record_cost("openai", 0.02).await;
        let map = tracker.all_costs().await;
        assert!((map["openai"] - 0.12).abs() < 0.0001);
        assert!((map["anthropic"] - 0.05).abs() < 0.0001);
    }

    #[tokio::test]
    async fn test_budget_enforcement() {
        let budget = BudgetConfig {
            max_daily_usd: 0.01,
            max_monthly_usd: 1.0,
            alert_threshold_pct: 80.0,
            hard_limit: true,
        };
        let tracker = CostTracker::with_budget(Some(budget));
        tracker.record_cost("openai", 0.02).await; // Over daily limit
        let result = tracker.check_budget().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_no_budget_always_ok() {
        let tracker = CostTracker::new(); // no budget
        tracker.record_cost("openai", 999.99).await;
        assert!(tracker.check_budget().await.is_ok());
    }

    #[tokio::test]
    async fn test_grand_total() {
        let tracker = CostTracker::new();
        tracker.record_cost("openai", 1.00).await;
        tracker.record_cost("anthropic", 2.00).await;
        let total = tracker.grand_total().await;
        assert!((total - 3.00).abs() < 0.0001);
    }

    #[test]
    fn test_build_record() {
        let tracker = CostTracker::new();
        let agent_id = Uuid::new_v4();
        let record =
            tracker.build_record(agent_id, "openai", "gpt-4o", 1_000, 500);
        assert_eq!(record.provider, "openai");
        assert_eq!(record.model, "gpt-4o");
        assert!(record.cost_usd > 0.0);
    }
}
