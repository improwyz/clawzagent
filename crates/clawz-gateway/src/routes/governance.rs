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

use clawz_core::types::governance::ApprovalStatus;
use clawz_services::dto::EvaluateGovernanceRequest;
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
    if let Some(ref pool) = state.db {
        let _ = crate::postgres_store::persist_policy(pool, &record).await;
    }
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
    let snapshot = record.clone();
    drop(policies);
    if let Some(ref pool) = state.db {
        let _ = crate::postgres_store::persist_policy(pool, &snapshot).await;
    }
    Ok(Json(json!(snapshot)))
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
    if let Some(ref pool) = state.db {
        crate::postgres_store::delete_policy(pool, &id).await;
    }
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
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = ((page - 1) * limit) as i64;
    let limit_i64 = limit as i64;

    if let Some(ref pool) = state.db {
        if let Ok((data, total)) = crate::postgres_store::query_audit_filtered(
            pool,
            q.resource_type.as_deref(),
            q.actor.as_deref(),
            limit_i64,
            offset,
        )
        .await
        {
            return Json(json!({
                "data": data,
                "total": total,
                "page": page,
                "limit": limit,
            }));
        }
    }

    let audit = state.audit_log.read().await;
    let filtered: Vec<&AuditEntry> = audit
        .iter()
        .filter(|e| {
            let rt_ok = q
                .resource_type
                .as_ref()
                .is_none_or(|rt| &e.resource_type == rt);
            let actor_ok = q.actor.as_ref().is_none_or(|a| &e.actor == a);
            rt_ok && actor_ok
        })
        .collect();

    let total = filtered.len();
    let page_data: Vec<&AuditEntry> = filtered
        .iter()
        .skip(offset as usize)
        .take(limit)
        .copied()
        .collect();

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

    {
        let agents = state.agents.read().await;
        if !agents.iter().any(|a| a.id == agent_id) {
            return Err(GatewayError::not_found("Agent", &agent_id));
        }
    }

    let platform = state
        .platform
        .as_ref()
        .ok_or_else(|| GatewayError::Internal("worker platform not configured".into()))?;

    let context = body.context.clone().unwrap_or(json!({}));
    let gov = platform
        .execution
        .evaluate_governance(EvaluateGovernanceRequest {
            agent_id: agent_id.clone(),
            action: action.clone(),
            context: context.clone(),
        })
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    state
        .append_audit(
            "governance",
            &format!("evaluate:{}", gov.result),
            "agent",
            &agent_id,
            Some(format!("action={action}")),
        )
        .await;

    state.publish_event(
        "governance.evaluate",
        json!({
            "agent_id": agent_id,
            "result": gov.result,
            "allowed": gov.allowed,
        }),
    );

    Ok(Json(json!({
        "agent_id": agent_id,
        "action": action,
        "context": context,
        "result": gov.result,
        "allowed": gov.allowed,
        "violations": gov.violations,
        "detail": gov.detail,
        "evaluated_at": Utc::now(),
    })))
}

// ─── Proposal / approval endpoints ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateProposalBody {
    pub agent_id: Option<String>,
    pub action: Option<String>,
    #[serde(default)]
    pub context: Option<Value>,
    pub required_approvals: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct VoteProposalBody {
    pub approver_id: Option<String>,
    pub decision: Option<String>,
    pub reason: Option<String>,
}

fn proposal_json(req: &clawz_core::types::governance::ApprovalRequest) -> Value {
    json!({
        "id": req.id,
        "agent_id": req.agent_id,
        "action": req.action,
        "context": req.context,
        "status": format!("{:?}", req.status).to_lowercase(),
        "required_approvals": req.required_approvals,
        "granted_approvals": req.granted_approvals,
        "approvers": req.approvers,
        "created_at": req.created_at,
        "expires_at": req.expires_at,
    })
}

async fn list_proposals(State(state): State<AppState>) -> Json<Value> {
    let all = state.approval_workflow.list_all().await;
    let data: Vec<Value> = all.iter().map(proposal_json).collect();
    Json(json!({ "data": data, "total": data.len() }))
}

async fn create_proposal(
    State(state): State<AppState>,
    Json(body): Json<CreateProposalBody>,
) -> Result<(StatusCode, Json<Value>), GatewayError> {
    let agent_id = body.agent_id.ok_or_else(|| {
        GatewayError::Unprocessable("field 'agent_id' is required".to_string())
    })?;
    let action = body.action.ok_or_else(|| {
        GatewayError::Unprocessable("field 'action' is required".to_string())
    })?;
    let required = body.required_approvals.unwrap_or(1);
    let context = body.context.unwrap_or(json!({}));

    let id = state
        .approval_workflow
        .request(agent_id, action, context, required)
        .await;

    let req = state
        .approval_workflow
        .get_request(&id)
        .await
        .ok_or_else(|| GatewayError::Internal("proposal missing after create".into()))?;

    state.publish_event("governance.proposal", json!({ "id": id, "status": "pending" }));

    Ok((StatusCode::CREATED, Json(proposal_json(&req))))
}

async fn get_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, GatewayError> {
    let req = state
        .approval_workflow
        .get_request(&id)
        .await
        .ok_or_else(|| GatewayError::not_found("Proposal", &id))?;
    Ok(Json(proposal_json(&req)))
}

async fn update_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CreateProposalBody>,
) -> Result<Json<Value>, GatewayError> {
    let existing = state
        .approval_workflow
        .get_request(&id)
        .await
        .ok_or_else(|| GatewayError::not_found("Proposal", &id))?;

    if existing.status != ApprovalStatus::Pending {
        return Err(GatewayError::Unprocessable(
            "only pending proposals can be updated".to_string(),
        ));
    }

    let agent_id = body.agent_id.unwrap_or(existing.agent_id);
    let action = body.action.unwrap_or(existing.action);
    let context = body.context.unwrap_or(existing.context);
    let required = body.required_approvals.unwrap_or(existing.required_approvals);

    state.approval_workflow.reject(&id, "system", "superseded by update").await.ok();
    let new_id = state
        .approval_workflow
        .request(agent_id, action, context, required)
        .await;
    let req = state
        .approval_workflow
        .get_request(&new_id)
        .await
        .ok_or_else(|| GatewayError::Internal("proposal missing after update".into()))?;
    Ok(Json(proposal_json(&req)))
}

async fn delete_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, GatewayError> {
    state
        .approval_workflow
        .reject(&id, "system", "deleted by operator")
        .await
        .map_err(|e| GatewayError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn vote_proposal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<VoteProposalBody>,
) -> Result<Json<Value>, GatewayError> {
    let approver = body.approver_id.unwrap_or_else(|| "operator".to_string());
    let decision = body.decision.unwrap_or_else(|| "approve".to_string());

    if decision == "approve" {
        state
            .approval_workflow
            .approve(&id, &approver)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    } else {
        let reason = body.reason.unwrap_or_else(|| "rejected".to_string());
        state
            .approval_workflow
            .reject(&id, &approver, &reason)
            .await
            .map_err(|e| GatewayError::Internal(e.to_string()))?;
    }

    let req = state
        .approval_workflow
        .get_request(&id)
        .await
        .ok_or_else(|| GatewayError::not_found("Proposal", &id))?;

    state.publish_event(
        "governance.vote",
        json!({ "id": id, "status": format!("{:?}", req.status).to_lowercase() }),
    );

    Ok(Json(proposal_json(&req)))
}
