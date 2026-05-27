//! Per-tenant mesh firewall.
//!
//! Enforces network-level isolation so that agents and tools belonging to
//! different tenants cannot communicate across tenant boundaries.  Within a
//! tenant, agent-to-agent traffic is always permitted; tool-to-agent traffic
//! is only permitted when the tool is explicitly bound to that agent.
//!
//! # Role in Networking
//!
//! The firewall sits **after** transport decryption but **before** the mesh
//! router forwards a message.  It is consulted by [`crate::mesh::manager::MeshManager`]
//! on every inter-peer message.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::TenantMesh` — core trait this module implements.

use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};

// Dependency: clawz-core::traits::TenantMesh — enforced per-tenant via trait.

/// Firewall rules for a single tenant within the mesh.
///
/// All fields are guarded by `RwLock` so that rules can be updated
/// concurrently with packet-filtering checks.
pub struct TenantFirewall {
    /// The tenant this firewall instance protects.
    #[allow(dead_code)]
    tenant_id: String,
    /// Registered agent IP addresses within the tenant.
    agent_ips: RwLock<HashSet<String>>,
    /// Mapping from tool ID → (tool_ip, agent_ip) for tool→agent allow rules.
    tool_to_agent: RwLock<HashMap<String, (String, String)>>,
}

impl TenantFirewall {
    /// Create a new empty firewall for the given tenant.
    pub fn new(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            agent_ips: RwLock::new(HashSet::new()),
            tool_to_agent: RwLock::new(HashMap::new()),
        }
    }

    /// Register an agent IP so it can participate in intra-tenant mesh traffic.
    pub fn add_agent(&self, _agent_id: &str, ip: &str) {
        self.agent_ips.write().insert(ip.to_string());
    }

    /// Remove an agent IP; its traffic will no longer be permitted.
    pub fn remove_agent(&self, ip: &str) {
        self.agent_ips.write().remove(ip);
    }

    /// Bind a tool to an agent so the tool may reach that agent.
    ///
    /// This creates a unidirectional allow rule: `tool_ip → agent_ip`.
    pub fn add_tool(&self, tool_id: &str, tool_ip: &str, _agent_id: &str, agent_ip: &str) {
        self.tool_to_agent.write().insert(
            tool_id.to_string(),
            (tool_ip.to_string(), agent_ip.to_string()),
        );
    }

    /// Revoke a tool's access to its bound agent.
    pub fn remove_tool(&self, tool_id: &str) {
        self.tool_to_agent.write().remove(tool_id);
    }

    /// Return true if traffic from `src_ip` to `dst_ip` is permitted.
    ///
    /// Rules evaluated in order:
    /// 1. Agent → Agent within the same tenant: always allowed.
    /// 2. Tool → its bound Agent: allowed.
    /// 3. Everything else: denied.
    pub fn allows(&self, src_ip: &str, dst_ip: &str) -> bool {
        let agents = self.agent_ips.read();

        // Agent → Agent: always allowed within tenant
        if agents.contains(src_ip) && agents.contains(dst_ip) {
            return true;
        }

        // Tool → owning Agent: allowed
        let tools = self.tool_to_agent.read();
        for (_, (tool_ip, agent_ip)) in tools.iter() {
            if src_ip == tool_ip && dst_ip == agent_ip {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_can_reach_owning_agent() {
        let rules = TenantFirewall::new("tenant-1");
        rules.add_tool("tool-50", "10.64.1.50", "agent-10", "10.64.1.10");
        assert!(rules.allows("10.64.1.50", "10.64.1.10"));
    }

    #[test]
    fn tool_cannot_reach_other_agent() {
        let rules = TenantFirewall::new("tenant-1");
        rules.add_tool("tool-50", "10.64.1.50", "agent-10", "10.64.1.10");
        assert!(!rules.allows("10.64.1.50", "10.64.1.11"));
    }

    #[test]
    fn agent_can_reach_peer_agent() {
        let rules = TenantFirewall::new("tenant-1");
        rules.add_agent("agent-10", "10.64.1.10");
        rules.add_agent("agent-11", "10.64.1.11");
        assert!(rules.allows("10.64.1.10", "10.64.1.11"));
    }

    #[test]
    fn remove_tool_removes_rules() {
        let rules = TenantFirewall::new("tenant-1");
        rules.add_tool("tool-50", "10.64.1.50", "agent-10", "10.64.1.10");
        rules.remove_tool("tool-50");
        assert!(!rules.allows("10.64.1.50", "10.64.1.10"));
    }
}
