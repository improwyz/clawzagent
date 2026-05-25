//! Task complexity analysis — determines how many sub-agents to spawn for a
//! given natural-language goal.
//!
//! # Overview
//! The [`TaskComplexityAnalyzer`] is consulted by the spawner before launching
//! sub-agents. It inspects the goal text and produces a [`ComplexityScore`]
//! containing:
//!
//! - `complexity`         — a coarse tier ([`TaskComplexity`])
//! - `parallelism_hint`   — recommended concurrent sub-agent count
//! - `estimated_turns`    — rough multi-turn budget (parallelism × 3)
//! - `suggested_roles`    — roles the spawner should prefer
//!
//! # Implementation strategy
//! The production implementation uses a **deterministic keyword heuristic**:
//! the analyzer scans the lowercased goal for capability keywords (e.g.
//! `auth`, `database`, `migration`, `test`, `api`, ...). The number of unique
//! matches drives the parallelism hint (clamped to `[1, 8]`).
//!
//! This was a deliberate choice — a keyword heuristic is:
//!   1. **Deterministic** → unit tests don't need an LLM stub.
//!   2. **Cheap** → no token spend, no latency, no failure modes.
//!   3. **Sufficient** → most goals classify correctly in practice.
//!
//! # Upgrade path
//! The [`LlmClient`] trait is defined here so a future implementation can
//! delegate to an LLM (via `crate::providers::ProviderRouter` or any other
//! adapter) for richer analysis. The analyzer holds an `Arc<dyn LlmClient>`
//! reserved for that upgrade — today it is unused on the hot path, but is
//! left in place so wiring it up later does not require changing the public
//! constructor signature.

use async_trait::async_trait;
use clawz_core::error::ClawzError;
use std::sync::Arc;

use crate::runtime::team::TeamRole;

// ---------------------------------------------------------------------------
// LlmClient — reserved for the LLM-based upgrade path
// ---------------------------------------------------------------------------

/// Minimal trait the analyzer would call once an LLM-based path is wired in.
///
/// Today this is **only** used as a placeholder type for the analyzer's
/// `llm_client` field — the production [`TaskComplexityAnalyzer::analyze`]
/// implementation uses a keyword heuristic and does not dispatch to the
/// client. See module-level docs for the rationale.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a single prompt and return the raw text response.
    async fn complete(&self, prompt: &str) -> Result<String, ClawzError>;
}

/// No-op `LlmClient` used when no real client is supplied.
///
/// `complete()` returns an empty string. The keyword-heuristic analyzer
/// never calls this method on its hot path, so the implementation is trivial.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopLlmClient;

#[async_trait]
impl LlmClient for NoopLlmClient {
    async fn complete(&self, _prompt: &str) -> Result<String, ClawzError> {
        Ok(String::new())
    }
}

// ---------------------------------------------------------------------------
// TaskComplexity tier
// ---------------------------------------------------------------------------

/// Coarse complexity tier for a goal. Determined from `parallelism_hint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskComplexity {
    /// 1 sub-agent — trivial goals.
    Low,
    /// 2–3 sub-agents — small multi-step goals.
    Medium,
    /// 4–5 sub-agents — typical feature work.
    High,
    /// 6+ sub-agents — large, cross-cutting goals.
    Massive,
}

// ---------------------------------------------------------------------------
// ComplexityScore
// ---------------------------------------------------------------------------

/// The result of analyzing a goal's complexity.
#[derive(Debug, Clone)]
pub struct ComplexityScore {
    /// Coarse tier derived from `parallelism_hint`.
    pub complexity: TaskComplexity,
    /// Recommended concurrent sub-agent count (always in `[1, 8]`).
    pub parallelism_hint: usize,
    /// Rough multi-turn budget (`parallelism_hint * 3`).
    pub estimated_turns: usize,
    /// Roles the spawner should prefer when materialising the team.
    pub suggested_roles: Vec<TeamRole>,
}

impl ComplexityScore {
    /// Build a [`ComplexityScore`] directly from a raw parallelism hint.
    ///
    /// The hint is clamped to `[1, 8]`. The tier is derived as follows:
    ///
    /// | parallelism | tier      |
    /// |-------------|-----------|
    /// | 1           | Low       |
    /// | 2–3         | Medium    |
    /// | 4–5         | High      |
    /// | 6–8         | Massive   |
    pub fn from_parallelism(hint: usize) -> Self {
        let parallelism = hint.max(1).min(8);
        let complexity = match parallelism {
            1 => TaskComplexity::Low,
            2..=3 => TaskComplexity::Medium,
            4..=5 => TaskComplexity::High,
            _ => TaskComplexity::Massive,
        };
        Self {
            complexity,
            parallelism_hint: parallelism,
            estimated_turns: parallelism * 3,
            suggested_roles: vec![TeamRole::Worker],
        }
    }
}

// ---------------------------------------------------------------------------
// TaskComplexityAnalyzer
// ---------------------------------------------------------------------------

/// Keywords that bump the parallelism hint by 1 each (case-insensitive).
const COMPLEXITY_KEYWORDS: &[&str] = &[
    "auth",
    "database",
    "migration",
    "test",
    "api",
    "cache",
    "queue",
    "websockets",
    "security",
    "deployment",
];

/// Analyzes natural-language goals and produces parallelism recommendations.
///
/// See the module-level docs for the keyword-heuristic strategy and the
/// LLM upgrade path.
pub struct TaskComplexityAnalyzer {
    /// Reserved for the future LLM-based path. Unused on the hot path today.
    #[allow(dead_code)]
    llm_client: Arc<dyn LlmClient>,
    /// Floor used when the goal has zero matching keywords.
    default_parallelism: usize,
}

impl TaskComplexityAnalyzer {
    /// Build a new analyzer with the given LLM client (currently unused — see
    /// module docs) and a default parallelism floor.
    ///
    /// `default_parallelism` is clamped to `[1, 8]`.
    pub fn new(llm_client: Arc<dyn LlmClient>, default_parallelism: usize) -> Self {
        Self {
            llm_client,
            default_parallelism: default_parallelism.max(1).min(8),
        }
    }

    /// Convenience constructor that uses [`NoopLlmClient`] as the client.
    /// Useful for tests and production code paths that do not yet need an
    /// LLM-backed analyzer.
    pub fn with_default_client(default_parallelism: usize) -> Self {
        Self::new(Arc::new(NoopLlmClient), default_parallelism)
    }

    /// Analyze a goal and return a [`ComplexityScore`].
    ///
    /// Current implementation: counts unique keyword hits in
    /// [`COMPLEXITY_KEYWORDS`] and clamps to `[1, 8]`. See module-level docs
    /// for the rationale and the upgrade path to an LLM-driven analyzer.
    pub async fn analyze(&self, goal: &str) -> Result<ComplexityScore, ClawzError> {
        let lower = goal.to_lowercase();
        let hits = COMPLEXITY_KEYWORDS
            .iter()
            .filter(|k| lower.contains(*k))
            .count();

        // Use the larger of (keyword hits, default floor), then clamp to [1, 8].
        let parallelism = hits.max(self.default_parallelism).max(1).min(8);

        let complexity = match parallelism {
            1 => TaskComplexity::Low,
            2..=3 => TaskComplexity::Medium,
            4..=5 => TaskComplexity::High,
            _ => TaskComplexity::Massive,
        };

        Ok(ComplexityScore {
            complexity,
            parallelism_hint: parallelism,
            estimated_turns: parallelism * 3,
            suggested_roles: vec![TeamRole::Worker],
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzer() -> TaskComplexityAnalyzer {
        // floor of 1 so trivial goals classify as Low.
        TaskComplexityAnalyzer::with_default_client(1)
    }

    #[tokio::test]
    async fn from_parallelism_classifies_tiers_correctly() {
        assert_eq!(ComplexityScore::from_parallelism(0).complexity, TaskComplexity::Low);
        assert_eq!(ComplexityScore::from_parallelism(1).complexity, TaskComplexity::Low);
        assert_eq!(ComplexityScore::from_parallelism(2).complexity, TaskComplexity::Medium);
        assert_eq!(ComplexityScore::from_parallelism(3).complexity, TaskComplexity::Medium);
        assert_eq!(ComplexityScore::from_parallelism(4).complexity, TaskComplexity::High);
        assert_eq!(ComplexityScore::from_parallelism(5).complexity, TaskComplexity::High);
        assert_eq!(ComplexityScore::from_parallelism(6).complexity, TaskComplexity::Massive);
        // Above-cap inputs clamp to 8 (Massive).
        let capped = ComplexityScore::from_parallelism(99);
        assert_eq!(capped.parallelism_hint, 8);
        assert_eq!(capped.complexity, TaskComplexity::Massive);
    }

    #[tokio::test]
    async fn analyzer_classifies_simple_as_low() {
        let a = analyzer();
        let score = a.analyze("compute 2 + 2").await.unwrap();
        assert_eq!(score.complexity, TaskComplexity::Low);
        assert_eq!(score.parallelism_hint, 1);
        assert_eq!(score.estimated_turns, 3);
        assert_eq!(score.suggested_roles, vec![TeamRole::Worker]);
    }

    #[tokio::test]
    async fn analyzer_classifies_two_keywords_as_medium() {
        let a = analyzer();
        let score = a.analyze("Add auth to the API endpoint").await.unwrap();
        // matches: "auth", "api" -> 2
        assert_eq!(score.parallelism_hint, 2);
        assert_eq!(score.complexity, TaskComplexity::Medium);
    }

    #[tokio::test]
    async fn analyzer_classifies_multi_step_as_high() {
        let a = analyzer();
        // matches: "auth", "database", "migration", "api" -> 4
        let score = a
            .analyze("Add auth, set up the database migration, and wire the API")
            .await
            .unwrap();
        assert_eq!(score.parallelism_hint, 4);
        assert_eq!(score.complexity, TaskComplexity::High);
    }

    #[tokio::test]
    async fn analyzer_classifies_massive_goal_and_caps_at_eight() {
        let a = analyzer();
        // matches all 10 keywords; should cap at 8.
        let goal = "Build auth, database, migration, test harness, api, cache, queue, \
                    websockets, security, and deployment pipeline";
        let score = a.analyze(goal).await.unwrap();
        assert_eq!(score.parallelism_hint, 8);
        assert_eq!(score.complexity, TaskComplexity::Massive);
        assert_eq!(score.estimated_turns, 24);
    }

    #[tokio::test]
    async fn analyzer_keywords_are_case_insensitive() {
        let a = analyzer();
        let score = a.analyze("Refactor the AUTH layer").await.unwrap();
        assert_eq!(score.parallelism_hint, 1);
        // Default floor is 1; one match -> still Low because hits.max(floor)==1.
        assert_eq!(score.complexity, TaskComplexity::Low);

        let score2 = a.analyze("Refactor the AUTH layer and the API").await.unwrap();
        assert_eq!(score2.parallelism_hint, 2);
        assert_eq!(score2.complexity, TaskComplexity::Medium);
    }

    #[tokio::test]
    async fn analyzer_respects_default_floor() {
        // floor of 3 -> simple goals get Medium tier.
        let a = TaskComplexityAnalyzer::with_default_client(3);
        let score = a.analyze("write a haiku").await.unwrap();
        assert_eq!(score.parallelism_hint, 3);
        assert_eq!(score.complexity, TaskComplexity::Medium);
    }

    #[tokio::test]
    async fn noop_llm_client_returns_empty_string() {
        let c = NoopLlmClient;
        assert_eq!(c.complete("anything").await.unwrap(), "");
    }
}
