//! Mesh networking types — peers, paths, routing, and traffic classification.
//!
//! In elastic deployment mode agents and tools communicate over a
//! WireGuard-based overlay network. These types describe peer
//! topology, link metrics, and routing preferences.
//!
//! // Dependency: used by worker::mesh, worker::transport, gateway::mesh_handlers

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── PeerStatus ────────────────────────────────────────────────────────────────

/// Reachability state of a mesh peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PeerStatus {
    Online,
    Offline,
    Degraded,
    #[default]
    Unknown,
}

impl std::fmt::Display for PeerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerStatus::Online => write!(f, "online"),
            PeerStatus::Offline => write!(f, "offline"),
            PeerStatus::Degraded => write!(f, "degraded"),
            PeerStatus::Unknown => write!(f, "unknown"),
        }
    }
}

// ── PeerCapabilities ──────────────────────────────────────────────────────────

/// Services and limits advertised by a peer.
/// // Dependency: used by worker::mesh routing to select best peer for delegation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerCapabilities {
    pub tools: Vec<String>,
    pub models: Vec<String>,
    pub channels: Vec<String>,
    pub can_delegate: bool,
    pub max_concurrent_tasks: u32,
}

// ── PeerInfo ──────────────────────────────────────────────────────────────────

/// A node in the mesh overlay network.
/// // Dependency: stored in worker::mesh state table, passed to traits::Transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: Uuid,
    /// Assigned IP inside the mesh subnet.
    pub mesh_ip: String,
    pub hostname: String,
    pub status: PeerStatus,
    pub last_seen: DateTime<Utc>,
    pub capabilities: PeerCapabilities,
    /// WireGuard public key (base64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

impl PeerInfo {
    pub fn new(id: Uuid, mesh_ip: impl Into<String>, hostname: impl Into<String>) -> Self {
        Self {
            id,
            mesh_ip: mesh_ip.into(),
            hostname: hostname.into(),
            status: PeerStatus::Unknown,
            last_seen: Utc::now(),
            capabilities: PeerCapabilities::default(),
            public_key: None,
        }
    }

    pub fn is_reachable(&self) -> bool {
        matches!(self.status, PeerStatus::Online | PeerStatus::Degraded)
    }

    pub fn mark_seen(&mut self) {
        self.last_seen = Utc::now();
        self.status = PeerStatus::Online;
    }

    pub fn mark_offline(&mut self) {
        self.status = PeerStatus::Offline;
    }

    /// Returns `true` if the peer was last seen more than `secs` seconds ago.
    pub fn is_stale(&self, secs: i64) -> bool {
        let elapsed = Utc::now()
            .signed_duration_since(self.last_seen)
            .num_seconds();
        elapsed > secs
    }
}

// ── TrafficType ───────────────────────────────────────────────────────────────

/// Classification of traffic on the mesh for QoS routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrafficType {
    /// Short-lived request/reply RPCs (low latency critical).
    Rpc,
    /// Streaming data (sustained throughput, ordered).
    Streaming,
    /// Governance / audit messages (reliability critical).
    Governance,
    /// Telemetry and metrics (best-effort).
    Metrics,
    /// Large file or vector transfers (throughput optimised).
    BulkTransfer,
}

// ── RoutingPolicy ─────────────────────────────────────────────────────────────

/// How to prioritise link metrics when selecting a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingPolicy {
    LatencyOptimized,
    ThroughputOptimized,
    ReliabilityOptimized,
    CostOptimized,
}

impl RoutingPolicy {
    /// Return the policy recommended for a given traffic type.
    pub fn for_traffic(t: TrafficType) -> Self {
        match t {
            TrafficType::Rpc => RoutingPolicy::LatencyOptimized,
            TrafficType::Streaming => RoutingPolicy::ThroughputOptimized,
            TrafficType::Governance => RoutingPolicy::ReliabilityOptimized,
            TrafficType::Metrics => RoutingPolicy::CostOptimized,
            TrafficType::BulkTransfer => RoutingPolicy::ThroughputOptimized,
        }
    }
}

// ── MeshPath ──────────────────────────────────────────────────────────────────

/// A known route to a peer with measured or estimated link metrics.
/// // Dependency: used by worker::mesh router to pick the best path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPath {
    pub peer_id: Uuid,
    /// Transport protocol: "wireguard", "tcp", "quic", "http2"
    pub transport: String,
    /// Round-trip latency in milliseconds (measured or estimated).
    pub latency_ms: f64,
    /// Available bandwidth in Mbps.
    pub bandwidth_mbps: f64,
    /// Probability of successful delivery (0.0 – 1.0).
    pub reliability: f64,
    /// Estimated cost per GB in USD.
    pub cost_per_gb: f64,
    pub last_measured: DateTime<Utc>,
}

impl MeshPath {
    pub fn new(
        peer_id: Uuid,
        transport: impl Into<String>,
        latency_ms: f64,
        bandwidth_mbps: f64,
        reliability: f64,
    ) -> Self {
        Self {
            peer_id,
            transport: transport.into(),
            latency_ms,
            bandwidth_mbps,
            reliability,
            cost_per_gb: 0.0,
            last_measured: Utc::now(),
        }
    }

    /// Compute a composite score for the given routing policy.
    /// Higher is better.
    pub fn score(&self, policy: RoutingPolicy) -> f64 {
        match policy {
            RoutingPolicy::LatencyOptimized => {
                // Inverse latency, weighted by reliability.
                if self.latency_ms <= 0.0 {
                    return 0.0;
                }
                (1_000.0 / self.latency_ms) * self.reliability
            }
            RoutingPolicy::ThroughputOptimized => self.bandwidth_mbps * self.reliability,
            RoutingPolicy::ReliabilityOptimized => self.reliability * 100.0,
            RoutingPolicy::CostOptimized => {
                if self.cost_per_gb <= 0.0 {
                    return 100.0;
                }
                (1.0 / self.cost_per_gb) * self.reliability
            }
        }
    }
}

/// Select the best path from a list according to `policy`.
/// // Called by: worker::mesh router before every cross-peer RPC.
pub fn select_best_path(paths: &[MeshPath], policy: RoutingPolicy) -> Option<&MeshPath> {
    paths.iter().filter(|p| p.reliability > 0.0).max_by(|a, b| {
        a.score(policy)
            .partial_cmp(&b.score(policy))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}
