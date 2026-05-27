//! Constitutional Convention — agent-driven rule amendment via deliberative voting.
//!
//! This is the "participatory governance" layer enabling the rules themselves to
//! evolve from within the agent swarm.  Agents propose amendments to the live
//! [`GovernancePolicy`], the convention holds a deliberative vote, and adopted
//! amendments are parsed as `key=value` pairs and applied to the policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::governance::council::{Council, Vote};
use clawz_core::error::ClawzError;
use clawz_core::types::governance::GovernancePolicy;

// ── Supporting Types ───────────────────────────────────────────────────────────

/// A proposed change to the governing rules.
#[derive(Serialize, Deserialize)]
pub struct ProposedAmendment {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    pub proposed_by: String,
    pub proposed_at: DateTime<Utc>,
    /// Raw rule text to be parsed as `key=value` pairs when the amendment is adopted.
    pub rule_text: String,
    #[serde(skip)]
    pub vote_count: Arc<RwLock<VoteCount>>,
    pub status: AmendmentStatus,
}

impl Clone for ProposedAmendment {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            title: self.title.clone(),
            description: self.description.clone(),
            proposed_by: self.proposed_by.clone(),
            proposed_at: self.proposed_at,
            rule_text: self.rule_text.clone(),
            vote_count: Arc::new(RwLock::new(VoteCount::default())),
            status: self.status,
        }
    }
}

impl std::fmt::Debug for ProposedAmendment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProposedAmendment")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("description", &self.description)
            .field("proposed_by", &self.proposed_by)
            .field("proposed_at", &self.proposed_at)
            .field("rule_text", &self.rule_text)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl ProposedAmendment {
    /// Construct a new open amendment.  Convenience constructor used by tests.
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        proposed_by: impl Into<String>,
        rule_text: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            description: description.into(),
            proposed_by: proposed_by.into(),
            proposed_at: Utc::now(),
            rule_text: rule_text.into(),
            vote_count: Arc::new(RwLock::new(VoteCount::default())),
            status: AmendmentStatus::Open,
        }
    }
}

/// Lifecycle state of a [`ProposedAmendment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmendmentStatus {
    Open,
    Adopted,
    Rejected,
    Expired,
    Withdrawn,
}

/// Vote tally for a single amendment.
#[derive(Debug, Default)]
pub struct VoteCount {
    pub approve: usize,
    pub reject: usize,
    pub abstain: usize,
}

/// Outcome of a constitutional decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConstitutionalDecision {
    Adopted,
    Rejected,
    Tied,
    QuorumNotMet,
}

// ── ConstitutionalConvention ─────────────────────────────────────────────────

/// A deliberative body allowing agents to propose and vote on rule amendments.
///
/// The convention wraps a [`Council`] and layers amendment-specific semantics on
/// top: configurable voting window, supermajority threshold, and automatic expiry.
/// Adopted amendments are parsed as `key=value` pairs and merged into the live
/// [`GovernancePolicy`].
pub struct ConstitutionalConvention {
    /// Underlying deliberative council (provides member registry and voting).
    council: Arc<Council>,
    /// All submitted amendments (both open and resolved).
    amendments: RwLock<Vec<ProposedAmendment>>,
    /// Fraction of non-abstain votes that must be Approve for adoption (default 0.67).
    adoption_threshold: f64,
    /// Seconds after which an open amendment is automatically expired.
    voting_window_secs: u64,
}

impl ConstitutionalConvention {
    /// Create a new convention backed by the given council.
    pub fn new(council: Arc<Council>) -> Self {
        Self {
            council,
            amendments: RwLock::new(Vec::new()),
            adoption_threshold: 0.67,
            voting_window_secs: 7 * 24 * 3600, // 7 days
        }
    }

    /// Override the supermajority adoption threshold.  Values outside (0.5, 1.0]
    /// are rejected.  Note: consumes self due to non-Copy fields.
    pub fn with_threshold(self, threshold: f64) -> Self {
        assert!(
            (0.5..=1.0).contains(&threshold),
            "threshold must be in (0.5, 1.0]"
        );
        Self {
            adoption_threshold: threshold,
            ..self
        }
    }

    /// Override the voting window duration in seconds.  Note: consumes self.
    pub fn with_voting_window(self, secs: u64) -> Self {
        Self {
            voting_window_secs: secs,
            ..self
        }
    }

    /// Register a new amendment proposal and return its ID.
    pub async fn propose(&self, amendment: ProposedAmendment) -> Result<Uuid, ClawzError> {
        let id = amendment.id;
        let mut list = self.amendments.write().await;
        list.push(amendment);
        Ok(id)
    }

    /// Cast a vote on an open amendment.  Returns an error if the amendment is
    /// not open, the member is not on the council, or the member has already voted.
    pub async fn vote(
        &self,
        member_id: &str,
        amendment_id: Uuid,
        vote: Vote,
        _reasoning: Option<String>,
    ) -> Result<(), ClawzError> {
        // Verify member is a council member
        let count = self.council.member_count().await;
        if count == 0 {
            return Err(ClawzError::Governance("council has no members".into()));
        }

        // Find and update the amendment in-place
        {
            let list = self.amendments.read().await;
            let amendment =
                list.iter()
                    .find(|a| a.id == amendment_id)
                    .ok_or_else(|| ClawzError::NotFound {
                        entity: "amendment".into(),
                        id: amendment_id.to_string(),
                    })?;

            if amendment.status != AmendmentStatus::Open {
                return Err(ClawzError::Governance(format!(
                    "amendment {:?} is not open",
                    amendment.status,
                )));
            }
        }

        // Re-acquire write lock and record vote in the actual amendment
        let mut list = self.amendments.write().await;
        let amendment = list
            .iter_mut()
            .find(|a| a.id == amendment_id)
            .ok_or_else(|| ClawzError::NotFound {
                entity: "amendment".into(),
                id: amendment_id.to_string(),
            })?;

        let mut vc = amendment.vote_count.write().await;
        match vote {
            Vote::Approve => vc.approve += 1,
            Vote::Reject => vc.reject += 1,
            Vote::Abstain => vc.abstain += 1,
        }
        drop(vc);

        // Also record in council member struct for audit trail
        self.council
            .cast_vote(member_id, vote, None)
            .await
            .map_err(|e| ClawzError::Governance(e.to_string()))?;

        Ok(())
    }

    /// Reach a decision on an amendment, automatically expiring any stale open
    /// amendments before processing the target.
    ///
    /// Decision rules:
    /// - If total eligible voters == 0 → `QuorumNotMet`
    /// - If fewer than 2 votes cast → `QuorumNotMet`
    /// - If abstain votes exceed half of total → `QuorumNotMet`
    /// - Otherwise, if approve ratio > `adoption_threshold` → `Adopted`
    /// - If reject ratio >= (1 - threshold) → `Rejected`
    /// - Otherwise → `Tied`
    pub async fn decide(&self, amendment_id: Uuid) -> Result<ConstitutionalDecision, ClawzError> {
        // Expire stale open amendments first
        self.expire_stale().await;

        // Look up amendment status and vote count from the live list
        let (status, proposed_at, vc) = {
            let list = self.amendments.read().await;
            let amendment =
                list.iter()
                    .find(|a| a.id == amendment_id)
                    .ok_or_else(|| ClawzError::NotFound {
                        entity: "amendment".into(),
                        id: amendment_id.to_string(),
                    })?;

            let vc = amendment.vote_count.read().await;
            (
                amendment.status,
                amendment.proposed_at,
                (vc.approve, vc.reject, vc.abstain),
            )
        };

        // Auto-expire if still open and window has passed
        if status == AmendmentStatus::Open {
            let elapsed = Utc::now().signed_duration_since(proposed_at).num_seconds() as u64;
            if elapsed > self.voting_window_secs {
                self.update_status(amendment_id, AmendmentStatus::Expired)
                    .await?;
                return Ok(ConstitutionalDecision::Rejected);
            }
        }

        let (vc_approve, vc_reject, vc_abstain) = vc;
        let total = vc_approve + vc_reject + vc_abstain;
        let non_abstain = vc_approve + vc_reject;

        // Quorum check
        if total < 2 || non_abstain == 0 {
            return Err(ClawzError::Governance(
                "quorum not met: at least 2 votes required".into(),
            ));
        }

        let decision = if vc_approve as f64 / non_abstain as f64 >= self.adoption_threshold {
            ConstitutionalDecision::Adopted
        } else if vc_reject as f64 / non_abstain as f64 >= self.adoption_threshold {
            ConstitutionalDecision::Rejected
        } else {
            ConstitutionalDecision::Tied
        };

        // Update amendment status based on decision
        let new_status = match decision {
            ConstitutionalDecision::Adopted => AmendmentStatus::Adopted,
            ConstitutionalDecision::Rejected | ConstitutionalDecision::Tied => {
                AmendmentStatus::Rejected
            }
            ConstitutionalDecision::QuorumNotMet => AmendmentStatus::Expired,
        };

        self.update_status(amendment_id, new_status).await?;

        Ok(decision)
    }

    /// Return all currently open amendments, newest first.
    pub async fn active_amendments(&self) -> Vec<ProposedAmendment> {
        let list = self.amendments.read().await;
        list.iter()
            .filter(|a| a.status == AmendmentStatus::Open)
            .cloned()
            .collect()
    }

    /// Apply all adopted amendments to the live [`GovernancePolicy`].
    ///
    /// Parses each amendment's `rule_text` as newline-separated `key=value` pairs
    /// and merges them into the policy's rules map.  Returns the IDs of all
    /// applied amendments.
    pub async fn apply_adopted(
        &self,
        policy: &mut GovernancePolicy,
    ) -> Result<Vec<Uuid>, ClawzError> {
        let list = self.amendments.read().await;
        let mut applied = Vec::new();

        for amendment in list.iter() {
            if amendment.status != AmendmentStatus::Adopted {
                continue;
            }

            let pairs = parse_rule_pairs(&amendment.rule_text)?;
            for (key, value) in pairs {
                let rule = clawz_core::types::governance::PolicyRule::allow(
                    format!("{{\"key\":\"{}\",\"value\":\"{}\"}}", key, value),
                    format!("{}={}", key, value),
                );
                policy.rules.push(rule);
            }
            applied.push(amendment.id);
        }

        Ok(applied)
    }

    // ── Private helpers ──────────────────────────────────────────────────────

    async fn expire_stale(&self) {
        let now = Utc::now();
        let mut list = self.amendments.write().await;
        for amendment in list.iter_mut() {
            if amendment.status != AmendmentStatus::Open {
                continue;
            }
            let elapsed = now
                .signed_duration_since(amendment.proposed_at)
                .num_seconds() as u64;
            if elapsed > self.voting_window_secs {
                amendment.status = AmendmentStatus::Expired;
            }
        }
    }

    async fn update_status(
        &self,
        amendment_id: Uuid,
        status: AmendmentStatus,
    ) -> Result<(), ClawzError> {
        let mut list = self.amendments.write().await;
        if let Some(a) = list.iter_mut().find(|a| a.id == amendment_id) {
            a.status = status;
            Ok(())
        } else {
            Err(ClawzError::NotFound {
                entity: "amendment".into(),
                id: amendment_id.to_string(),
            })
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `rule_text` as newline-separated `key=value` pairs.
fn parse_rule_pairs(rule_text: &str) -> Result<Vec<(String, String)>, ClawzError> {
    let mut pairs = Vec::new();
    for line in rule_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim().to_string();
            let value = line[pos + 1..].trim().to_string();
            if !key.is_empty() {
                pairs.push((key, value));
            }
        }
    }
    Ok(pairs)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::governance::council::CouncilRole;

    async fn make_convention() -> ConstitutionalConvention {
        let base = Council::new().with_vote_timeout(std::time::Duration::from_secs(60));
        let council = Arc::new(base);
        council
            .add_member(String::from("alice"), CouncilRole::Reviewer)
            .await;
        council
            .add_member(String::from("bob"), CouncilRole::Reviewer)
            .await;
        council
            .add_member(String::from("carol"), CouncilRole::Proponent)
            .await;
        ConstitutionalConvention::new(council)
    }

    #[tokio::test]
    async fn propose_returns_id_and_stores() {
        let conv = make_convention().await;
        let amendment = ProposedAmendment::new(
            "Raise threshold",
            "Raise supermajority to 70%",
            "alice",
            "adoption_threshold=0.70",
        );
        let id = conv.propose(amendment.clone()).await.unwrap();
        assert_eq!(id, amendment.id);
        let active = conv.active_amendments().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].title, "Raise threshold");
    }

    #[tokio::test]
    async fn vote_updates_tally() {
        let conv = make_convention().await;
        let amendment =
            ProposedAmendment::new("Test vote", "Test description", "alice", "key=value");
        let id = conv.propose(amendment).await.unwrap();

        conv.vote("alice", id, Vote::Approve, None).await.unwrap();
        conv.vote("bob", id, Vote::Reject, None).await.unwrap();

        let list = conv.amendments.read().await;
        let vc = list
            .iter()
            .find(|a| a.id == id)
            .unwrap()
            .vote_count
            .read()
            .await;
        assert_eq!(vc.approve, 1);
        assert_eq!(vc.reject, 1);
        assert_eq!(vc.abstain, 0);
    }

    #[tokio::test]
    async fn decide_adopted_with_supermajority() {
        let conv = make_convention().await;
        let amendment = ProposedAmendment::new(
            "Adopt this",
            "Should pass with 2/3",
            "carol",
            "threshold=0.75",
        );
        let id = conv.propose(amendment).await.unwrap();

        conv.vote("alice", id, Vote::Approve, None).await.unwrap();
        conv.vote("bob", id, Vote::Approve, None).await.unwrap();
        // 2 approve, 0 reject → 100% approve > 67% threshold → adopted

        let decision = conv.decide(id).await.unwrap();
        assert_eq!(decision, ConstitutionalDecision::Adopted);
    }

    #[tokio::test]
    async fn decide_rejected_with_supermajority_reject() {
        let conv = make_convention().await;
        let amendment = ProposedAmendment::new("Reject this", "Should fail", "carol", "foo=bar");
        let id = conv.propose(amendment).await.unwrap();

        conv.vote("alice", id, Vote::Reject, None).await.unwrap();
        conv.vote("bob", id, Vote::Reject, None).await.unwrap();
        // 2 reject, 0 approve → 100% reject >= 67% threshold → rejected

        let decision = conv.decide(id).await.unwrap();
        assert_eq!(decision, ConstitutionalDecision::Rejected);
    }

    #[tokio::test]
    async fn decide_quorum_not_met_with_single_vote() {
        let conv = make_convention().await;
        let amendment =
            ProposedAmendment::new("Too few votes", "Only one vote cast", "carol", "x=y");
        let id = conv.propose(amendment).await.unwrap();

        conv.vote("alice", id, Vote::Approve, None).await.unwrap();
        // Only 1 vote — should fail quorum

        let err = conv.decide(id).await.unwrap_err();
        assert!(matches!(err, ClawzError::Governance(_)));
    }

    #[tokio::test]
    async fn apply_adopted_parses_rule_text() {
        let conv = make_convention().await;
        let amendment = ProposedAmendment::new(
            "Apply rules",
            "Parses key=value pairs",
            "carol",
            "max_retries=3\ntimeout_secs=30\n# comment\nenabled=true",
        );
        let id = conv.propose(amendment).await.unwrap();
        conv.vote("alice", id, Vote::Approve, None).await.unwrap();
        conv.vote("bob", id, Vote::Approve, None).await.unwrap();
        conv.vote("carol", id, Vote::Approve, None).await.unwrap();
        conv.decide(id).await.unwrap();

        let mut policy = clawz_core::types::governance::GovernancePolicy {
            id: "test".into(),
            name: "test".into(),
            rules: vec![],
            enabled: true,
            priority: 0,
        };

        let applied = conv.apply_adopted(&mut policy).await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(policy.rules.len(), 3);
        // PolicyRule uses description field (not name)
        assert!(
            policy
                .rules
                .iter()
                .any(|r| r.description.contains("max_retries"))
        );
        assert!(
            policy
                .rules
                .iter()
                .any(|r| r.description.contains("timeout_secs"))
        );
        assert!(
            policy
                .rules
                .iter()
                .any(|r| r.description.contains("enabled"))
        );
    }

    #[tokio::test]
    async fn active_amendments_excludes_resolved() {
        let conv = make_convention().await;
        let a1 = ProposedAmendment::new("Open", "desc", "alice", "a=b");
        let a2 = ProposedAmendment::new("Resolved", "desc", "bob", "c=d");
        let id2 = conv.propose(a2).await.unwrap();
        let id1 = conv.propose(a1).await.unwrap();

        // Vote and decide a2
        conv.vote("alice", id2, Vote::Reject, None).await.unwrap();
        conv.vote("bob", id2, Vote::Reject, None).await.unwrap();
        conv.vote("carol", id2, Vote::Reject, None).await.unwrap();
        conv.decide(id2).await.unwrap();

        let active = conv.active_amendments().await;
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, id1);
    }

    #[tokio::test]
    async fn with_threshold_builder() {
        let council = Arc::new(Council::new());
        council.add_member("alice", CouncilRole::Proponent).await;
        council.add_member("bob", CouncilRole::Reviewer).await;
        council.add_member("carol", CouncilRole::Reviewer).await;
        // Use 0.5 threshold so 2/3 = 66.7% meets it
        let conv = ConstitutionalConvention::new(council).with_threshold(0.5);
        let amendment = ProposedAmendment::new("50%", "desc", "alice", "t=0.5");
        let id = conv.propose(amendment).await.unwrap();

        // With 0.5 threshold, 2/3 approve (66.7%) is enough to adopt
        conv.vote("alice", id, Vote::Approve, None).await.unwrap();
        conv.vote("bob", id, Vote::Approve, None).await.unwrap();
        conv.vote("carol", id, Vote::Reject, None).await.unwrap();
        // 2 approve / 3 non-abstain = 66.7% >= 50% → adopted
        let decision = conv.decide(id).await.unwrap();
        assert_eq!(decision, ConstitutionalDecision::Adopted);
    }
}
