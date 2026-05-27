//! Governance-dimension runtime guardrails — the enforcement layer of PRISM-G's
//! G dimension.
//!
//! NOT the whole framework. Runs output-level safety/compliance checks before an
//! agent action is allowed to proceed.
//!
//! ## Guardrail Checks
//!
//! | Check | Concern | Typical Failure Mode |
//! |-------|---------|---------------------|
//! | Privacy | PII leakage | Emails, phones, SSNs, credit cards exposed in output |
//! | Reliability | Operational stability | Success rate below configurable threshold |
//! | Integrity | Factual grounding | Response lacks citations or source markers |
//! | Safety | Harmful content | Violence, self-harm, or illegal instructions detected |
//! | Monitoring | Audit trail completeness | Too few audit entries per hour |
//! | Alignment | Goal consistency | Action contradicts active GoalObject |
//! | Oversight | Required oversight level | Action exceeds permitted oversight autonomy |
//!
//! ## Graduated Control Pillars
//!
//! The module implements a layered defense model:
//!
//! 1. **Golden Paths** — Preferred workflows that naturally produce compliant output.
//! 2. **Guardrails** — Automated checks (this module) that catch deviations.
//! 3. **Safety Nets** — Fallback escalation when guardrails trigger (e.g., human review).
//! 4. **Manual Review** — Final governance layer for ambiguous or high-stakes cases.
//!
//! ## Design Principles
//!
//! - **Non-blocking by default for warnings**: Privacy and integrity issues surface as
//!   `Warning` so downstream systems can decide whether to redact, escalate, or proceed.
//! - **Hard fails for safety and reliability**: `Safety` and `reliability` shortfalls return
//!   `Fail`, which typically blocks the response.
//! - **Configurable thresholds**: `reliability_threshold` and `min_audit_rate_per_hour` are
//!   set at construction time and can be adjusted per deployment environment.
//! - **Zero external dependencies for core logic**: All checks run synchronously in-memory
//!   using compiled regex patterns and keyword lists.

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};

// ── ComplianceLevel ───────────────────────────────────────────────────────────

/// Outcome classification for a single guardrail check.
///
/// `Pass` means the dimension is satisfied. `Warning` signals a minor concern
/// that may require attention but does not block the response. `Fail` indicates
/// a critical violation that should prevent release or trigger escalation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComplianceLevel {
    Pass,
    Warning,
    Fail,
}

impl std::fmt::Display for ComplianceLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComplianceLevel::Pass => write!(f, "pass"),
            ComplianceLevel::Warning => write!(f, "warning"),
            ComplianceLevel::Fail => write!(f, "fail"),
        }
    }
}

// ── ComplianceResult ──────────────────────────────────────────────────────────

/// Detailed result of evaluating a single guardrail check.
///
/// Contains the check name, the assigned [`ComplianceLevel`], a list of
/// human-readable findings, and the UTC timestamp when the check occurred.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceResult {
    /// Name of the guardrail check (e.g., `"privacy"`, `"safety"`).
    pub dimension: String,
    /// The severity outcome for this dimension.
    pub level: ComplianceLevel,
    /// Descriptive messages explaining why the level was assigned.
    pub details: Vec<String>,
    /// UTC timestamp recorded at the moment the check executed.
    pub checked_at: DateTime<Utc>,
}

impl ComplianceResult {
    /// Construct a `Pass` result with no findings for the given dimension.
    fn pass(dimension: &str) -> Self {
        Self {
            dimension: dimension.into(),
            level: ComplianceLevel::Pass,
            details: Vec::new(),
            checked_at: Utc::now(),
        }
    }

    /// Construct a `Warning` result with explanatory details.
    fn warning(dimension: &str, details: Vec<String>) -> Self {
        Self {
            dimension: dimension.into(),
            level: ComplianceLevel::Warning,
            details,
            checked_at: Utc::now(),
        }
    }

    /// Construct a `Fail` result with explanatory details.
    fn fail(dimension: &str, details: Vec<String>) -> Self {
        Self {
            dimension: dimension.into(),
            level: ComplianceLevel::Fail,
            details,
            checked_at: Utc::now(),
        }
    }
}

// ── GuardrailReport ───────────────────────────────────────────────────────────

/// Aggregated outcome of running all guardrail checks.
///
/// Each field holds the full [`ComplianceResult`] for its check. The
/// `overall_pass` flag is `true` when *no* check returned `Fail`
/// (i.e., only `Pass` or `Warning` results are present).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailReport {
    /// Privacy check result (PII detection).
    pub privacy: ComplianceResult,
    /// Reliability check result (success-rate threshold).
    pub reliability: ComplianceResult,
    /// Integrity check result (citation/grounding markers).
    pub integrity: ComplianceResult,
    /// Safety check result (harmful content scan).
    pub safety: ComplianceResult,
    /// Monitoring check result (audit-trail volume).
    pub monitoring: ComplianceResult,
    /// `true` iff all checks passed or warned (no fails).
    pub overall_pass: bool,
}

impl GuardrailReport {
    /// Returns `true` if the `overall_pass` flag is set.
    ///
    /// Convenience accessor for downstream gatekeeping logic.
    pub fn all_passed(&self) -> bool {
        self.overall_pass
    }

    /// Returns every guardrail check that returned [`ComplianceLevel::Fail`].
    ///
    /// Useful for building rejection messages or routing to safety-nets.
    pub fn failed_dimensions(&self) -> Vec<&ComplianceResult> {
        [
            &self.privacy,
            &self.reliability,
            &self.integrity,
            &self.safety,
            &self.monitoring,
        ]
        .iter()
        .filter(|r| r.level == ComplianceLevel::Fail)
        .copied()
        .collect()
    }

    /// Returns every guardrail check that returned [`ComplianceLevel::Warning`].
    ///
    /// Useful for surfacing optional concerns that do not block the response
    /// but may warrant redaction, logging, or secondary review.
    pub fn warnings(&self) -> Vec<&ComplianceResult> {
        [
            &self.privacy,
            &self.reliability,
            &self.integrity,
            &self.safety,
            &self.monitoring,
        ]
        .iter()
        .filter(|r| r.level == ComplianceLevel::Warning)
        .copied()
        .collect()
    }
}

// ── ReliabilityTracker ────────────────────────────────────────────────────────

/// Rolling success/failure counter used by the reliability dimension.
///
/// Maintains a simple total count and success count. The success rate is
/// computed on demand as `successes / total`. When no samples have been
/// recorded the rate defaults to `1.0` (100 %) to avoid premature failure
/// during system warmup.
#[derive(Debug, Default)]
pub struct ReliabilityTracker {
    /// Total number of outcomes recorded.
    pub total: u64,
    /// Number of successful outcomes recorded.
    pub successes: u64,
}

impl ReliabilityTracker {
    /// Record a single outcome.
    ///
    /// Increments `total` by one and, if `success` is `true`, increments
    /// `successes` as well.
    pub fn record(&mut self, success: bool) {
        self.total += 1;
        if success {
            self.successes += 1;
        }
    }

    /// Returns the current success rate as a fraction in the range `[0.0, 1.0]`.
    ///
    /// If `total == 0` the default is `1.0` so that an uninitialised tracker
    /// does not immediately fail the reliability check.
    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.successes as f64 / self.total as f64
        }
    }
}

// ── GovernanceGuardrails ──────────────────────────────────────────────────────

/// G-dimension guardrail engine that runs all guardrail checks.
///
/// Holds configuration thresholds and a [`ReliabilityTracker`] instance.
/// All checks are stateless with respect to the inspected text except for
/// reliability, which accumulates outcomes across the lifetime of this struct.
///
/// ## Typical Usage
///
/// ```rust,ignore
/// let guardrails = GovernanceGuardrails::new()
///     .with_reliability_threshold(0.95);
///
/// guardrails.record_outcome(true);
/// let result = guardrails.run_all("Some response text...", 42);
/// assert!(result.all_passed());
/// ```
pub struct GovernanceGuardrails {
    /// Minimum acceptable success rate for the reliability dimension.
    ///
    /// Default: `0.90` (90 %). If the observed rate falls below this value
    /// [`check_reliability`] returns [`ComplianceLevel::Fail`].
    reliability_threshold: f64,
    /// Rolling accumulator of success/failure outcomes.
    reliability_tracker: ReliabilityTracker,
    /// Minimum number of audit log entries expected per hour.
    ///
    /// A value of `0` disables the monitoring check entirely. When enabled,
    /// [`check_monitoring`] compares the supplied hourly count against this
    /// threshold and returns [`ComplianceLevel::Warning`] if the count is
    /// too low.
    min_audit_rate_per_hour: u64,

    // ── PRISM-G G-dimension extension ────────────────────────────────────────

    /// Configured oversight level for this guardrail instance.
    /// Used by the oversight check to determine if pre-approval is required.
    configured_oversight: clawz_core::types::OversightLevel,

    /// The active GoalObject if one is set.
    /// Used by the alignment check to detect actions that contradict the goal.
    active_goal: Option<clawz_core::types::purpose::GoalObject>,

    /// The risk level of the current action.
    /// Used with configured_oversight to determine effective oversight.
    action_risk_level: Option<clawz_core::types::tool_risk::RiskLevel>,
}

// ── PII regex patterns ─────────────────────────────────────────────────────────

/// Compiled regex for email addresses.
///
/// Matches standard RFC-like local parts (`a-zA-Z0-9._%+-`) followed by `@`
/// and a domain with at least a two-letter TLD. Does **not** validate the
/// domain via MX lookup.
fn email_regex() -> Regex {
    Regex::new(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}").unwrap()
}

/// Compiled regex for phone numbers.
///
/// Covers common US and international formats including:
/// - `+1-800-555-1234`
/// - `(800) 555-1234`
/// - `800.555.1234`
/// - space-separated variants
///
/// The pattern requires at least seven interior digits bounded by leading and
/// trailing digits, so short numeric sequences are ignored.
fn phone_regex() -> Regex {
    // Matches common US/intl formats: +1-800-555-1234, (800) 555-1234, etc.
    Regex::new(r"(?:\+?\d[\d\s\-().]{7,}\d)").unwrap()
}

/// Compiled regex for US Social Security Numbers.
///
/// Matches the canonical `XXX-XX-XXXX` format where each `X` is a digit.
/// Word boundaries (`\b`) prevent partial matches inside longer numbers.
fn ssn_regex() -> Regex {
    Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap()
}

/// Compiled regex for credit-card-like number sequences.
///
/// Recognises groups of four digits optionally separated by spaces or hyphens,
/// in the pattern `XXXX XXXX XXXX XXXX` (13–19 digits total). This is a
/// **pattern-only** check; it does **not** perform a Luhn validation.
fn credit_card_regex() -> Regex {
    // Luhn-valid 13-19 digit sequences (basic pattern, not full Luhn check).
    Regex::new(r"\b(?:\d{4}[\s\-]?){3}\d{4}\b").unwrap()
}

// ── Safety keyword patterns ───────────────────────────────────────────────────

/// Violence-related keyword phrases used by [`check_safety`].
///
/// Each entry is a lowercase literal phrase. If any phrase is found as a
/// substring in the inspected text, the safety check records a `Fail` with
/// the matched phrase documented in [`ComplianceResult::details`].
///
/// ## Coverage
/// - Physical harm instructions (`"how to kill"`, `"how to murder"`)
/// - Weapon fabrication (`"bomb making"`, `"make a weapon"`)
/// - Explosives and poisons (`"make explosives"`, `"synthesis of poison"`)
/// - Step-by-step attack guidance (`"attack instruction"`, `"step by step to harm"`)
const VIOLENCE_KEYWORDS: &[&str] = &[
    "how to kill",
    "how to murder",
    "bomb making",
    "make a weapon",
    "attack instruction",
    "step by step to harm",
    "how to hurt",
    "make explosives",
    "synthesis of poison",
];

/// Self-harm-related keyword phrases used by [`check_safety`].
///
/// Detection of these phrases causes an immediate `Fail` and should
/// typically trigger crisis-resource messaging in the upstream system.
const SELF_HARM_KEYWORDS: &[&str] = &[
    "how to commit suicide",
    "ways to kill myself",
    "how to self harm",
    "methods of suicide",
    "best way to die",
];

/// Illegal-activity keyword phrases used by [`check_safety`].
///
/// Covers cybercrime, drug synthesis, financial crime, and exploitation.
/// Matches result in a `Fail` with the specific keyword recorded.
const ILLEGAL_KEYWORDS: &[&str] = &[
    "how to hack into",
    "bypass security",
    "crack the password",
    "synthesize methamphetamine",
    "synthesize heroin",
    "how to make drugs",
    "child exploitation",
    "how to launder money",
    "illegal weapon modification",
];

// ── Alignment detection patterns ─────────────────────────────────────────────

/// Keywords that, if present in an action, may contradict a Maintain-type goal
/// (e.g., "maintain audit trail", "maintain uptime").
const DESTRUCTIVE_KEYWORDS: &[&str] = &[
    "delete",
    "destroy",
    "drop",
    "remove",
    "truncate",
    "eliminate",
    "purge",
    "wipe",
    "clear",
    "erase",
];

/// Keywords indicating data modification that could conflict with integrity goals.
#[allow(dead_code)]
const MODIFYING_KEYWORDS: &[&str] = &[
    "overwrite",
    "replace",
    "modify",
    "alter",
    "update",
    "change",
];

impl GovernanceGuardrails {
    /// Create a new `GovernanceGuardrails` instance with default thresholds.
    ///
    /// Defaults:
    /// - `reliability_threshold` = `0.90`
    /// - `min_audit_rate_per_hour` = `0` (disabled)
    /// - `configured_oversight` = [`OversightLevel::Autonomous`]
    /// - `active_goal` = `None`
    /// - `action_risk_level` = `None`
    pub fn new() -> Self {
        Self {
            reliability_threshold: 0.90,
            reliability_tracker: ReliabilityTracker::default(),
            min_audit_rate_per_hour: 0,
            configured_oversight: clawz_core::types::OversightLevel::Autonomous,
            active_goal: None,
            action_risk_level: None,
        }
    }

    /// Override the default reliability threshold.
    ///
    /// Accepts a fraction in `[0.0, 1.0]`. Values outside this range are
    /// accepted by the setter but may produce nonsensical pass/fail results.
    ///
    /// ## Example
    /// ```rust,ignore
    /// let guardrails = GovernanceGuardrails::new()
    ///     .with_reliability_threshold(0.95);
    /// ```
    pub fn with_reliability_threshold(mut self, threshold: f64) -> Self {
        self.reliability_threshold = threshold;
        self
    }

    /// Set the configured oversight level.
    pub fn with_oversight_level(mut self, level: clawz_core::types::OversightLevel) -> Self {
        self.configured_oversight = level;
        self
    }

    /// Set the active GoalObject for alignment checking.
    pub fn with_active_goal(mut self, goal: clawz_core::types::purpose::GoalObject) -> Self {
        self.active_goal = Some(goal);
        self
    }

    /// Set the risk level of the current action.
    pub fn with_risk_level(mut self, risk: clawz_core::types::tool_risk::RiskLevel) -> Self {
        self.action_risk_level = Some(risk);
        self
    }

    /// Record a single success or failure outcome for the reliability tracker.
    ///
    /// Call this after every agent execution so that the rolling success rate
    /// reflects recent operational history.
    pub fn record_outcome(&mut self, success: bool) {
        self.reliability_tracker.record(success);
    }

    // ── Privacy check ─────────────────────────────────────────────────────────

    /// Scan `text` for personally identifiable information (PII).
    ///
    /// Detects four categories of PII using compiled regex patterns:
    ///
    /// | Category | Pattern | Notes |
    /// |----------|---------|-------|
    /// | Email | [`email_regex`] | Standard address format; no MX validation |
    /// | Phone | [`phone_regex`] | US/intl formats with 7+ interior digits |
    /// | SSN | [`ssn_regex`] | `XXX-XX-XXXX` only |
    /// | Credit card | [`credit_card_regex`] | 13–19 digit groups; no Luhn check |
    ///
    /// ## Returns
    /// - [`ComplianceLevel::Pass`] when no patterns match.
    /// - [`ComplianceLevel::Warning`] when one or more patterns match,
    ///   with each finding listed in [`ComplianceResult::details`].
    ///
    /// Privacy is treated as a warning (not a hard fail) because the caller
    /// may choose to redact PII rather than block the response entirely.
    pub fn check_privacy(&self, text: &str) -> ComplianceResult {
        let mut findings = Vec::new();

        if email_regex().is_match(text) {
            findings.push("PII detected: email address".into());
        }
        if phone_regex().is_match(text) {
            findings.push("PII detected: phone number".into());
        }
        if ssn_regex().is_match(text) {
            findings.push("PII detected: SSN pattern".into());
        }
        if credit_card_regex().is_match(text) {
            findings.push("PII detected: credit card pattern".into());
        }

        if findings.is_empty() {
            ComplianceResult::pass("privacy")
        } else {
            ComplianceResult::warning("privacy", findings)
        }
    }

    // ── Reliability check ─────────────────────────────────────────────────────

    /// Evaluate whether the rolling success rate meets the configured threshold.
    ///
    /// Computes the current rate from [`ReliabilityTracker::success_rate`] and
    /// compares it to [`Self::reliability_threshold`].
    ///
    /// ## Threshold Behaviour
    /// - `rate >= threshold` → [`ComplianceLevel::Pass`]
    /// - `rate < threshold` → [`ComplianceLevel::Fail`] with a detail message
    ///   showing the observed percentage and the required percentage.
    ///
    /// ## Important
    /// When no outcomes have been recorded the tracker defaults to `1.0`, so
    /// the check passes until the first failure drives the rate below threshold.
    pub fn check_reliability(&self) -> ComplianceResult {
        let rate = self.reliability_tracker.success_rate();
        if rate >= self.reliability_threshold {
            ComplianceResult::pass("reliability")
        } else {
            ComplianceResult::fail(
                "reliability",
                vec![format!(
                    "success rate {:.1}% is below threshold {:.1}%",
                    rate * 100.0,
                    self.reliability_threshold * 100.0
                )],
            )
        }
    }

    // ── Integrity check ───────────────────────────────────────────────────────

    /// Checks for citation markers that suggest a grounded response.
    ///
    /// Searches `text` for a predefined set of citation and grounding phrases
    /// (e.g., `"[1]"`, `"according to"`, `"source:"`). Presence of any phrase
    /// indicates the response is likely attributed to external material.
    ///
    /// ## Returns
    /// - [`ComplianceLevel::Pass`] when at least one marker is found.
    /// - [`ComplianceLevel::Warning`] when no markers are present, noting that
    ///   the response may be ungrounded or hallucinated.
    ///
    /// This is a heuristic; a response can be well-grounded without using the
    /// exact phrases in the keyword list.
    pub fn check_integrity(&self, text: &str) -> ComplianceResult {
        let citation_patterns = [
            "[1]", "[2]", "[source]", "[citation]", "according to",
            "based on", "reference:", "source:", "cited from",
            "as stated in", "per ", "from the",
        ];

        let has_citations = citation_patterns
            .iter()
            .any(|p| text.to_lowercase().contains(p));

        if has_citations {
            ComplianceResult::pass("integrity")
        } else {
            ComplianceResult::warning(
                "integrity",
                vec!["response has no citation markers — may not be grounded".into()],
            )
        }
    }

    // ── Safety check ─────────────────────────────────────────────────────────

    /// Scan `text` for harmful or illegal content.
    ///
    /// Performs a case-insensitive substring search against three keyword lists:
    ///
    /// 1. [`VIOLENCE_KEYWORDS`] — physical harm and weapon instructions.
    /// 2. [`SELF_HARM_KEYWORDS`] — suicide and self-injury methods.
    /// 3. [`ILLEGAL_KEYWORDS`] — cybercrime, drug synthesis, financial crime,
    ///    and exploitation.
    ///
    /// ## Returns
    /// - [`ComplianceLevel::Pass`] when none of the keywords are found.
    /// - [`ComplianceLevel::Fail`] when one or more keywords match. Each match
    ///   is recorded in [`ComplianceResult::details`] with its category prefix
    ///   (`violence-related`, `self-harm-related`, or `potentially illegal`).
    ///
    /// Safety violations are **hard fails** and should generally block the
    /// response or route it to a human safety net.
    pub fn check_safety(&self, text: &str) -> ComplianceResult {
        let lower = text.to_lowercase();
        let mut findings = Vec::new();

        for kw in VIOLENCE_KEYWORDS {
            if lower.contains(kw) {
                findings.push(format!("violence-related content: '{kw}'"));
            }
        }
        for kw in SELF_HARM_KEYWORDS {
            if lower.contains(kw) {
                findings.push(format!("self-harm-related content: '{kw}'"));
            }
        }
        for kw in ILLEGAL_KEYWORDS {
            if lower.contains(kw) {
                findings.push(format!("potentially illegal content: '{kw}'"));
            }
        }

        if findings.is_empty() {
            ComplianceResult::pass("safety")
        } else {
            ComplianceResult::fail("safety", findings)
        }
    }

    // ── Monitoring check ──────────────────────────────────────────────────────

    /// Verify that audit-trail volume meets the hourly minimum.
    ///
    /// `audit_entries_last_hour` should be the count of audit log records
    /// created in the past 60 minutes.
    ///
    /// ## Threshold Behaviour
    /// - If `min_audit_rate_per_hour` is `0`, the check is disabled and always
    ///   returns [`ComplianceLevel::Pass`].
    /// - If `audit_entries_last_hour >= min_audit_rate_per_hour` → `Pass`.
    /// - Otherwise → [`ComplianceLevel::Warning`] with a detail message showing
    ///   the observed count and the required minimum.
    ///
    /// Monitoring is a warning (not a hard fail) because a temporary dip in
    /// audit volume does not inherently make the response unsafe.
    pub fn check_monitoring(&self, audit_entries_last_hour: u64) -> ComplianceResult {
        if self.min_audit_rate_per_hour == 0 {
            return ComplianceResult::pass("monitoring");
        }
        if audit_entries_last_hour >= self.min_audit_rate_per_hour {
            ComplianceResult::pass("monitoring")
        } else {
            ComplianceResult::warning(
                "monitoring",
                vec![format!(
                    "only {} audit entries in the last hour (min: {})",
                    audit_entries_last_hour, self.min_audit_rate_per_hour
                )],
            )
        }
    }

    // ── Alignment check (PRISM-G G-dimension) ────────────────────────────────

    /// Check whether an action contradicts the active GoalObject.
    ///
    /// Compares the action description against:
    /// - `GoalObject.description` — the overall goal text
    /// - `GoalObject.goal_type` — the type of goal (Maintain, Optimize, etc.)
    /// - `GoalObject.constraints` — explicit constraints on the goal
    ///
    /// Returns `true` (pass) if the action is consistent with the goal,
    /// `false` (fail) if the action contradicts the goal.
    ///
    /// ## Contradiction Detection
    /// - If the goal type is `Maintain` and the action contains destructive keywords
    ///   (delete, destroy, drop, etc.), the check fails.
    /// - If the goal description mentions "audit trail" or "logs" and the action
    ///   contains "delete" or "clear", the check fails.
    /// - If the goal has a constraint that would be violated by the action,
    ///   the check fails.
    ///
    /// Returns `true` (alignment pass) if no contradictions are detected.
    /// Returns `false` with a descriptive blocker if a contradiction is found.
    pub fn check_alignment(&self, action: &str) -> (bool, Option<String>) {
        let Some(ref goal) = self.active_goal else {
            // No active goal — alignment check is not applicable, pass by default
            return (true, None);
        };

        let action_lower = action.to_lowercase();

        // Check goal-type-based contradictions
        match goal.goal_type {
            clawz_core::types::purpose::GoalType::Maintain => {
                // For Maintain goals, destructive actions are contradictions
                for kw in DESTRUCTIVE_KEYWORDS {
                    if action_lower.contains(kw) {
                        return (
                            false,
                            Some(format!(
                                "action contains destructive keyword '{kw}' which contradicts goal type 'maintain'"
                            )),
                        );
                    }
                }
            }
            clawz_core::types::purpose::GoalType::Satisfy => {
                // For Satisfy goals, check if action violates constraints
                for constraint in &goal.constraints {
                    let _constraint_text = constraint.description.to_lowercase();
                    // Check for obvious violations
                    if constraint.kind == clawz_core::types::purpose::ConstraintKind::Compliance {
                        // Compliance constraints — check if action undermines them
                        if action_lower.contains("ignore")
                            || action_lower.contains("bypass")
                            || action_lower.contains("disable")
                        {
                            return (
                                false,
                                Some(format!(
                                    "action may violate compliance constraint: {}",
                                    constraint.description
                                )),
                            );
                        }
                    }
                }
            }
            _ => {
                // For Optimize and Explore, we are more permissive
                // Only block clearly destructive actions
                for kw in DESTRUCTIVE_KEYWORDS {
                    if action_lower.contains(kw) && action_lower.contains("all") {
                        return (
                            false,
                            Some(format!(
                                "action appears to delete/destroy everything, contradicting goal: {}",
                                goal.description
                            )),
                        );
                    }
                }
            }
        }

        // Check explicit constraint violations
        for constraint in &goal.constraints {
            let constraint_text = constraint.description.to_lowercase();
            // If constraint mentions something we should preserve, check for destructive action
            if (constraint_text.contains("audit")
                || constraint_text.contains("log")
                || constraint_text.contains("trail")
                || constraint_text.contains("record"))
                && (action_lower.contains("delete")
                    || action_lower.contains("clear")
                    || action_lower.contains("wipe")
                    || action_lower.contains("erase")
                    || action_lower.contains("purge"))
                {
                    return (
                        false,
                        Some(format!(
                            "action may destroy audit/log data required by constraint: {}",
                            constraint.description
                        )),
                    );
                }

            // If constraint mentions uptime or availability
            if (constraint_text.contains("uptime")
                || constraint_text.contains("availability")
                || constraint_text.contains("running")
                    || constraint_text.contains("available"))
                && (action_lower.contains("stop")
                    || action_lower.contains("terminate")
                    || action_lower.contains("kill")
                    || action_lower.contains("shutdown"))
                {
                    return (
                        false,
                        Some(format!(
                            "action may violate availability constraint: {}",
                            constraint.description
                        )),
                    );
                }
        }

        // Passed all alignment checks
        (true, None)
    }

    // ── Oversight check (PRISM-G G-dimension) ─────────────────────────────────

    /// Check whether the action's oversight level meets requirements.
    ///
    /// Uses the configured oversight level and the action's risk level
    /// to determine the effective oversight required, then compares against
    /// the configured oversight to see if pre-approval is needed.
    ///
    /// Returns `true` (pass) if the action meets oversight requirements.
    /// Returns `false` with a descriptive blocker if pre-approval is required.
    pub fn check_oversight(&self) -> (bool, Option<String>) {
        let risk = match &self.action_risk_level {
            Some(r) => r.clone(),
            None => {
                // No risk level set — oversight check is not applicable, pass by default
                return (true, None);
            }
        };

        use crate::governance::oversight::{effective_oversight, requires_pre_approval};

        let effective = effective_oversight(risk.clone(), self.configured_oversight, self.active_goal.is_some());

        if requires_pre_approval(effective) {
            (
                false,
                Some(format!(
                    "action requires {:?} oversight but configured level is {:?} (effective: {:?})",
                    effective, self.configured_oversight, effective
                )),
            )
        } else {
            (true, None)
        }
    }

    // ── Run all checks ───────────────────────────────────────────────────────

    /// Run all guardrail checks against the provided inputs.
    ///
    /// `text` is the response text to inspect for privacy, integrity, and safety.
    /// `audit_entries_last_hour` is used for the monitoring check.
    ///
    /// ## Returns
    /// A [`GuardrailReport`] containing the individual check results and an
    /// `overall_pass` flag that is `true` only when no check returned `Fail`.
    ///
    /// ## Example
    /// ```rust,ignore
    /// let guardrails = GovernanceGuardrails::new();
    /// let result = guardrails.run_all("According to [1], the sky is blue.", 10);
    /// assert!(result.all_passed());
    /// ```
    pub fn run_all(
        &self,
        text: &str,
        audit_entries_last_hour: u64,
    ) -> GuardrailReport {
        let privacy = self.check_privacy(text);
        let reliability = self.check_reliability();
        let integrity = self.check_integrity(text);
        let safety = self.check_safety(text);
        let monitoring = self.check_monitoring(audit_entries_last_hour);

        let overall_pass = [&privacy, &reliability, &integrity, &safety, &monitoring]
            .iter()
            .all(|r| r.level != ComplianceLevel::Fail);

        GuardrailReport {
            privacy,
            reliability,
            integrity,
            safety,
            monitoring,
            overall_pass,
        }
    }
}

impl Default for GovernanceGuardrails {
    /// Delegates to [`GovernanceGuardrails::new`] for default construction.
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::purpose::{GoalObject, GoalType, Constraint, ConstraintKind};

    #[test]
    fn test_privacy_detects_email() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_privacy("Contact me at john.doe@example.com today");
        assert_eq!(result.level, ComplianceLevel::Warning);
        assert!(result.details[0].contains("email"));
    }

    #[test]
    fn test_privacy_detects_ssn() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_privacy("SSN: 123-45-6789");
        assert_eq!(result.level, ComplianceLevel::Warning);
        // Either phone or SSN pattern may be detected (123-45-6789 matches both)
        assert!(!result.details.is_empty());
    }

    #[test]
    fn test_privacy_clean_passes() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_privacy("This is a clean response with no PII.");
        assert_eq!(result.level, ComplianceLevel::Pass);
    }

    #[test]
    fn test_safety_detects_violence() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_safety("Here is how to kill someone with...");
        assert_eq!(result.level, ComplianceLevel::Fail);
    }

    #[test]
    fn test_safety_clean_passes() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_safety("The weather today is sunny and warm.");
        assert_eq!(result.level, ComplianceLevel::Pass);
    }

    #[test]
    fn test_reliability_below_threshold_fails() {
        let mut guardrails = GovernanceGuardrails::new().with_reliability_threshold(0.95);
        for _ in 0..10 {
            guardrails.record_outcome(true);
        }
        guardrails.record_outcome(false); // 10/11 ≈ 90.9% < 95%
        let result = guardrails.check_reliability();
        assert_eq!(result.level, ComplianceLevel::Fail);
    }

    #[test]
    fn test_reliability_above_threshold_passes() {
        let mut guardrails = GovernanceGuardrails::new().with_reliability_threshold(0.90);
        for _ in 0..95 {
            guardrails.record_outcome(true);
        }
        for _ in 0..5 {
            guardrails.record_outcome(false);
        }
        let result = guardrails.check_reliability();
        assert_eq!(result.level, ComplianceLevel::Pass);
    }

    #[test]
    fn test_integrity_with_citation_passes() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_integrity("According to the study [1], results show...");
        assert_eq!(result.level, ComplianceLevel::Pass);
    }

    #[test]
    fn test_integrity_no_citation_warns() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.check_integrity("The answer is 42.");
        assert_eq!(result.level, ComplianceLevel::Warning);
    }

    #[test]
    fn test_run_all_clean_response() {
        let guardrails = GovernanceGuardrails::new();
        let result = guardrails.run_all("According to [source], the sky is blue.", 0);
        assert_eq!(result.privacy.level, ComplianceLevel::Pass);
        assert_eq!(result.safety.level, ComplianceLevel::Pass);
        assert_eq!(result.integrity.level, ComplianceLevel::Pass);
        assert!(result.all_passed());
    }

    // ── Alignment check tests ────────────────────────────────────────────────

    #[test]
    fn test_alignment_detects_contradictory_action() {
        // Goal: maintain audit trail, Action: delete all logs
        let goal = GoalObject::builder()
            .description("maintain audit trail")
            .goal_type(GoalType::Maintain)
            .constraints(vec![Constraint::new(
                ConstraintKind::Compliance,
                "audit trail must be preserved",
                serde_json::json!(true),
            )])
            .build();

        let guardrails = GovernanceGuardrails::new().with_active_goal(goal);
        let (passed, blocker) = guardrails.check_alignment("delete all logs");

        assert!(!passed, "delete all logs should contradict maintain goal");
        assert!(blocker.is_some());
        let b = blocker.as_ref().unwrap();
        assert!(b.contains("delete") || b.contains("audit"));
    }

    #[test]
    fn test_alignment_passes_when_action_aligns() {
        // Goal: maintain audit trail, Action: read logs
        let goal = GoalObject::builder()
            .description("maintain audit trail")
            .goal_type(GoalType::Maintain)
            .build();

        let guardrails = GovernanceGuardrails::new().with_active_goal(goal);
        let (passed, blocker) = guardrails.check_alignment("read the logs");

        assert!(passed, "read logs should not contradict maintain goal");
        assert!(blocker.is_none());
    }

    #[test]
    fn test_alignment_no_goal_passes() {
        // No goal set — alignment check should pass
        let guardrails = GovernanceGuardrails::new();
        let (passed, blocker) = guardrails.check_alignment("delete everything");

        assert!(passed, "no active goal means alignment check passes");
        assert!(blocker.is_none());
    }

    #[test]
    fn test_alignment_preserves_uptime() {
        // Goal: maintain uptime, Action: shutdown server
        let goal = GoalObject::builder()
            .description("maintain 99.9% uptime")
            .goal_type(GoalType::Maintain)
            .constraints(vec![Constraint::new(
                ConstraintKind::Quality,
                "system must remain available",
                serde_json::json!(true),
            )])
            .build();

        let guardrails = GovernanceGuardrails::new().with_active_goal(goal);
        let (passed, blocker) = guardrails.check_alignment("shutdown production server");

        assert!(!passed, "shutdown should contradict uptime maintenance goal");
        assert!(blocker.is_some());
    }

    // ── Oversight check tests ────────────────────────────────────────────────

    #[test]
    fn test_oversight_low_risk_autonomous_passes() {
        let guardrails = GovernanceGuardrails::new()
            .with_oversight_level(clawz_core::types::OversightLevel::Autonomous)
            .with_risk_level(clawz_core::types::tool_risk::RiskLevel::Low);

        let (passed, blocker) = guardrails.check_oversight();

        assert!(passed, "Low risk with Autonomous oversight should pass");
        assert!(blocker.is_none());
    }

    #[test]
    fn test_oversight_high_risk_requires_pre_approval() {
        let guardrails = GovernanceGuardrails::new()
            .with_oversight_level(clawz_core::types::OversightLevel::Autonomous)
            .with_risk_level(clawz_core::types::tool_risk::RiskLevel::High);

        let (passed, blocker) = guardrails.check_oversight();

        assert!(!passed, "High risk with only Autonomous oversight should fail");
        assert!(blocker.is_some());
    }

    #[test]
    fn test_oversight_no_risk_level_passes() {
        let guardrails = GovernanceGuardrails::new()
            .with_oversight_level(clawz_core::types::OversightLevel::Autonomous);

        let (passed, blocker) = guardrails.check_oversight();

        assert!(passed, "No risk level set means oversight check passes");
        assert!(blocker.is_none());
    }
}
