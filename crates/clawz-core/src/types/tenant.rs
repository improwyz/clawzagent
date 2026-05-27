//! RBAC and multi-tenancy types for ClawZ cascade authorization model.
//!
//! The cascade model means higher roles can spawn lower roles, creating
//! a directed acyclic graph of authority. Each scoped context inherits
//! the parent's network but gets a sub-lease of the budget.
//!
//! // Dependency: used by worker::governance, worker::scheduler, gateway::auth middleware

use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// Unique identifier for a tenant in the ClawZ platform.
/// // Dependency: embedded in AgentHandle, TenantContext, and MeshNetwork.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TenantId(String);

impl TenantId {
    /// Create a new TenantId from a string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Get a string reference to the tenant ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TenantId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TenantId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl Default for TenantId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

/// Role hierarchy in the ClawZ cascade authorization model.
/// Higher levels have greater permissions and can spawn child roles.
/// // Dependency: checked by TenantContext::scope_for_agent and scope_for_tool.
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize, PartialOrd, Ord, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Organization owner — complete control and resource allocation.
    Owner,
    /// Administrator — manages teams, policies, and resources.
    Admin,
    /// Operator — executes tasks, manages deployments, creates sub-agents.
    Operator,
    /// Viewer — read-only access to dashboards and logs.
    #[default]
    Viewer,
    /// Agent — autonomous entity spawned by operators, scoped permissions.
    Agent,
    /// Tool — restricted utility role, minimal permissions.
    Tool,
}

impl Role {
    /// Get the numeric level of this role for hierarchy checks.
    /// Higher values indicate higher privilege levels.
    pub fn level(&self) -> u8 {
        match self {
            Role::Owner => 100,
            Role::Admin => 80,
            Role::Operator => 60,
            Role::Viewer => 40,
            Role::Agent => 20,
            Role::Tool => 10,
        }
    }

    /// Check if this role can spawn a child role.
    /// A role can spawn another role only if it has strictly higher level.
    pub fn can_spawn_child(&self, child: Role) -> bool {
        self.level() > child.level()
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Owner => write!(f, "owner"),
            Role::Admin => write!(f, "admin"),
            Role::Operator => write!(f, "operator"),
            Role::Viewer => write!(f, "viewer"),
            Role::Agent => write!(f, "agent"),
            Role::Tool => write!(f, "tool"),
        }
    }
}

/// Fine-grained permission set with wildcard matching support.
/// // Dependency: checked by TenantContext before every scoped action.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionSet {
    /// Permissions as a set of action patterns.
    /// Patterns support `:*` suffix for wildcard matching (e.g., "deploy:*" matches "deploy:create", "deploy:delete").
    permissions: HashSet<String>,
}

impl PermissionSet {
    /// Create a PermissionSet for a given role with default permissions.
    pub fn for_role(role: Role) -> Self {
        let permissions = match role {
            Role::Owner => vec![
                "*".to_string(), // Full wildcard
            ],
            Role::Admin => vec![
                "deploy:*".to_string(),
                "governance:*".to_string(),
                "tenant:*".to_string(),
                "agent:spawn".to_string(),
                "agent:list".to_string(),
                "logs:read".to_string(),
            ],
            Role::Operator => vec![
                "deploy:create".to_string(),
                "deploy:update".to_string(),
                "deploy:delete".to_string(),
                "agent:spawn".to_string(),
                "agent:list".to_string(),
                "agent:monitor".to_string(),
                "logs:read".to_string(),
                "tool:execute".to_string(),
            ],
            Role::Viewer => vec![
                "agent:list".to_string(),
                "deploy:read".to_string(),
                "logs:read".to_string(),
                "metrics:read".to_string(),
            ],
            Role::Agent => vec![
                "agent:self:update".to_string(),
                "agent:spawn_child".to_string(),
                "tool:execute".to_string(),
                "logs:write".to_string(),
            ],
            Role::Tool => vec![
                "tool:self:execute".to_string(),
                "logs:write".to_string(),
                "metrics:write".to_string(),
            ],
        };

        Self {
            permissions: permissions.into_iter().collect(),
        }
    }

    /// Check if this permission set includes a given action.
    /// Supports wildcard matching with `:*` suffix.
    pub fn can(&self, action: &str) -> bool {
        // Exact match
        if self.permissions.contains(action) {
            return true;
        }

        // Full wildcard
        if self.permissions.contains("*") {
            return true;
        }

        // Prefix wildcard matching (e.g., "deploy:*" matches "deploy:create")
        for perm in &self.permissions {
            if perm.ends_with(":*") {
                let prefix = &perm[..perm.len() - 2]; // Remove ":*"
                if action.starts_with(prefix) && action[prefix.len()..].starts_with(':') {
                    return true;
                }
            }
        }

        false
    }

    /// Add a permission to the set.
    pub fn add(&mut self, permission: impl Into<String>) {
        self.permissions.insert(permission.into());
    }
}

/// Budget lease with deduction and sub-lease support.
/// // Dependency: owned by TenantContext, sub-leased to child agents and tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetLease {
    /// Total budget available for this lease.
    total: f64,
    /// Current remaining budget.
    remaining: f64,
    /// Creation timestamp.
    created_at: DateTime<Utc>,
}

impl BudgetLease {
    /// Create a new budget lease with the given total.
    pub fn new(total: f64) -> Self {
        Self {
            total,
            remaining: total,
            created_at: Utc::now(),
        }
    }

    /// Get the remaining budget.
    pub fn remaining(&self) -> f64 {
        self.remaining
    }

    /// Get the total budget for this lease.
    pub fn total(&self) -> f64 {
        self.total
    }

    /// Deduct an amount from the budget.
    /// Returns an error if the amount exceeds the remaining budget.
    pub fn deduct(&mut self, amount: f64) -> Result<()> {
        if amount > self.remaining {
            return Err(crate::error::ClawzError::Budget(format!(
                "insufficient budget: needed {}, available {}",
                amount, self.remaining
            )));
        }
        self.remaining -= amount;
        Ok(())
    }

    /// Create a sub-lease by splitting the current budget proportionally.
    /// Fraction should be between 0.0 and 1.0.
    pub fn sub_lease(&mut self, fraction: f64) -> Self {
        let sub_amount = self.remaining * fraction.clamp(0.0, 1.0);
        let _ = self.deduct(sub_amount); // Already clamped, won't fail
        BudgetLease::new(sub_amount)
    }

    /// Get the creation timestamp.
    pub fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

/// Network scope and isolation constraints for a tenant or agent.
/// // Dependency: owned by TenantContext, enforced by worker::mesh firewall.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkScope {
    /// Mesh network identifier (e.g., NetBird network name).
    pub mesh_network: String,
    /// Subnet CIDR block for this scope.
    pub subnet: String,
    /// Allowed peer identifiers (empty means all peers in the network).
    pub allowed_peers: Vec<String>,
}

impl NetworkScope {
    /// Create a new network scope.
    pub fn new(mesh_network: impl Into<String>, subnet: impl Into<String>) -> Self {
        Self {
            mesh_network: mesh_network.into(),
            subnet: subnet.into(),
            allowed_peers: Vec::new(),
        }
    }

    /// Add an allowed peer to the scope.
    pub fn with_allowed_peer(mut self, peer: impl Into<String>) -> Self {
        self.allowed_peers.push(peer.into());
        self
    }
}

/// Complete tenant context with RBAC, budget, and network isolation.
/// // Dependency: passed to traits::AgentScheduler::spawn_agent, traits::ToolOrchestrator::spawn_tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    /// Unique tenant identifier.
    pub tenant_id: TenantId,
    /// Role for this context.
    pub role: Role,
    /// Fine-grained permissions for this role.
    pub permissions: PermissionSet,
    /// Budget lease for this tenant.
    pub budget: BudgetLease,
    /// Primary network scope for the tenant.
    pub network_scope: NetworkScope,
    /// Context creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Optional expiration time for the context (for session-based scopes).
    pub expires_at: Option<DateTime<Utc>>,
}

impl TenantContext {
    /// Create a new TenantContext with given ID and role.
    pub fn new(tenant_id: TenantId, role: Role) -> Self {
        let permissions = PermissionSet::for_role(role);
        let budget = BudgetLease::new(1000.0); // Default budget
        let network_scope = NetworkScope::new("default-mesh", "10.0.0.0/8");

        Self {
            tenant_id,
            role,
            permissions,
            budget,
            network_scope,
            created_at: Utc::now(),
            expires_at: None,
        }
    }

    /// Create a new TenantContext with custom budget.
    pub fn with_budget(mut self, budget: f64) -> Self {
        self.budget = BudgetLease::new(budget);
        self
    }

    /// Set the network scope for this context.
    pub fn with_network_scope(mut self, scope: NetworkScope) -> Self {
        self.network_scope = scope;
        self
    }

    /// Set an expiration time for this context.
    pub fn with_expiration(mut self, expires_at: DateTime<Utc>) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    /// Create a scoped context for a child agent with downgraded role.
    /// The new role must be lower in the hierarchy than the current role.
    pub fn scope_for_agent(&mut self, agent_id: &str) -> Result<Self> {
        // Agents get Agent role if spawned by Operator or higher
        let child_role = match self.role {
            Role::Owner | Role::Admin | Role::Operator => Role::Agent,
            _ => {
                return Err(crate::error::ClawzError::Governance(
                    "only Owner, Admin, or Operator can spawn agents".to_string(),
                ))
            }
        };

        if !self.role.can_spawn_child(child_role) {
            return Err(crate::error::ClawzError::Governance(format!(
                "{} cannot spawn {} agents",
                self.role, child_role
            )));
        }

        let mut child_context = Self::new(self.tenant_id.clone(), child_role);

        // Sub-lease 50% of remaining budget to the agent
        child_context.budget = self.budget.clone();
        child_context.budget.sub_lease(0.5);

        // Inherit network scope with agent-specific restrictions
        child_context.network_scope = NetworkScope::new(
            self.network_scope.mesh_network.clone(),
            self.network_scope.subnet.clone(),
        );
        child_context
            .network_scope
            .allowed_peers
            .push(agent_id.to_string());

        // Agent contexts expire in 24 hours
        child_context.expires_at = Some(Utc::now() + chrono::Duration::hours(24));

        Ok(child_context)
    }

    /// Create a scoped context for a tool execution within an agent context.
    /// Tools get Tool role with minimal permissions and tight budget constraints.
    pub fn scope_for_tool(&mut self, tool_id: &str, agent_id: &str) -> Result<Self> {
        // Only agents can use tools
        if self.role != Role::Agent && self.role != Role::Operator && self.role != Role::Admin {
            return Err(crate::error::ClawzError::Governance(
                "only agents or higher roles can execute tools".to_string(),
            ));
        }

        let mut tool_context = Self::new(self.tenant_id.clone(), Role::Tool);

        // Tools get 10% of the agent's budget
        tool_context.budget = self.budget.clone();
        tool_context.budget.sub_lease(0.1);

        // Tools can only access the agent's network scope
        tool_context.network_scope = NetworkScope::new(
            self.network_scope.mesh_network.clone(),
            self.network_scope.subnet.clone(),
        );
        tool_context.network_scope.allowed_peers = vec![agent_id.to_string()];
        tool_context
            .network_scope
            .allowed_peers
            .push(tool_id.to_string());

        // Tool contexts expire in 1 hour
        tool_context.expires_at = Some(Utc::now() + chrono::Duration::hours(1));

        Ok(tool_context)
    }

    /// Check if this context has expired.
    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            Utc::now() > expires_at
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_context_new() {
        let tenant_id = TenantId::new("tenant-001");
        let ctx = TenantContext::new(tenant_id.clone(), Role::Owner);

        assert_eq!(ctx.tenant_id, tenant_id);
        assert_eq!(ctx.role, Role::Owner);
        assert_eq!(ctx.budget.total(), 1000.0);
        assert_eq!(ctx.budget.remaining(), 1000.0);
        assert!(!ctx.is_expired());
        assert_eq!(ctx.network_scope.mesh_network, "default-mesh");
    }

    #[test]
    fn role_hierarchy() {
        assert!(Role::Owner.level() > Role::Admin.level());
        assert!(Role::Admin.level() > Role::Operator.level());
        assert!(Role::Operator.level() > Role::Viewer.level());
        assert!(Role::Viewer.level() > Role::Agent.level());
        assert!(Role::Agent.level() > Role::Tool.level());

        assert!(Role::Owner.can_spawn_child(Role::Admin));
        assert!(Role::Operator.can_spawn_child(Role::Agent));
        assert!(Role::Agent.can_spawn_child(Role::Tool));
        assert!(!Role::Tool.can_spawn_child(Role::Owner));
    }

    #[test]
    fn permission_check() {
        let admin_perms = PermissionSet::for_role(Role::Admin);
        assert!(admin_perms.can("deploy:create"));
        assert!(admin_perms.can("deploy:delete"));
        assert!(admin_perms.can("governance:policy"));
        assert!(admin_perms.can("agent:spawn"));
        assert!(!admin_perms.can("owner:nuke"));

        let owner_perms = PermissionSet::for_role(Role::Owner);
        assert!(owner_perms.can("anything"));
        assert!(owner_perms.can("deploy:create"));

        let tool_perms = PermissionSet::for_role(Role::Tool);
        assert!(tool_perms.can("tool:self:execute"));
        assert!(tool_perms.can("metrics:write"));
        assert!(!tool_perms.can("deploy:create"));

        let agent_perms = PermissionSet::for_role(Role::Agent);
        assert!(agent_perms.can("agent:self:update"));
        assert!(agent_perms.can("agent:spawn_child"));
        assert!(agent_perms.can("tool:execute"));
    }

    #[test]
    fn budget_lease_deduction() {
        let mut budget = BudgetLease::new(100.0);
        assert_eq!(budget.remaining(), 100.0);

        // Deduct within budget
        assert!(budget.deduct(30.0).is_ok());
        assert_eq!(budget.remaining(), 70.0);

        // Deduct more
        assert!(budget.deduct(50.0).is_ok());
        assert_eq!(budget.remaining(), 20.0);

        // Try to overdraft
        assert!(budget.deduct(30.0).is_err());
        assert_eq!(budget.remaining(), 20.0); // No change on error
    }

    #[test]
    fn context_scoping_for_agent() {
        let tenant_id = TenantId::new("tenant-001");
        let mut parent_ctx = TenantContext::new(tenant_id, Role::Operator);
        parent_ctx.budget = BudgetLease::new(100.0);

        let agent_ctx = parent_ctx
            .scope_for_agent("agent-001")
            .expect("should create agent scope");

        assert_eq!(agent_ctx.role, Role::Agent);
        assert_eq!(agent_ctx.tenant_id, parent_ctx.tenant_id);
        // Agent should get sub-leased budget (50% of remaining)
        assert!(agent_ctx.budget.remaining() <= 50.0);
        assert!(agent_ctx.expires_at.is_some());

        // Viewer cannot spawn agents
        let mut viewer_ctx = TenantContext::new(TenantId::new("tenant-002"), Role::Viewer);
        assert!(viewer_ctx.scope_for_agent("agent-002").is_err());
    }

    #[test]
    fn budget_sub_lease() {
        let mut budget = BudgetLease::new(100.0);
        let sub_budget = budget.sub_lease(0.5);

        assert_eq!(sub_budget.total(), 50.0);
        assert_eq!(sub_budget.remaining(), 50.0);
        assert_eq!(budget.remaining(), 50.0); // Parent budget reduced
    }

    #[test]
    fn tenant_id_display() {
        let id = TenantId::new("my-tenant");
        assert_eq!(id.to_string(), "my-tenant");
        assert_eq!(id.as_str(), "my-tenant");
    }

    #[test]
    fn context_expiration() {
        let tenant_id = TenantId::new("tenant-001");
        let mut ctx = TenantContext::new(tenant_id, Role::Owner);

        assert!(!ctx.is_expired());

        // Set expiration to 1 second ago
        ctx.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(ctx.is_expired());

        // Set expiration to 1 hour in future
        ctx.expires_at = Some(Utc::now() + chrono::Duration::hours(1));
        assert!(!ctx.is_expired());
    }

    #[test]
    fn network_scope_with_peers() {
        let scope = NetworkScope::new("mesh-prod", "10.0.0.0/8")
            .with_allowed_peer("peer-001")
            .with_allowed_peer("peer-002");

        assert_eq!(scope.mesh_network, "mesh-prod");
        assert_eq!(scope.subnet, "10.0.0.0/8");
        assert_eq!(scope.allowed_peers.len(), 2);
        assert!(scope.allowed_peers.contains(&"peer-001".to_string()));
    }
}
