//! Helpers to build tenant-scoped context from gateway authentication.

use clawz_core::types::tenant::{Role, TenantContext, TenantId};

use crate::auth::AuthContext;

/// Map gateway [`AuthContext`] to orchestration [`TenantContext`].
pub fn tenant_context_from_auth(auth: &AuthContext) -> TenantContext {
    let role = match auth.role.to_lowercase().as_str() {
        "owner" => Role::Owner,
        "admin" => Role::Admin,
        "operator" => Role::Operator,
        "agent" => Role::Agent,
        "tool" => Role::Tool,
        _ => Role::Viewer,
    };
    TenantContext::new(TenantId::new(auth.tenant_id.clone()), role)
}
