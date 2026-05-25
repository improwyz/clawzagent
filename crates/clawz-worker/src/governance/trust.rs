//! Trust scoring system with decay, history, and tier mapping.
//!
//! This module provides a thread-safe trust scoring mechanism for agents in the
//! clawz ecosystem. Each agent receives a numerical trust score that evolves
//! over time based on observed behavior, with scores naturally decaying toward
//! a configurable baseline to encourage consistent good behavior.
//!
//! # Trust Tiers
//!
//! Scores are divided into five discrete tiers that determine what capabilities
//! an agent is granted:
//!
//! | Tier       | Score Range | Description                              |
//! |------------|-------------|------------------------------------------|
//! | `Untrusted`| 0–199       | New or penalized agents; heavily restricted |
//! | `Limited`  | 200–399     | Agents with some negative history; limited access |
//! | `Standard` | 400–599     | Default tier for new agents; normal privileges |
//! | `Trusted`  | 600–799     | Proven reliable agents; expanded capabilities |
//! | `Full`     | 800–1000    | Highly trusted agents; maximum privileges  |
//!
//! Tiers are derived from the raw `u16` score via [`tier_from_score`]. Agents
//! move between tiers automatically as their scores change.
//!
//! # Score Encoding
//!
//! Internally, scores are stored as [`u16`] values in the range **0–1000**.
//! This compact representation avoids floating-point drift in persisted state
//! and keeps memory usage minimal when tracking thousands of agents.
//!
//! The public API exposes [`TrustScore::as_f64`], which normalizes the raw
//! score to a [`f64`] in **[0.0, 1.0]**. This normalized form is required for
//! compatibility with the `clawz-core` `GovernanceEngine` trait, which expects
//! floating-point trust values for integration with downstream policy engines.
//!
//! # Decay Mechanism
//!
//! Trust is not static. The [`TrustScorer::decay_all`] method applies a periodic
//! decay step that nudges every agent's score toward a configurable `baseline`
//! (default: 500, the center of the `Standard` tier).
//!
//! * Scores **above** baseline decrease by `decay_per_tick`.
//! * Scores **below** baseline increase by `decay_per_tick`.
//! * Agents already at baseline are left unchanged.
//!
//! This creates a **regression-to-the-mean** effect: consistently good behavior
//! must be maintained to keep a high score, while occasional mistakes gradually
//! heal for agents with a history of trustworthiness. The decay per tick and
//! baseline are configurable via [`TrustScorer::with_decay`] and
//! [`TrustScorer::with_baseline`].
//!
//! # Example
//!
//! ```rust,no_run
//! # use clawz_worker::governance::trust::TrustScorer;
//! # #[tokio::main]
//! # async fn main() {
//! let scorer = TrustScorer::new()
//!     .with_decay(5)
//!     .with_baseline(500);
//!
//! // Reward good behavior
//! scorer.update_score("agent-42", 100, "completed task successfully").await;
//!
//! // Penalize bad behavior
//! scorer.update_score("agent-42", -50, "timeout on critical path").await;
//!
//! let score = scorer.get_score("agent-42").await;
//! println!("Score: {} (tier: {:?}, normalized: {})", score.score, score.tier, score.as_f64());
//! # }
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use clawz_core::types::governance::TrustTier;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Default starting score for new agents (500 = center of Standard tier).
const DEFAULT_SCORE: u16 = 500;
/// Score floor — never decay or penalize below this value.
const SCORE_FLOOR: u16 = 0;
/// Score ceiling — rewards cannot push a score above this value.
const SCORE_CEILING: u16 = 1000;
/// Maximum score for the `Untrusted` tier (0–199).
const TIER_UNTRUSTED_MAX: u16 = 199;
/// Maximum score for the `Limited` tier (200–399).
const TIER_LIMITED_MAX: u16 = 399;
/// Maximum score for the `Standard` tier (400–599).
const TIER_STANDARD_MAX: u16 = 599;
/// Maximum score for the `Trusted` tier (600–799).
const TIER_TRUSTED_MAX: u16 = 799;

// ── TrustEvent ────────────────────────────────────────────────────────────────

/// A single mutation recorded in an agent's trust history.
///
/// Every call to [`TrustScore::apply_delta`] produces a `TrustEvent` that
/// captures the signed change, human-readable reason, resulting score, and
/// timestamp. This audit trail is invaluable for debugging unexpected tier
/// transitions and for offline analysis of agent behavior patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustEvent {
    /// Signed change applied to the score (positive for rewards, negative for penalties).
    pub delta: i32,
    /// Human-readable explanation of why the delta was applied.
    pub reason: String,
    /// Raw `u16` score immediately after the delta was applied.
    pub score_after: u16,
    /// UTC timestamp of when the event occurred.
    pub timestamp: DateTime<Utc>,
}

// ── TrustScore ────────────────────────────────────────────────────────────────

/// Per-agent trust state including raw score, computed tier, and full history.
///
/// `TrustScore` is the immutable snapshot returned by [`TrustScorer::get_score`]
/// and [`TrustScorer::all_scores`]. It is cheap to clone because the history
/// vector is typically small (dozens of events per agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustScore {
    /// Unique identifier of the agent this score belongs to.
    pub agent_id: String,
    /// Raw trust value in the range **0–1000**.
    ///
    /// Use [`TrustScore::as_f64`] when you need a normalized `f64` for
    /// interoperability with `clawz-core` policy engines.
    pub score: u16,
    /// Current trust tier derived from `score` via [`tier_from_score`].
    pub tier: TrustTier,
    /// UTC timestamp of the most recent score mutation.
    pub last_updated: DateTime<Utc>,
    /// Chronological list of all score changes (oldest first).
    pub history: Vec<TrustEvent>,
}

impl TrustScore {
    /// Create a new `TrustScore` for `agent_id` with the default starting score.
    ///
    /// The default score is [`DEFAULT_SCORE`] (500), which places the agent in
    /// the [`TrustTier::Standard`] tier.
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            score: DEFAULT_SCORE,
            tier: tier_from_score(DEFAULT_SCORE),
            last_updated: Utc::now(),
            history: Vec::new(),
        }
    }

    /// Apply a signed `delta` to the score and append an event to the history.
    ///
    /// # Arguments
    ///
    /// * `delta` — Signed change to apply. Positive values reward the agent;
    ///   negative values penalize it.
    /// * `reason` — Human-readable string explaining the change (e.g.
    ///   `"completed task"` or `"timeout"`).
    ///
    /// # Clamping
    ///
    /// The resulting score is clamped to the inclusive range
    /// [`SCORE_FLOOR`]..=[`SCORE_CEILING`] so that rewards never overflow and
    /// penalties never underflow the `u16` storage.
    pub fn apply_delta(&mut self, delta: i32, reason: &str) {
        let new_score = (self.score as i32 + delta)
            .max(SCORE_FLOOR as i32)
            .min(SCORE_CEILING as i32) as u16;
        self.score = new_score;
        self.tier = tier_from_score(new_score);
        self.last_updated = Utc::now();
        self.history.push(TrustEvent {
            delta,
            reason: reason.to_string(),
            score_after: new_score,
            timestamp: Utc::now(),
        });
    }

    /// Return the score normalized to the range **[0.0, 1.0]**.
    ///
    /// This `f64` representation is the canonical form expected by the
    /// `clawz-core` `GovernanceEngine` trait and downstream policy engines.
    pub fn as_f64(&self) -> f64 {
        self.score as f64 / 1000.0
    }
}

/// Map a raw `u16` score to the corresponding [`TrustTier`].
///
/// The boundaries are fixed and match the definitions in `clawz-core`:
///
/// | Range     | Tier       |
/// |-----------|------------|
/// | 0–199     | Untrusted  |
/// | 200–399   | Limited    |
/// | 400–599   | Standard   |
/// | 600–799   | Trusted    |
/// | 800–1000  | Full       |
fn tier_from_score(score: u16) -> TrustTier {
    match score {
        0..=199 => TrustTier::Untrusted,
        200..=399 => TrustTier::Limited,
        400..=599 => TrustTier::Standard,
        600..=799 => TrustTier::Trusted,
        _ => TrustTier::Full,
    }
}

// ── TrustScorer ───────────────────────────────────────────────────────────────

/// Thread-safe trust scorer with per-agent scores, periodic decay, and audit history.
///
/// `TrustScorer` is the primary public interface for the trust subsystem. It
/// maintains a concurrent map of [`TrustScore`] instances keyed by agent ID,
/// protected by a [`tokio::sync::RwLock`] so that reads (queries) and writes
/// (updates / decay) can scale across async worker tasks.
///
/// # Configuration
///
/// Use the builder-style methods to customize behavior before handing the
/// scorer to the rest of the application:
///
/// ```rust,no_run
/// # use clawz_worker::governance::trust::TrustScorer;
/// let scorer = TrustScorer::new()
///     .with_decay(2)          // move 2 points per tick toward baseline
///     .with_baseline(500);     // regress to the Standard tier center
/// ```
///
/// # Decay
///
/// Call [`TrustScorer::decay_all`] periodically (e.g. once per minute) to apply
/// the regression-to-baseline mechanism. See the [module-level documentation](self)
/// for a detailed explanation of why decay matters.
pub struct TrustScorer {
    /// In-memory map of agent ID → [`TrustScore`], shared across all async tasks.
    scores: Arc<RwLock<HashMap<String, TrustScore>>>,
    /// Number of raw points to shift each agent's score per [`decay_all`](Self::decay_all) call.
    ///
    /// A value of `1` is conservative; larger values make the system forget
    /// quickly. The default is `1`.
    decay_per_tick: i32,
    /// The raw `u16` score toward which decay converges.
    ///
    /// Defaults to [`DEFAULT_SCORE`] (500). Choosing a lower baseline makes
    /// the system more pessimistic; a higher baseline is more forgiving.
    baseline: u16,
}

impl TrustScorer {
    /// Create a `TrustScorer` with default settings.
    ///
    /// Defaults:
    /// * `decay_per_tick` = `1`
    /// * `baseline` = [`DEFAULT_SCORE`] (`500`)
    pub fn new() -> Self {
        Self {
            scores: Arc::new(RwLock::new(HashMap::new())),
            decay_per_tick: 1,
            baseline: DEFAULT_SCORE,
        }
    }

    /// Set the number of points to decay per tick.
    ///
    /// Consumes `self` and returns it, enabling builder-style chaining.
    ///
    /// # Panics
    ///
    /// The current implementation does not panic, but callers should ensure
    /// `per_tick` is positive. A value of `0` effectively disables decay.
    pub fn with_decay(mut self, per_tick: i32) -> Self {
        self.decay_per_tick = per_tick;
        self
    }

    /// Set the baseline score toward which decay converges.
    ///
    /// Consumes `self` and returns it, enabling builder-style chaining.
    ///
    /// # Considerations
    ///
    /// * A baseline of `500` (default) keeps new agents in the `Standard` tier.
    /// * A baseline below `400` pushes agents toward `Limited` unless actively rewarded.
    /// * A baseline above `600` makes `Trusted` the "resting" state.
    pub fn with_baseline(mut self, baseline: u16) -> Self {
        self.baseline = baseline;
        self
    }

    /// Retrieve the current [`TrustScore`] for `agent_id`.
    ///
    /// If the agent has never been seen, a default score is returned (but not
    /// persisted in the internal map). Use [`update_score`](Self::update_score)
    /// to materialize an entry.
    pub async fn get_score(&self, agent_id: &str) -> TrustScore {
        let scores = self.scores.read().await;
        scores
            .get(agent_id)
            .cloned()
            .unwrap_or_else(|| TrustScore::new(agent_id))
    }

    /// Apply a signed `delta` to an agent's score and record the reason.
    ///
    /// If the agent does not yet exist in the map, a default [`TrustScore`] is
    /// created first, then the delta is applied.
    ///
    /// # Arguments
    ///
    /// * `agent_id` — Identifier of the agent to update.
    /// * `delta` — Signed change (positive = reward, negative = penalty).
    /// * `reason` — Human-readable explanation stored in the history.
    ///
    /// # Logging
    ///
    /// A `debug!` log line is emitted after every successful update so that
    /// operators can trace score evolution in real time.
    pub async fn update_score(&self, agent_id: &str, delta: i32, reason: &str) {
        let mut scores = self.scores.write().await;
        let score = scores
            .entry(agent_id.to_string())
            .or_insert_with(|| TrustScore::new(agent_id));
        score.apply_delta(delta, reason);
        log::debug!(
            "[trust] agent='{}' delta={:+} score={} tier={}",
            agent_id,
            delta,
            score.score,
            score.tier
        );
    }

    /// Apply decay to **all** tracked agents.
    ///
    /// This is the regression-to-baseline step. For each agent:
    ///
    /// 1. If the current score is **above** `baseline`, subtract `decay_per_tick`
    ///    (or the distance to baseline, whichever is smaller).
    /// 2. If the current score is **below** `baseline`, add `decay_per_tick`
    ///    (or the distance to baseline, whichever is smaller).
    /// 3. If the score is already exactly at `baseline`, do nothing.
    ///
    /// The underlying [`TrustScore::apply_delta`] call records a history event
    /// with reason `"decay"` or `"decay-recovery"`, so decay steps are fully
    /// auditable.
    ///
    /// # When to call
    ///
    /// Typically invoked from a background timer (e.g. every 60 seconds). The
    /// frequency should be tuned to the `decay_per_tick` value so that the
    /// effective half-life of a score excursion feels natural for the domain.
    pub async fn decay_all(&self) {
        let mut scores = self.scores.write().await;
        for score in scores.values_mut() {
            let current = score.score as i32;
            let target = self.baseline as i32;
            if current > target {
                let decay = self.decay_per_tick.min(current - target);
                score.apply_delta(-decay, "decay");
            } else if current < target {
                let grow = self.decay_per_tick.min(target - current);
                score.apply_delta(grow, "decay-recovery");
            }
        }
    }

    /// Return a snapshot of every agent's current [`TrustScore`].
    ///
    /// The returned vector is a point-in-time copy; subsequent mutations will
    /// not affect it. Useful for metrics exporters, dashboards, and periodic
    /// persistence to durable storage.
    pub async fn all_scores(&self) -> Vec<TrustScore> {
        self.scores.read().await.values().cloned().collect()
    }

    /// Return the full [`TrustEvent`] history for `agent_id`.
    ///
    /// Returns an empty vector if the agent is unknown.
    pub async fn history(&self, agent_id: &str) -> Vec<TrustEvent> {
        self.scores
            .read()
            .await
            .get(agent_id)
            .map(|s| s.history.clone())
            .unwrap_or_default()
    }
}

impl Default for TrustScorer {
    /// Equivalent to [`TrustScorer::new`].
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_new_agent_has_default_score() {
        let scorer = TrustScorer::new();
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, DEFAULT_SCORE);
        assert_eq!(score.tier, TrustTier::Standard);
    }

    #[tokio::test]
    async fn test_positive_delta_increases_score() {
        let scorer = TrustScorer::new();
        scorer.update_score("agent-1", 100, "good behavior").await;
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, 600);
        assert_eq!(score.tier, TrustTier::Trusted);
    }

    #[tokio::test]
    async fn test_negative_delta_decreases_score() {
        let scorer = TrustScorer::new();
        scorer.update_score("agent-1", -300, "bad action").await;
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, 200);
        assert_eq!(score.tier, TrustTier::Limited);
    }

    #[tokio::test]
    async fn test_score_clamped_at_ceiling() {
        let scorer = TrustScorer::new();
        scorer.update_score("agent-1", 1000, "overflow").await;
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, 1000);
        assert_eq!(score.tier, TrustTier::Full);
    }

    #[tokio::test]
    async fn test_score_clamped_at_floor() {
        let scorer = TrustScorer::new();
        scorer.update_score("agent-1", -2000, "underflow").await;
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, 0);
        assert_eq!(score.tier, TrustTier::Untrusted);
    }

    #[tokio::test]
    async fn test_history_recorded() {
        let scorer = TrustScorer::new();
        scorer.update_score("agent-1", 50, "reason A").await;
        scorer.update_score("agent-1", -20, "reason B").await;
        let history = scorer.history("agent-1").await;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].reason, "reason A");
        assert_eq!(history[1].reason, "reason B");
    }

    #[tokio::test]
    async fn test_decay_toward_baseline() {
        let scorer = TrustScorer::new().with_decay(10).with_baseline(500);
        scorer.update_score("agent-1", 200, "bonus").await; // score = 700
        scorer.decay_all().await;
        let score = scorer.get_score("agent-1").await;
        assert_eq!(score.score, 690); // 700 - 10
    }

    #[tokio::test]
    async fn test_as_f64() {
        let scorer = TrustScorer::new();
        let score = scorer.get_score("agent-1").await;
        assert!((score.as_f64() - 0.5).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn test_tier_boundaries() {
        assert_eq!(tier_from_score(0), TrustTier::Untrusted);
        assert_eq!(tier_from_score(199), TrustTier::Untrusted);
        assert_eq!(tier_from_score(200), TrustTier::Limited);
        assert_eq!(tier_from_score(399), TrustTier::Limited);
        assert_eq!(tier_from_score(400), TrustTier::Standard);
        assert_eq!(tier_from_score(599), TrustTier::Standard);
        assert_eq!(tier_from_score(600), TrustTier::Trusted);
        assert_eq!(tier_from_score(799), TrustTier::Trusted);
        assert_eq!(tier_from_score(800), TrustTier::Full);
        assert_eq!(tier_from_score(1000), TrustTier::Full);
    }
}
