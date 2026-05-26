//! Compliance evidence export — maps audit logs to compliance frameworks.
//!
//! This module transforms the raw audit trail produced by the worker's
//! governance layer into structured, framework-specific compliance bundles
//! that can be handed to auditors, regulators, or internal risk teams.
//!
//! Supported frameworks:
//!
//! | Framework | Use-case |
//! |-----------|----------|
//! | **SOC2** | Demonstrates logical-access, monitoring, change-management and risk-mitigation controls to an AICPA auditor. |
//! | **GDPR** | Provides evidence of lawful processing (Art.5), privacy-by-design (Art.25), processing records (Art.30) and security-of-processing (Art.32) for a European DPA. |
//! | **EU AI Act** | Maps audit entries to the high-risk AI-system obligations in the EU AI Act, including risk-management (Art.9), record-keeping (Art.12), transparency (Art.13), human-oversight (Art.14) and accuracy/robustness (Art.15). |
//!
//! The export is **read-only**: it queries the [`AuditLogger`] and builds an
//! immutable [`ComplianceBundle`] that contains [`ControlMapping`]s, each
//! linking a specific framework control to the slice of the audit trail that
//! satisfies it.

use chrono::{DateTime, Utc};
use clawz_core::error::{ClawzError, Result};
use serde::{Deserialize, Serialize};

use super::audit::{AuditEntry, AuditFilter, AuditLogger, AuditResult};

// ── Framework ─────────────────────────────────────────────────────────────────

/// Identifies the compliance regime that an export targets.
///
/// Each variant serialises as snake_case JSON (e.g. `"soc2"`, `"gdpr"`,
/// `"eu_ai_act"`) while the [`Display`] implementation produces the human
/// label used inside a [`ComplianceBundle`] (e.g. `"SOC2"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComplianceFramework {
    /// AICPA Trust Services Criteria (security, availability, confidentiality).
    Soc2,
    /// EU General Data Protection Regulation.
    Gdpr,
    /// EU Artificial Intelligence Act (high-risk AI system obligations).
    EuAiAct,
}

impl std::fmt::Display for ComplianceFramework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComplianceFramework::Soc2 => write!(f, "SOC2"),
            ComplianceFramework::Gdpr => write!(f, "GDPR"),
            ComplianceFramework::EuAiAct => write!(f, "EU AI Act"),
        }
    }
}

// ── ControlMapping ────────────────────────────────────────────────────────────

/// A single framework control mapped to the audit evidence that supports it.
///
/// A [`ControlMapping`] is the atomic unit inside a [`ComplianceBundle`].
/// It carries the formal control identifier (e.g. `"CC6.1"` or `"Art.5"`),
/// a human-readable name and description, the list of [`AuditEntry`] values
/// that serve as evidence, and a computed [`ControlStatus`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMapping {
    /// Framework-specific control identifier.
    ///
    /// Examples: `"CC6.1"` (SOC2 logical access), `"Art.5"` (GDPR data
    /// processing principles), `"Art.9"` (EU AI Act risk management).
    pub control_id: String,

    /// Short human-readable name for the control.
    ///
    /// Used in reports and UI summaries so auditors do not need to memorise
    /// every control code.
    pub control_name: String,

    /// Long-form explanation of what the control checks.
    ///
    /// Helps reviewers understand *why* the attached evidence satisfies the
    /// control without reading the raw framework text.
    pub description: String,

    /// Audit entries collected from the [`AuditLogger`] that constitute
    /// evidence for this control.
    ///
    /// The entries are filtered from the larger audit trail using
    /// framework-specific heuristics (action keywords, result codes, etc.).
    pub evidence: Vec<AuditEntry>,

    /// Derived status: [`ControlStatus::Satisfied`] if at least one evidence
    /// entry exists, otherwise [`ControlStatus::NotSatisfied`].
    ///
    /// Note: the current implementation never produces
    /// [`ControlStatus::Partial`]; the variant exists for future scoring
    /// heuristics.
    pub status: ControlStatus,
}

/// Satisfaction level of a mapped compliance control.
///
/// Serialises as lowercase JSON (`"satisfied"`, `"partial"`,
/// `"not_satisfied"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ControlStatus {
    /// At least one evidence entry was found for the control.
    Satisfied,
    /// Partial evidence (reserved for future scoring heuristics).
    Partial,
    /// No evidence entries matched the control filter.
    NotSatisfied,
}

impl ControlMapping {
    /// Build a new mapping, deriving [`ControlStatus`] from the evidence count.
    fn new(
        control_id: impl Into<String>,
        control_name: impl Into<String>,
        description: impl Into<String>,
        evidence: Vec<AuditEntry>,
    ) -> Self {
        let status = if evidence.is_empty() {
            ControlStatus::NotSatisfied
        } else {
            ControlStatus::Satisfied
        };
        Self {
            control_id: control_id.into(),
            control_name: control_name.into(),
            description: description.into(),
            evidence,
            status,
        }
    }
}

// ── DateRange ─────────────────────────────────────────────────────────────────

/// Inclusive UTC date range used to bound compliance queries.
///
/// A [`DateRange`] is passed to [`ComplianceExporter`] so that the
/// resulting bundle only contains evidence from the reporting window under
/// review.
#[derive(Debug, Clone, Copy)]
pub struct DateRange {
    /// Start of the range (inclusive).
    pub from: DateTime<Utc>,
    /// End of the range (inclusive).
    pub to: DateTime<Utc>,
}

impl DateRange {
    /// Create a [`DateRange`] from two UTC timestamps.
    pub fn new(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Self { from, to }
    }

    /// Convenience constructor for the last 30 calendar days.
    ///
    /// `from = now - 30 days`, `to = now`.
    pub fn last_30_days() -> Self {
        let to = Utc::now();
        let from = to - chrono::Duration::days(30);
        Self { from, to }
    }

    /// Convenience constructor for the last 90 calendar days.
    ///
    /// `from = now - 90 days`, `to = now`.
    pub fn last_90_days() -> Self {
        let to = Utc::now();
        let from = to - chrono::Duration::days(90);
        Self { from, to }
    }
}

// ── ComplianceBundle ──────────────────────────────────────────────────────────

/// Complete, serialisable compliance report for a single framework and
/// time-window.
///
/// The bundle is the top-level output of [`ComplianceExporter`].  It is
/// designed to be:
///
/// * **Self-contained** — every field required by an auditor is present.
/// * **Immutable after creation** — the constructor computes the summary
///   counts so the numbers cannot drift from the underlying evidence.
/// * **JSON-friendly** — serde derives allow direct serialisation to file,
///   S3, or an HTTP response.
///
/// # Structure
///
/// ```text
/// ComplianceBundle
/// ├── framework            "SOC2" | "GDPR" | "EU AI Act"
/// ├── generated_at         UTC timestamp when the bundle was produced
/// ├── date_range_from/to   The query window
/// ├── controls[]           Vec<ControlMapping> — one per framework control
/// └── summary              ComplianceSummary — aggregate tallies
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceBundle {
    /// Human-readable framework label (from [`ComplianceFramework::to_string()`]).
    pub framework: String,

    /// Timestamp when the bundle was generated.
    ///
    /// Captured at construction time via [`Utc::now()`].
    pub generated_at: DateTime<Utc>,

    /// Lower bound of the evidence query window.
    pub date_range_from: DateTime<Utc>,

    /// Upper bound of the evidence query window.
    pub date_range_to: DateTime<Utc>,

    /// Per-control mappings containing the filtered evidence.
    ///
    /// The order mirrors the order in which the framework controls are
    /// defined in the exporter (e.g. SOC2 CC6.1, CC6.6, CC7.2, …).
    pub controls: Vec<ControlMapping>,

    /// Pre-computed aggregate statistics over the controls vector.
    pub summary: ComplianceSummary,
}

/// Aggregate tallies derived from the [`controls`](ComplianceBundle::controls)
/// vector.
///
/// These numbers are calculated once by [`ComplianceBundle::build`] and are
/// guaranteed to be consistent with the underlying data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceSummary {
    /// Total number of controls mapped for this framework.
    pub total_controls: usize,
    /// How many controls are [`ControlStatus::Satisfied`].
    pub satisfied: usize,
    /// How many controls are [`ControlStatus::Partial`].
    pub partial: usize,
    /// How many controls are [`ControlStatus::NotSatisfied`].
    pub not_satisfied: usize,
    /// Sum of `evidence.len()` across *all* controls.
    ///
    /// This is the raw audit-entry count inside the bundle, not the number
    /// of distinct controls.
    pub total_evidence_entries: usize,
}

impl ComplianceBundle {
    /// Build a bundle from the pre-filtered controls and the original query
    /// range.
    ///
    /// Computes [`ComplianceSummary`] automatically.
    fn build(
        framework: ComplianceFramework,
        controls: Vec<ControlMapping>,
        range: DateRange,
    ) -> Self {
        let total_controls = controls.len();
        let satisfied = controls
            .iter()
            .filter(|c| c.status == ControlStatus::Satisfied)
            .count();
        let partial = controls
            .iter()
            .filter(|c| c.status == ControlStatus::Partial)
            .count();
        let not_satisfied = controls
            .iter()
            .filter(|c| c.status == ControlStatus::NotSatisfied)
            .count();
        let total_evidence_entries = controls
            .iter()
            .map(|c| c.evidence.len())
            .sum();

        Self {
            framework: framework.to_string(),
            generated_at: Utc::now(),
            date_range_from: range.from,
            date_range_to: range.to,
            controls,
            summary: ComplianceSummary {
                total_controls,
                satisfied,
                partial,
                not_satisfied,
                total_evidence_entries,
            },
        }
    }
}

// ── ComplianceExporter ────────────────────────────────────────────────────────

/// Entry-point for generating framework-specific compliance bundles.
///
/// [`ComplianceExporter`] holds a borrow on an [`AuditLogger`] and provides
/// methods that query the audit trail, apply framework-specific filters, and
/// return an immutable [`ComplianceBundle`] ready for serialisation or human
/// review.
///
/// # Example
///
/// ```ignore
/// // ComplianceExporter requires a real AuditLogger and type imports
/// // from the compliance module. Run in a real environment.
/// let exporter = ComplianceExporter::new(&audit_logger);
/// let bundle = exporter.export_evidence(ComplianceFramework::Soc2, DateRange::last_30_days());
/// ```
pub struct ComplianceExporter<'a> {
    /// Reference to the [`AuditLogger`] that stores the raw audit trail.
    ///
    /// The exporter only reads from the logger; it never modifies it.
    logger: &'a AuditLogger,
}

impl<'a> ComplianceExporter<'a> {
    /// Create a new exporter backed by the supplied [`AuditLogger`].
    ///
    /// The exporter does not take ownership of the logger; multiple exporters
    /// (or multiple frameworks) can be run against the same logger.
    pub fn new(logger: &'a AuditLogger) -> Self {
        Self { logger }
    }

    /// Export evidence for the given framework and date range.
    ///
    /// This is the primary public API.  It delegates to a private
    /// framework-specific method (`export_soc2`, `export_gdpr`,
    /// `export_eu_ai_act`) that applies the relevant control filters,
    /// builds [`ControlMapping`]s, and wraps them in a [`ComplianceBundle`].
    ///
    /// # Arguments
    ///
    /// * `framework` — The compliance regime to target.
    /// * `range` — Inclusive UTC window over which to query the audit log.
    ///
    /// # Returns
    ///
    /// A fully populated [`ComplianceBundle`] including pre-computed summary
    /// statistics.
    pub fn export_evidence(
        &self,
        framework: ComplianceFramework,
        range: DateRange,
    ) -> ComplianceBundle {
        match framework {
            ComplianceFramework::Soc2 => self.export_soc2(range),
            ComplianceFramework::Gdpr => self.export_gdpr(range),
            ComplianceFramework::EuAiAct => self.export_eu_ai_act(range),
        }
    }

    /// Query the audit logger for every entry that falls inside `range`.
    ///
    /// Helper used by all three framework-specific export methods.
    fn entries_in_range(&self, range: DateRange) -> Vec<AuditEntry> {
        let filter = AuditFilter::new().from(range.from).to(range.to);
        self.logger.get_entries(&filter)
    }

    // ── SOC2 ─────────────────────────────────────────────────────────────────

    /// Build a SOC2 Type II evidence bundle.
    ///
    /// Maps the audit trail to five common Trust Services Criteria controls:
    ///
    /// | Control | Criterion | Filter heuristic |
    /// |---------|-----------|------------------|
    /// | CC6.1 | Logical access | Actions containing `"auth"`, `"login"` or `"access"` |
    /// | CC6.6 | Policy enforcement | Actions containing `"governance"` or `"policy"` |
    /// | CC7.2 | System monitoring | *All* entries in the range (full trail) |
    /// | CC8.1 | Change management | Actions containing `"deploy"` or `"update"` |
    /// | CC9.2 | Risk mitigation | Entries whose result is [`AuditResult::Deny`] |
    fn export_soc2(&self, range: DateRange) -> ComplianceBundle {
        let all = self.entries_in_range(range);

        // CC6.1 — Logical and physical access
        let access_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| {
                e.action.contains("auth")
                    || e.action.contains("login")
                    || e.action.contains("access")
            })
            .cloned()
            .collect();

        // CC6.6 — Logical access to information assets
        let governance_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.action.contains("governance") || e.action.contains("policy"))
            .cloned()
            .collect();

        // CC7.2 — Monitoring
        let monitor_entries: Vec<AuditEntry> = all.clone();

        // CC8.1 — Change management
        let change_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.action.contains("deploy") || e.action.contains("update"))
            .cloned()
            .collect();

        // CC9.2 — Risk mitigation — governance denies
        let deny_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Deny)
            .cloned()
            .collect();

        let controls = vec![
            ControlMapping::new(
                "CC6.1",
                "Logical Access",
                "Controls over logical access to systems",
                access_entries,
            ),
            ControlMapping::new(
                "CC6.6",
                "Policy Enforcement",
                "Governance policy decisions logged",
                governance_entries,
            ),
            ControlMapping::new(
                "CC7.2",
                "System Monitoring",
                "Comprehensive audit trail maintained",
                monitor_entries,
            ),
            ControlMapping::new(
                "CC8.1",
                "Change Management",
                "Deployment and update operations logged",
                change_entries,
            ),
            ControlMapping::new(
                "CC9.2",
                "Risk Mitigation",
                "Denied actions logged for risk evidence",
                deny_entries,
            ),
        ];

        ComplianceBundle::build(ComplianceFramework::Soc2, controls, range)
    }

    // ── GDPR ──────────────────────────────────────────────────────────────────

    /// Build a GDPR evidence bundle.
    ///
    /// Maps the audit trail to four key articles of the Regulation:
    ///
    /// | Article | Topic | Filter heuristic |
    /// |---------|-------|------------------|
    /// | Art.5 | Data-processing principles | Actions containing `"data"` or `"pii"` |
    /// | Art.25 | Data protection by design | Actions containing `"privacy"` |
    /// | Art.30 | Records of processing | *All* entries in the range (complete trail) |
    /// | Art.32 | Security of processing | Entries whose result is [`AuditResult::Deny`] |
    fn export_gdpr(&self, range: DateRange) -> ComplianceBundle {
        let all = self.entries_in_range(range);

        // Art. 5 — Data processing principles
        let data_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.action.contains("data") || e.action.contains("pii"))
            .cloned()
            .collect();

        // Art. 25 — Data protection by design
        let privacy_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.action.contains("privacy"))
            .cloned()
            .collect();

        // Art. 30 — Records of processing activities
        let all_entries = all.clone();

        // Art. 32 — Security of processing
        let deny_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Deny)
            .cloned()
            .collect();

        let controls = vec![
            ControlMapping::new(
                "Art.5",
                "Data Processing Principles",
                "Evidence of lawful data processing",
                data_entries,
            ),
            ControlMapping::new(
                "Art.25",
                "Data Protection by Design",
                "Privacy checks applied to outputs",
                privacy_entries,
            ),
            ControlMapping::new(
                "Art.30",
                "Records of Processing",
                "Complete audit trail of all agent actions",
                all_entries,
            ),
            ControlMapping::new(
                "Art.32",
                "Security of Processing",
                "Access denials logged as security evidence",
                deny_entries,
            ),
        ];

        ComplianceBundle::build(ComplianceFramework::Gdpr, controls, range)
    }

    // ── EU AI Act ─────────────────────────────────────────────────────────────

    /// Build an EU AI Act evidence bundle.
    ///
    /// Maps the audit trail to five high-risk AI-system obligations:
    ///
    /// | Article | Topic | Filter heuristic |
    /// |---------|-------|------------------|
    /// | Art.9 | Risk management | Entries with result [`AuditResult::Deny`] or [`AuditResult::Review`] |
    /// | Art.12 | Record-keeping | *All* entries in the range (complete trail) |
    /// | Art.13 | Transparency | Entries with result [`AuditResult::Allow`] |
    /// | Art.14 | Human oversight | Entries with result [`AuditResult::Review`] |
    /// | Art.15 | Accuracy / robustness | Entries with result [`AuditResult::Error`] |
    fn export_eu_ai_act(&self, range: DateRange) -> ComplianceBundle {
        let all = self.entries_in_range(range);

        // Art. 9 — Risk management
        let risk_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Deny || e.result == AuditResult::Review)
            .cloned()
            .collect();

        // Art. 12 — Record-keeping
        let record_entries = all.clone();

        // Art. 13 — Transparency
        let transparent_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Allow)
            .cloned()
            .collect();

        // Art. 14 — Human oversight
        let review_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Review)
            .cloned()
            .collect();

        // Art. 15 — Accuracy, robustness
        let error_entries: Vec<AuditEntry> = all
            .iter()
            .filter(|e| e.result == AuditResult::Error)
            .cloned()
            .collect();

        let controls = vec![
            ControlMapping::new(
                "Art.9",
                "Risk Management",
                "Risk management system — denied/reviewed actions",
                risk_entries,
            ),
            ControlMapping::new(
                "Art.12",
                "Record-keeping",
                "Audit trail of all AI system actions",
                record_entries,
            ),
            ControlMapping::new(
                "Art.13",
                "Transparency",
                "Allowed actions transparently logged",
                transparent_entries,
            ),
            ControlMapping::new(
                "Art.14",
                "Human Oversight",
                "Actions queued for human review",
                review_entries,
            ),
            ControlMapping::new(
                "Art.15",
                "Accuracy and Robustness",
                "Errors logged for reliability monitoring",
                error_entries,
            ),
        ];

        ComplianceBundle::build(ComplianceFramework::EuAiAct, controls, range)
    }

    /// Export a [`ComplianceBundle`] as a pretty-printed JSON string.
    ///
    /// This is a convenience wrapper around [`export_evidence`] +
    /// `serde_json::to_string_pretty`.  Use it when you need a ready-to-save
    /// or ready-to-transmit JSON payload without handling the intermediate
    /// [`ComplianceBundle`] yourself.
    ///
    /// # Errors
    ///
    /// Returns [`ClawzError::Serialization`] if the bundle cannot be
    /// serialised to JSON (should be rare in practice).
    pub fn export_json(
        &self,
        framework: ComplianceFramework,
        range: DateRange,
    ) -> Result<String> {
        let bundle = self.export_evidence(framework, range);
        serde_json::to_string_pretty(&bundle)
            .map_err(|e| ClawzError::Serialization(e.to_string()))
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_logger() -> AuditLogger {
        let logger = AuditLogger::new();
        logger.append("a1", "chat", AuditResult::Allow, serde_json::json!({}));
        logger.append("a2", "deploy", AuditResult::Deny, serde_json::json!({}));
        logger.append("a3", "governance_check", AuditResult::Review, serde_json::json!({}));
        logger
    }

    #[test]
    fn test_soc2_export() {
        let logger = populated_logger();
        let exporter = ComplianceExporter::new(&logger);
        let bundle = exporter.export_evidence(ComplianceFramework::Soc2, DateRange::last_30_days());
        assert_eq!(bundle.framework, "SOC2");
        assert!(!bundle.controls.is_empty());
        // CC7.2 (monitoring) should have all 3 entries.
        let cc7 = bundle.controls.iter().find(|c| c.control_id == "CC7.2").unwrap();
        assert_eq!(cc7.evidence.len(), 3);
    }

    #[test]
    fn test_gdpr_export() {
        let logger = populated_logger();
        let exporter = ComplianceExporter::new(&logger);
        let bundle = exporter.export_evidence(ComplianceFramework::Gdpr, DateRange::last_30_days());
        assert_eq!(bundle.framework, "GDPR");
    }

    #[test]
    fn test_eu_ai_act_export() {
        let logger = populated_logger();
        let exporter = ComplianceExporter::new(&logger);
        let bundle = exporter.export_evidence(ComplianceFramework::EuAiAct, DateRange::last_30_days());
        assert_eq!(bundle.framework, "EU AI Act");
        // Art.12 should have all entries.
        let art12 = bundle.controls.iter().find(|c| c.control_id == "Art.12").unwrap();
        assert_eq!(art12.evidence.len(), 3);
    }

    #[test]
    fn test_empty_logger_not_satisfied() {
        let logger = AuditLogger::new();
        let exporter = ComplianceExporter::new(&logger);
        let bundle = exporter.export_evidence(ComplianceFramework::Soc2, DateRange::last_30_days());
        // CC6.1 (access) should have no evidence.
        let cc6 = bundle.controls.iter().find(|c| c.control_id == "CC6.1").unwrap();
        assert_eq!(cc6.status, ControlStatus::NotSatisfied);
    }

    #[test]
    fn test_export_json_valid() {
        let logger = populated_logger();
        let exporter = ComplianceExporter::new(&logger);
        let json = exporter
            .export_json(ComplianceFramework::Gdpr, DateRange::last_30_days())
            .unwrap();
        assert!(json.contains("GDPR"));
    }

    #[test]
    fn test_summary_counts() {
        let logger = populated_logger();
        let exporter = ComplianceExporter::new(&logger);
        let bundle = exporter.export_evidence(ComplianceFramework::Soc2, DateRange::last_30_days());
        assert_eq!(
            bundle.summary.total_controls,
            bundle.controls.len()
        );
        assert_eq!(
            bundle.summary.satisfied + bundle.summary.partial + bundle.summary.not_satisfied,
            bundle.summary.total_controls
        );
    }
}
