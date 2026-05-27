//! Multi-agent deliberation council with configurable voting and tie-breaking.
//!
//! The `Council` module implements a small deliberative body where multiple
//! agents (members) evaluate a [`Proposal`] and cast [`Vote`]s under a
//! [`DecisionRule`].  It is designed for lightweight governance inside a worker
//! process: proposals are submitted, members vote, and a [`CouncilDecision`]
//! is produced.
//!
//! # Member roles
//!
//! Every [`CouncilMember`] has one of three [`CouncilRole`]s:
//!
//! * **Proponent** – originates or champions a proposal; votes like any other
//!   non-arbiter member.
//! * **Reviewer** – evaluates the proposal on its merits; votes like any other
//!   non-arbiter member.
//! * **Arbiter** – does **not** count toward the primary vote tally; their only
//!   job is to break ties when the [`DecisionRule::Majority`] rule is in use
//!   and the non-arbiter members are dead-locked.
//!
//! # Voting mechanics
//!
//! Votes are cast per-member as [`Vote::Approve`], [`Vote::Reject`], or
//! [`Vote::Abstain`].  A member who never votes is treated as abstaining when
//! [`Council::deliberate`] is called.
//!
//! The [`DecisionRule`] determines how approvals are converted into a final
//! decision:
//!
//! | Rule | Behaviour |
//! |------|-----------|
//! | [`Unanimous`](DecisionRule::Unanimous) | Every non-arbiter member must vote [`Approve`](Vote::Approve). One rejection or abstention kills the proposal. |
//! | [`Majority`](DecisionRule::Majority) | More [`Approve`](Vote::Approve) votes than [`Reject`](Vote::Reject) votes among non-arbiters. If the count is equal, the [`Arbiter`](CouncilRole::Arbiter) breaks the tie (see *Tie-breaking* below). All-abstain is an error. |
//! | [`Supermajority`](DecisionRule::Supermajority) | The approval ratio `approvals / (approvals + rejections)` must be **strictly greater** than the configured threshold (default `0.667`). Abstentions are excluded from the denominator. All-abstain is an error. |
//!
//! # Tie-breaking
//!
//! Tie-breaking only applies to [`DecisionRule::Majority`]:
//!
//! 1. Count [`Approve`](Vote::Approve) and [`Reject`](Vote::Reject) among all
//!    non-arbiter members.
//! 2. If the two counts are equal, locate the single [`Arbiter`](CouncilRole::Arbiter).
//! 3. The arbiter’s vote (if any) decides the outcome:
//!    [`Approve`](Vote::Approve) → proposal passes, [`Reject`](Vote::Reject) or
//!    absent → proposal fails.
//! 4. The resulting [`CouncilDecision`] sets
//!    [`tie_broken_by_arbiter`](CouncilDecision::tie_broken_by_arbiter) to
//!    `true` so callers can observe that the primary vote was dead-locked.
//!
//! # Example
//!
//! ```rust,no_run
//! use clawz_worker::governance::council::{Council, CouncilRole, DecisionRule, Vote};
//!
//! # async fn example() {
//! let council = Council::new()
//!     .with_rule(DecisionRule::Majority)
//!     .with_vote_timeout(std::time::Duration::from_secs(30));
//!
//! council.add_member("proponent-1", CouncilRole::Proponent).await;
//! council.add_member("reviewer-1",  CouncilRole::Reviewer).await;
//! council.add_member("arbiter-1",   CouncilRole::Arbiter).await;
//!
//! council.cast_vote("proponent-1", Vote::Approve, Some("LGTM".to_string())).await.unwrap();
//! council.cast_vote("reviewer-1",  Vote::Reject,  Some("Needs work".to_string())).await.unwrap();
//! // tie → arbiter decides
//! council.cast_vote("arbiter-1",   Vote::Approve, Some("I break the tie".to_string())).await.unwrap();
//! # }
//! ```

use std::time::Duration;

use chrono::{DateTime, Utc};
use clawz_core::error::{ClawzError, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Roles and votes ───────────────────────────────────────────────────────────

/// Role assigned to a [`CouncilMember`] that determines how (or whether) their
/// vote counts toward the primary tally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CouncilRole {
    /// Champions or originates a proposal; votes as a regular non-arbiter member.
    Proponent,
    /// Evaluates the proposal on its merits; votes as a regular non-arbiter member.
    Reviewer,
    /// Does **not** participate in the primary vote tally.  Only used to break
    /// ties under [`DecisionRule::Majority`].
    Arbiter,
}

/// A member’s stance on a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vote {
    /// Vote in favour of the proposal.
    Approve,
    /// Vote against the proposal.
    Reject,
    /// Explicitly decline to take a side.  Counted like an absent vote when
    /// tallying ratios, but recorded separately for transparency.
    Abstain,
}

/// Rule that governs how individual [`Vote`]s are aggregated into a
/// [`CouncilDecision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionRule {
    /// Every non-arbiter member must [`Approve`](Vote::Approve).  A single
    /// [`Reject`](Vote::Reject) or [`Abstain`](Vote::Abstain) causes the
    /// proposal to fail.
    Unanimous,
    /// More [`Approve`](Vote::Approve) votes than [`Reject`](Vote::Reject)
    /// among non-arbiters.  Ties are broken by the [`Arbiter`](CouncilRole::Arbiter).
    /// All-abstain is treated as an error.
    Majority,
    /// The approval ratio (`approvals / (approvals + rejections)`) must be
    /// **strictly greater** than the configured [`supermajority_threshold`](Council::supermajority_threshold).
    /// Abstentions are excluded from the denominator.  All-abstain is treated
    /// as an error.
    Supermajority,
}

// ── CouncilMember ─────────────────────────────────────────────────────────────

/// A participant in the council with an assigned [`CouncilRole`], optional
/// [`Vote`], and optional free-text reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilMember {
    /// Unique identifier for this member (usually an agent ID).
    pub id: String,
    /// Role that determines whether this member counts toward the primary tally.
    pub role: CouncilRole,
    /// Vote cast by this member, if any.  `None` means the member has not yet
    /// voted and will be treated as [`Abstain`](Vote::Abstain) during
    /// [`Council::deliberate`].
    pub vote: Option<Vote>,
    /// Free-text explanation for the vote (e.g. chain-of-thought, critique).
    pub reasoning: Option<String>,
    /// UTC timestamp of when the vote was recorded.
    pub voted_at: Option<DateTime<Utc>>,
}

impl CouncilMember {
    /// Create a new member with the given ID and role.  All vote fields are
    /// initially unset.
    pub fn new(id: impl Into<String>, role: CouncilRole) -> Self {
        Self {
            id: id.into(),
            role,
            vote: None,
            reasoning: None,
            voted_at: None,
        }
    }

    /// Record a vote together with an optional textual rationale.  Overwrites
    /// any previously cast vote by this member.
    pub fn cast_vote(&mut self, vote: Vote, reasoning: Option<String>) {
        self.vote = Some(vote);
        self.reasoning = reasoning;
        self.voted_at = Some(Utc::now());
    }
}

// ── CouncilDecision ───────────────────────────────────────────────────────────

/// Outcome produced by [`Council::deliberate`] after all votes (or the timeout)
/// have been collected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CouncilDecision {
    /// `true` if the proposal was accepted under the active [`DecisionRule`].
    pub approved: bool,
    /// Number of non-arbiter members who voted [`Approve`](Vote::Approve).
    pub approvals: usize,
    /// Number of non-arbiter members who voted [`Reject`](Vote::Reject).
    pub rejections: usize,
    /// Number of non-arbiter members who abstained or did not vote.
    pub abstentions: usize,
    /// `true` when [`DecisionRule::Majority`] resulted in a tie and the
    /// [`Arbiter`](CouncilRole::Arbiter) cast the deciding vote.
    pub tie_broken_by_arbiter: bool,
    /// UTC timestamp of when the decision was computed.
    pub decided_at: DateTime<Utc>,
    /// Collected reasoning strings from every member who provided one,
    /// formatted as `[id(vote)]: reasoning`.
    pub reasoning: Vec<String>,
}

// ── Proposal ─────────────────────────────────────────────────────────────────

/// A concrete item put before the council for deliberation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Stable UUID for this proposal.
    pub id: String,
    /// Short human-readable title.
    pub title: String,
    /// Longer description or body of the proposal.
    pub description: String,
    /// Arbitrary structured context (e.g. serialized task state) that
    /// members may inspect when forming their votes.
    pub context: serde_json::Value,
    /// UTC timestamp of when the proposal was created.
    pub proposed_at: DateTime<Utc>,
}

impl Proposal {
    /// Build a new proposal with a generated UUID and the current timestamp.
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        context: serde_json::Value,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.into(),
            description: description.into(),
            context,
            proposed_at: Utc::now(),
        }
    }
}

// ── Council ───────────────────────────────────────────────────────────────────

/// The deliberative body.  Holds members, configuration, and orchestrates voting.
///
/// `Council` is cloneable because it wraps member state in an
/// [`Arc<RwLock<Vec<CouncilMember>>>`], allowing concurrent access from
/// multiple async tasks.
pub struct Council {
    /// Concurrent member list.  Write access is needed to add members or cast
    /// votes; read access suffices for deliberation.
    members: Arc<RwLock<Vec<CouncilMember>>>,
    /// Active decision rule (default: [`DecisionRule::Majority`]).
    decision_rule: DecisionRule,
    /// For [`DecisionRule::Supermajority`]: the fraction of
    /// `approvals / (approvals + rejections)` that must be *strictly*
    /// exceeded for the proposal to pass.  Defaults to `0.667` (≈ 2/3).
    supermajority_threshold: f64,
    /// Timeout after which a member who has not voted is treated as
    /// [`Abstain`](Vote::Abstain).  This value is stored for callers that
    /// implement external timeout logic; the council itself does not start a
    /// timer.
    vote_timeout: Duration,
}

impl Council {
    /// Create a new empty council with sensible defaults:
    /// * [`DecisionRule::Majority`]
    /// * `supermajority_threshold = 0.667`
    /// * `vote_timeout = 60` seconds
    pub fn new() -> Self {
        Self {
            members: Arc::new(RwLock::new(Vec::new())),
            decision_rule: DecisionRule::Majority,
            supermajority_threshold: 0.667,
            vote_timeout: Duration::from_secs(60),
        }
    }

    /// Override the [`DecisionRule`] (builder-style).
    pub fn with_rule(mut self, rule: DecisionRule) -> Self {
        self.decision_rule = rule;
        self
    }

    /// Override the supermajority threshold (builder-style).  Only relevant
    /// when the active rule is [`DecisionRule::Supermajority`].
    pub fn with_supermajority(mut self, threshold: f64) -> Self {
        self.supermajority_threshold = threshold;
        self
    }

    /// Override the vote-timeout duration (builder-style).  The council does
    /// not enforce this internally; it is a hint for external orchestrators.
    pub fn with_vote_timeout(mut self, timeout: Duration) -> Self {
        self.vote_timeout = timeout;
        self
    }

    // ── Member management ─────────────────────────────────────────────────────

    /// Add a new member to the council.  Members can be added at any time,
    /// but votes cast before a member joins will not be retroactively applied.
    pub async fn add_member(&self, id: impl Into<String>, role: CouncilRole) {
        self.members
            .write()
            .await
            .push(CouncilMember::new(id, role));
    }

    /// Return the current number of members (including arbiters).
    pub async fn member_count(&self) -> usize {
        self.members.read().await.len()
    }

    // ── Voting ────────────────────────────────────────────────────────────────

    /// Record a vote for the member identified by `member_id`.
    ///
    /// Returns [`ClawzError::NotFound`] if no member with that ID exists.
    /// Overwrites any previous vote by the same member.
    pub async fn cast_vote(
        &self,
        member_id: &str,
        vote: Vote,
        reasoning: Option<String>,
    ) -> Result<()> {
        let mut members = self.members.write().await;
        let member = members
            .iter_mut()
            .find(|m| m.id == member_id)
            .ok_or_else(|| ClawzError::NotFound {
                entity: "council member".into(),
                id: member_id.into(),
            })?;
        member.cast_vote(vote, reasoning);
        log::debug!("[council] member '{}' voted {:?}", member_id, vote);
        Ok(())
    }

    // ── Deliberation ─────────────────────────────────────────────────────────

    /// Evaluate the proposal and reach a [`CouncilDecision`].
    ///
    /// Non-voting members are counted as [`Abstain`](Vote::Abstain).
    ///
    /// # Errors
    ///
    /// * [`ClawzError::Validation`] – the council has no members.
    /// * [`ClawzError::Validation`] – under [`DecisionRule::Majority`] or
    ///   [`DecisionRule::Supermajority`], every non-arbiter member abstained
    ///   (zero voting total), making the ratio undefined.
    pub async fn deliberate(&self, _proposal: &Proposal) -> Result<CouncilDecision> {
        let members = self.members.read().await;

        if members.is_empty() {
            return Err(ClawzError::Validation("council has no members".into()));
        }

        // Non-arbiter members count toward the primary vote.
        let non_arbiter: Vec<&CouncilMember> = members
            .iter()
            .filter(|m| m.role != CouncilRole::Arbiter)
            .collect();

        let approvals = non_arbiter
            .iter()
            .filter(|m| m.vote == Some(Vote::Approve))
            .count();
        let rejections = non_arbiter
            .iter()
            .filter(|m| m.vote == Some(Vote::Reject))
            .count();
        let abstentions = non_arbiter
            .iter()
            .filter(|m| m.vote != Some(Vote::Approve) && m.vote != Some(Vote::Reject))
            .count();

        let total = non_arbiter.len();
        let voting_total = approvals + rejections; // abstentions don't count toward ratio

        // Collect reasoning strings.
        let reasoning: Vec<String> = members
            .iter()
            .filter_map(|m| {
                m.reasoning
                    .as_ref()
                    .map(|r| format!("[{}({:?})]: {}", m.id, m.vote.unwrap_or(Vote::Abstain), r))
            })
            .collect();

        // Determine decision.
        let (approved, tie_broken) = match self.decision_rule {
            DecisionRule::Unanimous => (approvals == total, false),

            DecisionRule::Majority => {
                if voting_total == 0 {
                    return Err(ClawzError::Validation(
                        "no votes cast (all abstained)".into(),
                    ));
                }
                let tie = approvals == rejections;
                if tie {
                    // Arbiter breaks tie.
                    let arbiter_vote = members
                        .iter()
                        .find(|m| m.role == CouncilRole::Arbiter)
                        .and_then(|m| m.vote);
                    let decided = arbiter_vote == Some(Vote::Approve);
                    (decided, true)
                } else {
                    (approvals > rejections, false)
                }
            }

            DecisionRule::Supermajority => {
                if voting_total == 0 {
                    return Err(ClawzError::Validation(
                        "no votes cast (all abstained)".into(),
                    ));
                }
                let ratio = approvals as f64 / voting_total as f64;
                (ratio > self.supermajority_threshold, false)
            }
        };

        log::info!(
            "[council] decision: approved={} approvals={}/{} rule={:?}",
            approved,
            approvals,
            total,
            self.decision_rule
        );

        Ok(CouncilDecision {
            approved,
            approvals,
            rejections,
            abstentions,
            tie_broken_by_arbiter: tie_broken,
            decided_at: Utc::now(),
            reasoning,
        })
    }
}

impl Default for Council {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_proposal() -> Proposal {
        Proposal::new("test", "test proposal", serde_json::json!({}))
    }

    #[tokio::test]
    async fn test_majority_approve() {
        let council = Council::new().with_rule(DecisionRule::Majority);
        council.add_member("a1", CouncilRole::Proponent).await;
        council.add_member("a2", CouncilRole::Reviewer).await;
        council.add_member("a3", CouncilRole::Reviewer).await;

        council.cast_vote("a1", Vote::Approve, None).await.unwrap();
        council.cast_vote("a2", Vote::Approve, None).await.unwrap();
        council.cast_vote("a3", Vote::Reject, None).await.unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        assert!(decision.approved);
        assert_eq!(decision.approvals, 2);
        assert_eq!(decision.rejections, 1);
    }

    #[tokio::test]
    async fn test_majority_reject() {
        let council = Council::new().with_rule(DecisionRule::Majority);
        council.add_member("a1", CouncilRole::Proponent).await;
        council.add_member("a2", CouncilRole::Reviewer).await;

        council.cast_vote("a1", Vote::Reject, None).await.unwrap();
        council.cast_vote("a2", Vote::Reject, None).await.unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        assert!(!decision.approved);
    }

    #[tokio::test]
    async fn test_unanimous_requires_all() {
        let council = Council::new().with_rule(DecisionRule::Unanimous);
        council.add_member("a1", CouncilRole::Proponent).await;
        council.add_member("a2", CouncilRole::Reviewer).await;

        council.cast_vote("a1", Vote::Approve, None).await.unwrap();
        council.cast_vote("a2", Vote::Reject, None).await.unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        assert!(!decision.approved);
    }

    #[tokio::test]
    async fn test_tie_broken_by_arbiter() {
        // a1=Approve, a2=Reject => tie among non-arbiters (1 vs 1).
        // Arbiter votes Approve → tie is broken, approved=true.
        let council = Council::new().with_rule(DecisionRule::Majority);
        council.add_member("a1", CouncilRole::Proponent).await;
        council.add_member("a2", CouncilRole::Reviewer).await;
        council.add_member("arbiter", CouncilRole::Arbiter).await;

        council.cast_vote("a1", Vote::Approve, None).await.unwrap();
        council.cast_vote("a2", Vote::Reject, None).await.unwrap();
        council
            .cast_vote("arbiter", Vote::Approve, Some("I decide approve".into()))
            .await
            .unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        assert!(decision.approved);
        assert!(decision.tie_broken_by_arbiter);
    }

    #[tokio::test]
    async fn test_supermajority_threshold() {
        let council = Council::new()
            .with_rule(DecisionRule::Supermajority)
            .with_supermajority(0.667);

        council.add_member("a1", CouncilRole::Proponent).await;
        council.add_member("a2", CouncilRole::Reviewer).await;
        council.add_member("a3", CouncilRole::Reviewer).await;

        // 2/3 ≈ 0.666, not strictly > 0.667 — should not pass.
        council.cast_vote("a1", Vote::Approve, None).await.unwrap();
        council.cast_vote("a2", Vote::Approve, None).await.unwrap();
        council.cast_vote("a3", Vote::Reject, None).await.unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        // 2/3 = 0.6666... which is NOT > 0.667 (strictly), so should fail
        assert!(!decision.approved);
    }

    #[tokio::test]
    async fn test_empty_council_fails() {
        let council = Council::new();
        assert!(council.deliberate(&make_proposal()).await.is_err());
    }

    #[tokio::test]
    async fn test_reasoning_collected() {
        let council = Council::new().with_rule(DecisionRule::Majority);
        council.add_member("a1", CouncilRole::Proponent).await;
        council
            .cast_vote("a1", Vote::Approve, Some("looks good".into()))
            .await
            .unwrap();

        let decision = council.deliberate(&make_proposal()).await.unwrap();
        assert!(!decision.reasoning.is_empty());
        assert!(decision.reasoning[0].contains("looks good"));
    }
}
