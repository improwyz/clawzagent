//! Mesh networking configuration.
//!
//! [`MeshConfig`] is loaded from the `[mesh]` section of `provider.toml`.
//! [`NetworkIdentity`] is derived at runtime (or loaded from a saved key).
//!
//! # Role in Networking
//!
//! These structs control peer discovery parameters, heartbeat timing,
//! and route caching — all tuned for WAN mesh behaviour.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── MeshConfig ────────────────────────────────────────────────────────────────

/// Top-level mesh networking configuration.
///
/// Loaded from TOML (`[mesh]` section) and consumed by [`crate::mesh::manager::MeshManager`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MeshConfig {
    /// Whether mesh networking is active.
    pub enabled: bool,
    /// Human-readable network name (used for mDNS service discovery).
    pub network_name: String,
    /// UDP port to listen on for mesh traffic.
    pub listen_port: u16,
    /// Bootstrap peers to connect to on startup (host:port).
    pub bootstrap_peers: Vec<String>,
    /// How frequently to send heartbeats when a peer is healthy (ms).
    pub heartbeat_interval_ms: u64,
    /// How long without a heartbeat before marking a peer suspect (ms).
    pub heartbeat_timeout_ms: u64,
    /// Maximum number of simultaneous transport paths per peer.
    pub max_paths_per_peer: usize,
    /// How long to cache route decisions (ms).
    pub route_cache_ttl_ms: u64,
    /// How long an unreachable peer stays in the registry before being reaped (seconds).
    pub stale_peer_timeout_secs: i64,
    /// Gateway API URL for API-based peer discovery (optional).
    pub gateway_api_url: Option<String>,
    /// How often to re-run discovery (seconds).
    pub discovery_interval_secs: u64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            network_name: "clawz-mesh".to_string(),
            listen_port: 51820,
            bootstrap_peers: Vec::new(),
            // 5 s balances responsiveness with bandwidth; shorter intervals
            // are used for suspect peers (see heartbeat adaptive interval).
            heartbeat_interval_ms: 5_000,
            // 15 s gives three heartbeat windows before suspicion.
            heartbeat_timeout_ms: 15_000,
            max_paths_per_peer: 3,
            route_cache_ttl_ms: 30_000,
            // 5 minutes: long enough to survive brief network partitions,
            // short enough to keep the peer list tidy.
            stale_peer_timeout_secs: 300,
            gateway_api_url: None,
            discovery_interval_secs: 60,
        }
    }
}

// ── NetworkIdentity ───────────────────────────────────────────────────────────

/// Persistent identity of this node in the mesh.
///
/// Normally loaded from disk; generated on first start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkIdentity {
    /// Stable node identifier (UUID v4).
    pub node_id: Uuid,
    /// WireGuard-style mesh IP (100.64.x.x/10).
    pub mesh_ip: String,
    /// Base64-encoded Ed25519 public key.
    pub public_key: String,
    /// Hostname (from `gethostname` or explicit config).
    pub hostname: String,
}

impl NetworkIdentity {
    /// Create a new randomly-generated identity.
    pub fn generate() -> Self {
        let node_id = Uuid::new_v4();
        // Assign a deterministic mesh IP from the CGNAT range (100.64.0.0/10).
        // In a real deployment, the management server would allocate these.
        let octets = node_id.as_bytes();
        let b1 = octets[0] & 0x3F; // keep within 0–63
        let b2 = octets[1];
        let mesh_ip = format!("100.64.{b1}.{b2}");

        // Generate a placeholder public key (real impl would use ed25519-dalek).
        let key_bytes: Vec<u8> = (0..32).map(|i| octets[i % 16] ^ (i as u8)).collect();
        let public_key = base64_encode(&key_bytes);

        let hostname = hostname_or_default();

        Self {
            node_id,
            mesh_ip,
            public_key,
            hostname,
        }
    }

    /// Load from a TOML string, or generate a fresh identity if the string is empty.
    pub fn from_toml_or_generate(toml: &str) -> Result<Self, toml::de::Error> {
        if toml.trim().is_empty() {
            Ok(Self::generate())
        } else {
            toml::from_str(toml)
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Simple base64 encoder (no padding) for the placeholder public key.
///
/// We avoid pulling in the `base64` crate here because this is a
/// temporary placeholder until real Ed25519 key generation is wired up.
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(data.len() * 4 / 3 + 4);
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut i = 0usize;
    while i + 2 < data.len() {
        let b0 = data[i] as usize;
        let b1 = data[i + 1] as usize;
        let b2 = data[i + 2] as usize;
        let _ = write!(
            out,
            "{}{}{}{}",
            CHARS[b0 >> 2] as char,
            CHARS[((b0 & 0x3) << 4) | (b1 >> 4)] as char,
            CHARS[((b1 & 0xF) << 2) | (b2 >> 6)] as char,
            CHARS[b2 & 0x3F] as char,
        );
        i += 3;
    }
    out
}

/// Read the system hostname from `/etc/hostname`, falling back to a default.
///
/// `/etc/hostname` is the most reliable source on Linux containers;
/// `gethostname` would require an extra dependency (`libc` or `whoami`).
fn hostname_or_default() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
        .pipe_or("clawz-node".to_string())
}

/// Convenience trait: return `default` if the string is empty.
trait PipeOr {
    fn pipe_or(self, default: String) -> String;
}

impl PipeOr for String {
    fn pipe_or(self, default: String) -> String {
        if self.is_empty() { default } else { self }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_config_default() {
        let cfg = MeshConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.heartbeat_interval_ms, 5_000);
        assert_eq!(cfg.max_paths_per_peer, 3);
    }

    #[test]
    fn mesh_config_from_toml() {
        let toml = r#"
enabled = true
network_name = "test-mesh"
listen_port = 51821
bootstrap_peers = ["10.0.0.1:51820"]
heartbeat_interval_ms = 3000
heartbeat_timeout_ms = 10000
max_paths_per_peer = 2
route_cache_ttl_ms = 5000
stale_peer_timeout_secs = 120
discovery_interval_secs = 30
"#;
        let cfg: MeshConfig = toml::from_str(toml).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.network_name, "test-mesh");
        assert_eq!(cfg.listen_port, 51821);
        assert_eq!(cfg.bootstrap_peers, vec!["10.0.0.1:51820"]);
        assert_eq!(cfg.heartbeat_interval_ms, 3000);
        assert_eq!(cfg.max_paths_per_peer, 2);
    }

    #[test]
    fn network_identity_generate() {
        let id = NetworkIdentity::generate();
        assert!(id.mesh_ip.starts_with("100.64."));
        assert!(!id.public_key.is_empty());
        assert!(!id.hostname.is_empty());
    }

    #[test]
    fn network_identity_from_toml() {
        let toml = r#"
node_id = "550e8400-e29b-41d4-a716-446655440000"
mesh_ip = "100.64.1.1"
public_key = "dGVzdA=="
hostname = "test-host"
"#;
        let id = NetworkIdentity::from_toml_or_generate(toml).unwrap();
        assert_eq!(id.mesh_ip, "100.64.1.1");
        assert_eq!(id.hostname, "test-host");
    }

    #[test]
    fn network_identity_empty_toml_generates() {
        let id = NetworkIdentity::from_toml_or_generate("").unwrap();
        assert!(!id.mesh_ip.is_empty());
    }
}
