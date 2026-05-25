//! Reality dimension types for the PRISM-G framework.
//!
//! The Reality dimension models the operational environment: systems,
//! constraints, live state, and organisational structure. These types are
//! consumed by the worker's `reality` module to build [`ContextBundle`]s,
//! detect drift, and version reality snapshots over time.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── System inventory ──────────────────────────────────────────────────────────

/// Description of a registered tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub version: String,
}

/// Description of an API endpoint and its health-check URL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiDescriptor {
    pub endpoint: String,
    pub health: String,
}

/// A resource limit with current utilisation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResourceLimit {
    pub resource: String,
    pub max_value: u64,
    pub used: u64,
}

/// Snapshot of discoverable system resources.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemInventory {
    pub tools: Vec<ToolDescriptor>,
    pub apis: Vec<ApiDescriptor>,
    pub limits: Vec<ResourceLimit>,
}

// ── Permissions & organisation ────────────────────────────────────────────────

/// A single permission entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionEntry {
    pub who: String,
    pub action: String,
    pub resource: String,
    pub allow: bool,
}

/// Full permission matrix for a tenant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionMatrix {
    pub entries: Vec<PermissionEntry>,
}

/// A named team with member identifiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    pub members: Vec<String>,
}

/// An escalation path between two entities under a condition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EscalationPath {
    pub from: String,
    pub to: String,
    pub condition: String,
}

/// Organisational structure: teams and escalation paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrgStructure {
    pub teams: Vec<Team>,
    pub escalation_paths: Vec<EscalationPath>,
}

// ── Constraints & state ───────────────────────────────────────────────────────

/// Active constraints currently in force.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActiveConstraints {
    pub policies: Vec<String>,
    pub slas: Vec<String>,
    pub budgets: Vec<String>,
    pub regulations: Vec<String>,
}

/// Current operational state captured as free-form summaries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CurrentState {
    pub system_status: String,
    pub project_progress: String,
    pub resource_consumption: String,
}

/// A historical pattern with observed frequency.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoricalPattern {
    pub pattern: String,
    pub outcome: String,
    pub frequency: f32,
}

// ── Reliability tier ──────────────────────────────────────────────────────────

/// Confidence tier assigned to a [`ContextBundle`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reliability {
    High,
    Medium,
    Low,
}

// ── Context bundle ────────────────────────────────────────────────────────────

/// A versioned, tenant-scoped snapshot of reality.
///
/// `ContextBundle` is the central type of the Reality dimension. It is
/// produced by [`RealityModel::build`](crate::reality::RealityModel) and
/// consumed by drift detectors, governance checks, and runtime planning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextBundle {
    pub bundle_id: Uuid,
    pub version: u32,
    pub tenant_id: String,
    pub system_inventory: SystemInventory,
    pub permission_matrix: PermissionMatrix,
    pub org_structure: OrgStructure,
    pub active_constraints: ActiveConstraints,
    pub current_state: CurrentState,
    pub historical_patterns: Vec<HistoricalPattern>,
    pub captured_at: DateTime<Utc>,
    pub confidence: f32,
    pub reliability: Reliability,
}

impl ContextBundle {
    /// Start building a [`ContextBundle`] with sensible defaults.
    ///
    /// Defaults:
    /// - `bundle_id` = random UUID v4
    /// - `version` = 1
    /// - `tenant_id` = `"default"`
    /// - `confidence` = 1.0
    /// - `reliability` = [`Reliability::High`]
    /// - all collections empty
    /// - `captured_at` = now (UTC)
    pub fn builder() -> ContextBundleBuilder {
        ContextBundleBuilder::default()
    }
}

// ── Builder ───────────────────────────────────────────────────────────────────

/// Builder for [`ContextBundle`].
#[derive(Debug, Clone)]
pub struct ContextBundleBuilder {
    bundle_id: Uuid,
    version: u32,
    tenant_id: String,
    system_inventory: SystemInventory,
    permission_matrix: PermissionMatrix,
    org_structure: OrgStructure,
    active_constraints: ActiveConstraints,
    current_state: CurrentState,
    historical_patterns: Vec<HistoricalPattern>,
    captured_at: DateTime<Utc>,
    confidence: f32,
    reliability: Reliability,
}

impl Default for ContextBundleBuilder {
    fn default() -> Self {
        Self {
            bundle_id: Uuid::new_v4(),
            version: 1,
            tenant_id: "default".into(),
            system_inventory: SystemInventory {
                tools: Vec::new(),
                apis: Vec::new(),
                limits: Vec::new(),
            },
            permission_matrix: PermissionMatrix {
                entries: Vec::new(),
            },
            org_structure: OrgStructure {
                teams: Vec::new(),
                escalation_paths: Vec::new(),
            },
            active_constraints: ActiveConstraints {
                policies: Vec::new(),
                slas: Vec::new(),
                budgets: Vec::new(),
                regulations: Vec::new(),
            },
            current_state: CurrentState {
                system_status: String::new(),
                project_progress: String::new(),
                resource_consumption: String::new(),
            },
            historical_patterns: Vec::new(),
            captured_at: Utc::now(),
            confidence: 1.0,
            reliability: Reliability::High,
        }
    }
}

impl ContextBundleBuilder {
    pub fn bundle_id(mut self, id: Uuid) -> Self {
        self.bundle_id = id;
        self
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = tenant_id.into();
        self
    }

    pub fn system_inventory(mut self, inventory: SystemInventory) -> Self {
        self.system_inventory = inventory;
        self
    }

    pub fn permission_matrix(mut self, matrix: PermissionMatrix) -> Self {
        self.permission_matrix = matrix;
        self
    }

    pub fn org_structure(mut self, structure: OrgStructure) -> Self {
        self.org_structure = structure;
        self
    }

    pub fn active_constraints(mut self, constraints: ActiveConstraints) -> Self {
        self.active_constraints = constraints;
        self
    }

    pub fn current_state(mut self, state: CurrentState) -> Self {
        self.current_state = state;
        self
    }

    pub fn historical_patterns(mut self, patterns: Vec<HistoricalPattern>) -> Self {
        self.historical_patterns = patterns;
        self
    }

    pub fn captured_at(mut self, at: DateTime<Utc>) -> Self {
        self.captured_at = at;
        self
    }

    pub fn confidence(mut self, confidence: f32) -> Self {
        self.confidence = confidence;
        self
    }

    pub fn reliability(mut self, reliability: Reliability) -> Self {
        self.reliability = reliability;
        self
    }

    pub fn build(self) -> ContextBundle {
        ContextBundle {
            bundle_id: self.bundle_id,
            version: self.version,
            tenant_id: self.tenant_id,
            system_inventory: self.system_inventory,
            permission_matrix: self.permission_matrix,
            org_structure: self.org_structure,
            active_constraints: self.active_constraints,
            current_state: self.current_state,
            historical_patterns: self.historical_patterns,
            captured_at: self.captured_at,
            confidence: self.confidence,
            reliability: self.reliability,
        }
    }
}

// ── Delta & versioning ────────────────────────────────────────────────────────

/// Describes changes between two versions of a [`ContextBundle`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextDelta {
    pub from_version: u32,
    pub to_version: u32,
    pub changes: Vec<String>,
}

// ── Drift ─────────────────────────────────────────────────────────────────────

/// Kind of drift detected between predicted and observed reality.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftKind {
    PredictionError,
    Anomaly,
    ChangeNotification,
    PeriodicValidation,
    ExecutionFailure,
}

/// A single drift signal emitted by the [`DriftDetector`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftSignal {
    pub kind: DriftKind,
    pub detail: String,
}

// ── Conflict resolution ───────────────────────────────────────────────────────

/// Record of a resolved merge conflict during context updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictMarker {
    pub key: String,
    pub old_value: String,
    pub new_value: String,
    pub resolved_at: DateTime<Utc>,
}
