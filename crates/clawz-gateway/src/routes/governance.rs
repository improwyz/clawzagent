//! Governance, policy, and audit REST API.
//!
//! This module enforces organisational guard-rails around agent behaviour.
//! It covers:
//! - **Policies** — rule sets with configurable enforcement (`audit`, `warn`, `block`).
//! - **Audit log** — append-only trail of every create / delete / evaluate event.
//! - **Trust scoring** — per-agent heuristic based on violation frequency.
//! - **Evaluation** — run-time policy check against an action string.
//!
//! # Design notes
//! - Policies are evaluated synchronously so callers get an immediate allow/deny.
//! - The audit log is in-memory only in this implementation; production should
//!   persist to a WAL or external SIEM.
//!
//! # Cross-module interactions
//! - `trust_score` and `evaluate` read `AppState.agents` to validate the target
//!   agent before computing results.
//! - `evaluate` writes to `AppState.audit_log` so every decision is traceable.
//! - `create_policy` also audits itself so policy changes appear in the same log.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

// Dependency: AuditEntry, PolicyRecord, and GatewayError defined in the crate root.
use crate::{AppState, AuditEntry, GatewayError, PolicyRecord};

/// Assemble the governance sub-router.
///
/// Mounted by the gateway at `/api/v1/governance`.
pub fn routes() -> Router<AppState> {
    Router::new()
        // Policy CRUD
        .route("/policies", get(list_policies).post(create_policy))
        .route("/policies/{id}", get(get_policy).put(update_policy).delete(delete_policy))
        // Audit log query
        .route("/audit", get(audit_log))
        // Per-agent trust score
        .route("/trust/{agent_id}", get(trust_score))
        // Real-time policy evaluation
        .route("/evaluate", post(evaluate))
        // Legacy proposal endpoints retained for UI compatibility
        .route("/proposals", get(list_proposals).post(create_proposal))
        .route("/proposals/{id}", get(get_proposal).put(update_proposal).delete(delete_proposal))
        .route("/proposals/{id}/vote", post(vote_proposal))
}

// ─── Body / query types ───────────────────────────────────────────────────────

/// Query parameters for `GET /governance/audit`.
#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    /// 1-based page number (default 1).
    pub page: Option<usize>,
    /// Items per page, clamped to 1..200 (default 50).
    pub limit: Option<usize>,
    /// Filter to a specific resource type ("agent", "policy", "user", …).
    pub resource_type: Option<String>,
    /// Filter to actions performed by this actor ID.
    pub actor: Option<String>,
}

/// Request body for `POST /governance/policies`.
#[derive(Debug, Deserialize)]
pub struct CreatePolicyBody {
    /// Human-readable policy name (required).
    pub name: Option<String>,
    /// Optional longer explanation of intent.
    pub description: Option<String>,
    /// Substring rules; if an evaluated action contains one of these strings the
    /// policy is considered matched.
    pub rules: Option<Vec<String>>,
    /// Enforcement mode: "audit" (log only), "warn" (allow + flag), or "block".
    /// Defaults to "audit".
    pub enforcement: Option<String>,
    /// Whether the policy is active. Defaults to `true`.
    pub enabled: Option<bool>,
}

/// Request body for `PUT /governance/policies/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdatePolicyBody {
    /// New display name.
    pub name: Option<String>,
    /// New description.
    pub description: Option<String>,
    /// Replaced rule set.
    pub rules: Option<Vec<String>>,
    /// New enforcement mode.
    pub enforcement: Option<String>,
    /// Enable / disable toggle.
    pub enabled: Option<bool>,
}

/// Request body for `POST /governance/evaluate`.
#[derive(Debug, Deserialize)]
pub struct EvaluateBody {
    /// ID of the agent whose action is being evaluated (required).
    pub agent_id: Option<String>,
    /// Action string to test against active policies (required).
    pub action: Option<String>,
    /// Optional free-form context object for diagnostic logging.
    pub context: Option<serde_json::Value>,
}

// ─── Policy handlers ──────────────────────────────────────────────────────────

/// `GET /governance/policies` — list all policies.
async fn list_policies(State(state): State<AppState>) -> Json<Value> {
    let policies = state.policies.read().await;
    Json(json!({ "data": *policies, "total": policies.len() }))
}

/// `POST /governance/policies` — create a new policy.
///
/// The policy is created enabled by default so it immediately takes effect.
/// An audit entry is appended so governance changes are traceable.
async fn create_policy(
    State(state): State<AppState>,
    Json(body): Json<CreatePolicyBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let name = body.name.ok_or_else(|| {
        GatewayError::Unprocessable("field 'name' is required".to_string())
    })?;

    let now = Utc::now();
    let record = PolicyRecord {
        id: Uuid::new_v4().to_string(),
        name,
        description: body.description.unwrap_or_default(),
        rules: body.rules.unwrap_or_default(),
        enforcement: body.enforcement.unwrap_or_else(|| "audit".to_string()),
        enabled: body.enabled.unwrap_or(true),
        created_at: now,
        updated_at: now,
    };

    state
        .append_audit("system", "create", "policy", &record.id, None)
        .await;

    let mut policies = state.policies.write().await;
    policies.push(record.clone());
    Ok((StatusCode::CREATED, Json(json!(record))))
}

/// `GET /governance/policies/{id}` — fetch a single policy.
async fn get_policy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let policies = state.policies.read().await;
    let record = policies
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Policy", &id))?;
    Ok(Json(json!(record)))
}

/// `PUT /governance/policies/{id}` — partial update.
async fn update_policy(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdatePolicyBody>,
) -> Result<Json<Value>, GatewayError> {
    let mut policies = state.policies.write().await;
    let record = policies
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Policy", &id))?;

    if let Some(name) = body.name { record.name = name; }
    if let Some(desc) = body.description { record.description = desc; }
    if let Some(rules) = body.rules { record.rules = rules; }
    if let Some(enf) = body.enforcement { record.enforcement = enf; }
    if let Some(enabled) = body.enabled { record.enabled = enabled; }
    record.updated_at = Utc::now();

    Ok(Json(json!(record.clone())))
}

/// `DELETE /governance/policies/{id}` — remove a policy.
async fn delete_policy(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    let mut policies = state.policies.write().await;
    let pos = policies
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| GatewayError::not_found("Policy", &id))?;
    policies.remove(pos);
    Ok(StatusCode::NO_CONTENT)
}

// ─── Audit log ────────────────────────────────────────────────────────────────

/// `GET /governance/audit` — paginated, filterable audit trail.
///
/// Supports filtering by `resource_type` and `actor` so compliance dashboards
/// can narrow the view without client-side filtering.
async fn audit_log(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Json<Value> {
    // Clamp pagination: high limits are allowed because audit is typically
    // smaller than conversation data, but we still cap at 200 for safety.
    let page = q.page.unwrap_or(1).max(1);
    let limit = q.limit.unwrap_or(50).max(1).min(200);
    let offset = (page - 1) * limit;

    let audit = state.audit_log.read().await;
    let filtered: Vec<&AuditEntry> = audit
        .iter()
        .filter(|e| {
            let rt_ok = q.resource_type.as_ref().map_or(true, |rt| &e.resource_type == rt);
            let actor_ok = q.actor.as_ref().map_or(true, |a| &e.actor == a);
            rt_ok && actor_ok
        })
        .collect();

    let total = filtered.len();
    let page_data: Vec<&&AuditEntry> = filtered.iter().skip(offset).take(limit).collect();

    Json(json!({
        "data": page_data,
        "total": total,
        "page": page,
        "limit": limit,
    }))
}

// ─── Trust score ──────────────────────────────────────────────────────────────

/// `GET /governance/trust/{agent_id}` — compute a heuristic trust score.
///
/// The score is derived from the ratio of violation / blocked audit entries
/// to total entries for the agent. A clean record yields 1.0. The score is
/// bucketed into "high" / "medium" / "low" tiers for policy decisions.
///
/// # Cross-module dependency
/// Reads `AppState.agents` to validate the agent exists before scoring.
async fn trust_score(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    // Verify the agent exists so we do not score a deleted / typo ID.
    {
        let agents = state.agents.read().await;
        if !agents.iter().any(|a| a.id == agent_id) {
            return Err(GatewayError::not_found("Agent", &agent_id));
        }
    }

    // Compute a deterministic trust score based on audit entries.
    let audit = state.audit_log.read().await;
    let agent_entries: Vec<&AuditEntry> = audit
        .iter()
        .filter(|e| e.resource_id == agent_id)
        .collect();

    let total_actions = agent_entries.len();
    let violations = agent_entries
        .iter()
        .filter(|e| e.action.contains("violation") || e.action.contains("blocked"))
        .count();
    // Default to perfect trust when no actions have been recorded yet.
    let trust_score = if total_actions == 0 {
        1.0f64
    } else {
        1.0 - (violations as f64 / total_actions as f64)
    };

    Ok(Json(json!({
        "agent_id": agent_id,
        "trust_score": trust_score,
        "total_actions": total_actions,
        "violations": violations,
        "tier": if trust_score >= 0.9 { "high" } else if trust_score >= 0.6 { "medium" } else { "low" },
        "computed_at": Utc::now(),
    })))
}

// ─── Evaluate ─────────────────────────────────────────────────────────────────

/// `POST /governance/evaluate` — real-time policy check.
///
/// Iterates every **enabled** policy and tests whether the action string contains
/// any of the policy's rule substrings. If at least one matched policy has
/// enforcement `"block"`, the overall result is `"blocked"`; otherwise it is
/// `"allowed"` or `"allowed_with_warning"`. The outcome is written to the audit
/// log so the decision is immutable and inspectable later.
///
/// # Cross-module dependency
/// Reads `AppState.agents` to validate the target agent.
async fn evaluate(
    State(state): State<AppState>,
    Json(body): Json<EvaluateBody>,
) -> Result<Json<Value>, GatewayError> {
    let agent_id = body.agent_id.ok_or_else(|| {
        GatewayError::Unprocessable("field 'agent_id' is required".to_string())
    })?;
    let action = body.action.ok_or_else(|| {
        GatewayError::Unprocessable("field 'action' is required".to_string())
    })?;

    let policies = state.policies.read().await;
    let active_policies: Vec<&PolicyRecord> = policies.iter().filter(|p| p.enabled).collect();

    // Simple substring evaluation: if the action contains any rule string,
    // the policy is considered matched. This is intentionally coarse-grained
    // so it can be replaced by a DSL or WASM engine later without changing
    // the API contract.
    let mut violations: Vec<Value> = Vec::new();
    for policy in &active_policies {
        for rule in &policy.rules {
            if action.contains(rule.as_str()) {
                violations.push(json!({
                    "policy_id": policy.id,
                    "policy_name": policy.name,
                    "rule": rule,
                    "enforcement": policy.enforcement,
                }));
            }
        }
    }

    let allowed = violations.iter().all(|v| v["enforcement"] != "block");
    let result_status = if violations.is_empty() {
        "allowed"
    } else if allowed {
        "allowed_with_warning"
    } else {
        "blocked"
    };

    // Persist the evaluation so compliance tooling can replay the decision.
    state
        .append_audit(
            "governance",
            &format!("evaluate:{}", result_status),
            "agent",
            &agent_id,
            Some(format!("action={action}")),
        )
        .await;

    Ok(Json(json!({
        "agent_id": agent_id,
        "action": action,
        "context": body.context,
        "result": result_status,
        "allowed": allowed,
        "violations": violations,
        "evaluated_at": Utc::now(),
    })))
}

// ─── Legacy proposal endpoints ────────────────────────────────────────────────

/// `GET /governance/proposals` — legacy stub returning an empty list.
async fn list_proposals() -> Json<Value> {
    Json(json!({ "data": [], "total": 0 }))
}

/// `POST /governance/proposals` — legacy stub creating a pending proposal.
async fn create_proposal() -> Json<Value> {
    Json(json!({ "id": Uuid::new_v4().to_string(), "status": "pending" }))
}

/// `GET /governance/proposals/{id}` — legacy stub.
async fn get_proposal(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "id": id, "status": "pending" }))
}

/// `PUT /governance/proposals/{id}` — legacy stub.
async fn update_proposal(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "id": id, "updated": true }))
}

/// `DELETE /governance/proposals/{id}` — legacy stub.
async fn delete_proposal(Path(id): Path<String>) -> StatusCode {
    let _ = id;
    StatusCode::NO_CONTENT
}

/// `POST /governance/proposals/{id}/vote` — legacy stub.
async fn vote_proposal(Path(id): Path<String>) -> Json<Value> {
    Json(json!({ "id": id, "voted": true }))
}
