//! Mesh traffic router.
//!
//! Selects the best transport path for a given (peer, traffic-type) pair
//! according to the active routing policy.  Route decisions are cached for
//! `route_cache_ttl_ms` to avoid recomputing on every call.
//!
//! # Role in Networking
//!
//! The router is the "brain" of the mesh: it turns a destination peer ID
//! into a concrete transport choice (gRPC, QUIC, WSS).  It consumes
//! live path statistics produced by the heartbeat layer and consults
//! per-traffic-type policies from `clawz_core::types::mesh::RoutingPolicy`.
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::heartbeat::HeartbeatProtocol`] — feeds RTT and health data.
//! - [`crate::mesh::manager::MeshManager`] — owns the router and invokes it for every send.
//! - `clawz_core::types::mesh::{MeshPath, RoutingPolicy, TrafficType}` — shared primitives.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

// Dependency: clawz-core::types::mesh — shared routing primitives.
use clawz_core::types::mesh::{MeshPath, RoutingPolicy, TrafficType};

// ── RouteEntry ────────────────────────────────────────────────────────────────

/// A cached routing decision for a specific (peer, traffic type) pair.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// Selected path.
    pub path: MeshPath,
    /// When this entry was computed.
    pub created_at: Instant,
    /// TTL for this cache entry.
    pub ttl: Duration,
    /// Score used to select this path.
    pub score: f64,
}

impl RouteEntry {
    /// Return true if this cached entry should be recomputed.
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

// ── PathStats ─────────────────────────────────────────────────────────────────

/// Live measured statistics for a transport path to a peer.
///
/// The router maintains one `PathStats` per (peer_id, transport_name) pair
/// and updates it as measurements arrive.
#[derive(Debug, Clone)]
pub struct PathStats {
    /// Name of the transport (e.g. "grpc", "quic", "wss").
    pub transport: String,
    /// Most recently measured RTT in ms.
    pub latency_ms: f64,
    /// Most recently measured bandwidth in Mbps.
    pub bandwidth_mbps: f64,
    /// Estimated packet delivery ratio (0.0–1.0).
    pub reliability: f64,
    /// Estimated cost per GB (USD).
    pub cost_per_gb: f64,
    /// Whether the path is currently considered healthy.
    pub healthy: bool,
    /// When these stats were last updated.
    pub last_updated: Instant,
}

impl PathStats {
    /// Create a new stats entry with optimistic defaults.
    pub fn new(transport: impl Into<String>) -> Self {
        Self {
            transport: transport.into(),
            latency_ms: 0.0,
            // Start with a high optimistic bandwidth so newly-registered paths
            // are not penalised before the first real measurement arrives.
            bandwidth_mbps: 1_000.0,
            reliability: 1.0,
            cost_per_gb: 0.0,
            healthy: true,
            last_updated: Instant::now(),
        }
    }

    /// Convert to a [`MeshPath`] for use with the scoring functions in core.
    pub fn to_mesh_path(&self, peer_id: Uuid) -> MeshPath {
        MeshPath::new(
            peer_id,
            &self.transport,
            self.latency_ms,
            self.bandwidth_mbps,
            self.reliability,
        )
    }

    /// Compute a score for the given policy.  Higher is better.
    pub fn score(&self, policy: RoutingPolicy) -> f64 {
        if !self.healthy {
            return -1.0;
        }
        self.to_mesh_path(Uuid::nil()).score(policy)
    }
}

// ── MeshRouter ────────────────────────────────────────────────────────────────

/// Traffic-type-aware mesh router with route caching.
///
/// # Thread Safety
/// All mutable state is wrapped in `Arc<RwLock<>>` so the router can be
/// cloned and shared across tokio tasks.
#[derive(Clone)]
pub struct MeshRouter {
    /// Default policy when no per-traffic-type override is configured.
    default_policy: RoutingPolicy,
    /// Per-traffic-type policy overrides.
    traffic_policies: Arc<RwLock<HashMap<String, RoutingPolicy>>>,
    /// Live path statistics: peer_id → transport_name → stats.
    path_stats: Arc<RwLock<HashMap<Uuid, HashMap<String, PathStats>>>>,
    /// Route cache: (peer_id, traffic_type_str) → RouteEntry.
    route_cache: Arc<RwLock<HashMap<(Uuid, String), RouteEntry>>>,
    /// How long route cache entries live.
    cache_ttl: Duration,
}

impl MeshRouter {
    /// Create a new router with the given default policy and cache TTL.
    pub fn new(default_policy: RoutingPolicy, cache_ttl_ms: u64) -> Self {
        let mut traffic_policies: HashMap<String, RoutingPolicy> = HashMap::new();
        // Pre-populate the well-known traffic-type → policy mapping.
        // These defaults reflect typical workload characteristics:
        // RPC wants low latency, streaming wants throughput, governance
        // wants reliability, metrics want cheap paths, bulk wants throughput.
        traffic_policies.insert("rpc".to_string(), RoutingPolicy::LatencyOptimized);
        traffic_policies.insert("streaming".to_string(), RoutingPolicy::ThroughputOptimized);
        traffic_policies.insert(
            "governance".to_string(),
            RoutingPolicy::ReliabilityOptimized,
        );
        traffic_policies.insert("metrics".to_string(), RoutingPolicy::CostOptimized);
        traffic_policies.insert(
            "bulk_transfer".to_string(),
            RoutingPolicy::ThroughputOptimized,
        );

        Self {
            default_policy,
            traffic_policies: Arc::new(RwLock::new(traffic_policies)),
            path_stats: Arc::new(RwLock::new(HashMap::new())),
            route_cache: Arc::new(RwLock::new(HashMap::new())),
            cache_ttl: Duration::from_millis(cache_ttl_ms),
        }
    }

    // ── Path management ────────────────────────────────────────────────────────

    /// Register or update the stats for a transport path to a peer.
    pub async fn upsert_path(
        &self,
        peer_id: Uuid,
        transport: impl Into<String>,
        latency_ms: f64,
        bandwidth_mbps: f64,
        reliability: f64,
    ) {
        let transport = transport.into();
        let mut stats = self.path_stats.write().await;
        let peer_stats = stats.entry(peer_id).or_default();
        let entry = peer_stats
            .entry(transport.clone())
            .or_insert_with(|| PathStats::new(&transport));
        entry.latency_ms = latency_ms;
        entry.bandwidth_mbps = bandwidth_mbps;
        entry.reliability = reliability;
        entry.last_updated = Instant::now();

        // Invalidate cached routes for this peer so the next send()
        // sees the updated statistics.
        drop(stats);
        self.invalidate_peer_routes(peer_id).await;
    }

    /// Mark a path as unhealthy (e.g. heartbeat detected it as Down).
    pub async fn mark_path_unhealthy(&self, peer_id: &Uuid, transport: &str) {
        let mut stats = self.path_stats.write().await;
        if let Some(peer_stats) = stats.get_mut(peer_id) {
            if let Some(path) = peer_stats.get_mut(transport) {
                path.healthy = false;
                path.last_updated = Instant::now();
            }
        }
        drop(stats);
        self.invalidate_peer_routes(*peer_id).await;
    }

    /// Mark a path as healthy again.
    pub async fn mark_path_healthy(&self, peer_id: &Uuid, transport: &str) {
        let mut stats = self.path_stats.write().await;
        if let Some(peer_stats) = stats.get_mut(peer_id) {
            if let Some(path) = peer_stats.get_mut(transport) {
                path.healthy = true;
                path.last_updated = Instant::now();
            }
        }
        drop(stats);
        self.invalidate_peer_routes(*peer_id).await;
    }

    /// Remove all paths for a peer (when peer is removed from the mesh).
    pub async fn remove_peer(&self, peer_id: &Uuid) {
        self.path_stats.write().await.remove(peer_id);
        self.invalidate_peer_routes(*peer_id).await;
    }

    // ── Route selection ────────────────────────────────────────────────────────

    /// Select the best path for the given peer and traffic type.
    ///
    /// Returns `None` if no healthy paths are registered for that peer.
    pub async fn select_path(&self, peer_id: Uuid, traffic_type: TrafficType) -> Option<MeshPath> {
        let cache_key = (peer_id, traffic_type_str(traffic_type).to_string());

        // Check cache first to avoid recomputing scores on hot paths.
        {
            let cache = self.route_cache.read().await;
            if let Some(entry) = cache.get(&cache_key) {
                if !entry.is_expired() {
                    return Some(entry.path.clone());
                }
            }
        }

        let policy = self.policy_for(traffic_type).await;
        let path = self.compute_best_path(peer_id, policy).await?;
        let score = path.score(policy);

        let entry = RouteEntry {
            path: path.clone(),
            created_at: Instant::now(),
            ttl: self.cache_ttl,
            score,
        };
        self.route_cache.write().await.insert(cache_key, entry);

        Some(path)
    }

    /// Return all healthy paths for a peer, sorted by score for the given policy.
    pub async fn ranked_paths(&self, peer_id: Uuid, traffic_type: TrafficType) -> Vec<MeshPath> {
        let policy = self.policy_for(traffic_type).await;
        let stats = self.path_stats.read().await;
        let peer_stats = match stats.get(&peer_id) {
            Some(s) => s,
            None => return vec![],
        };

        let mut paths: Vec<(f64, MeshPath)> = peer_stats
            .values()
            .filter(|s| s.healthy)
            .map(|s| (s.score(policy), s.to_mesh_path(peer_id)))
            .collect();

        paths.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        paths.into_iter().map(|(_, p)| p).collect()
    }

    /// Compute a score for a specific path given the traffic type.
    pub async fn calculate_path_score(
        &self,
        peer_id: &Uuid,
        transport: &str,
        traffic_type: TrafficType,
    ) -> f64 {
        let policy = self.policy_for(traffic_type).await;
        let stats = self.path_stats.read().await;
        stats
            .get(peer_id)
            .and_then(|p| p.get(transport))
            .map(|s| s.score(policy))
            .unwrap_or(0.0)
    }

    // ── Policy management ──────────────────────────────────────────────────────

    /// Override the routing policy for a specific traffic type.
    pub async fn set_policy(&self, traffic_type: TrafficType, policy: RoutingPolicy) {
        self.traffic_policies
            .write()
            .await
            .insert(traffic_type_str(traffic_type).to_string(), policy);
    }

    /// Return the effective policy for a traffic type.
    pub async fn policy_for(&self, traffic_type: TrafficType) -> RoutingPolicy {
        let policies = self.traffic_policies.read().await;
        policies
            .get(traffic_type_str(traffic_type))
            .copied()
            .unwrap_or(self.default_policy)
    }

    // ── Cache management ───────────────────────────────────────────────────────

    /// Remove all cached routes for a peer.
    async fn invalidate_peer_routes(&self, peer_id: Uuid) {
        let mut cache = self.route_cache.write().await;
        cache.retain(|(pid, _), _| *pid != peer_id);
    }

    /// Evict all expired cache entries.
    pub async fn evict_expired_routes(&self) {
        self.route_cache
            .write()
            .await
            .retain(|_, entry| !entry.is_expired());
    }

    // ── Internal helpers ───────────────────────────────────────────────────────

    async fn compute_best_path(&self, peer_id: Uuid, policy: RoutingPolicy) -> Option<MeshPath> {
        let stats = self.path_stats.read().await;
        let peer_stats = stats.get(&peer_id)?;

        peer_stats
            .values()
            .filter(|s| s.healthy)
            .map(|s| (s.score(policy), s.to_mesh_path(peer_id)))
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, path)| path)
    }

    /// Return true if any healthy path is available for a peer.
    pub async fn has_healthy_path(&self, peer_id: &Uuid) -> bool {
        let stats = self.path_stats.read().await;
        stats
            .get(peer_id)
            .map(|p| p.values().any(|s| s.healthy))
            .unwrap_or(false)
    }

    /// Return all peer IDs that have at least one registered path.
    pub async fn known_peers(&self) -> Vec<Uuid> {
        self.path_stats.read().await.keys().copied().collect()
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Map a [`TrafficType`] to its policy-table key string.
fn traffic_type_str(t: TrafficType) -> &'static str {
    match t {
        TrafficType::Rpc => "rpc",
        TrafficType::Streaming => "streaming",
        TrafficType::Governance => "governance",
        TrafficType::Metrics => "metrics",
        TrafficType::BulkTransfer => "bulk_transfer",
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::mesh::TrafficType;

    fn make_router() -> MeshRouter {
        MeshRouter::new(RoutingPolicy::LatencyOptimized, 30_000)
    }

    #[tokio::test]
    async fn select_best_latency_path() {
        let router = make_router();
        let peer = Uuid::new_v4();

        router.upsert_path(peer, "grpc", 10.0, 1000.0, 0.99).await;
        router.upsert_path(peer, "quic", 5.0, 1000.0, 0.99).await;
        router.upsert_path(peer, "wss", 150.0, 100.0, 0.95).await;

        let path = router.select_path(peer, TrafficType::Rpc).await.unwrap();
        assert_eq!(path.transport, "quic"); // lowest latency
    }

    #[tokio::test]
    async fn select_best_throughput_path() {
        let router = make_router();
        let peer = Uuid::new_v4();

        router.upsert_path(peer, "grpc", 10.0, 100.0, 0.99).await;
        router.upsert_path(peer, "quic", 5.0, 10_000.0, 0.99).await;

        let path = router
            .select_path(peer, TrafficType::BulkTransfer)
            .await
            .unwrap();
        assert_eq!(path.transport, "quic"); // highest bandwidth
    }

    #[tokio::test]
    async fn unhealthy_path_not_selected() {
        let router = make_router();
        let peer = Uuid::new_v4();

        router.upsert_path(peer, "grpc", 5.0, 1000.0, 0.99).await;
        router.upsert_path(peer, "quic", 1.0, 1000.0, 0.99).await;
        router.mark_path_unhealthy(&peer, "quic").await;

        let path = router.select_path(peer, TrafficType::Rpc).await.unwrap();
        assert_eq!(path.transport, "grpc"); // quic is unhealthy
    }

    #[tokio::test]
    async fn cache_hit() {
        let router = make_router();
        let peer = Uuid::new_v4();

        router.upsert_path(peer, "grpc", 10.0, 1000.0, 0.99).await;
        let p1 = router.select_path(peer, TrafficType::Rpc).await.unwrap();
        // Second call should hit cache.
        let p2 = router.select_path(peer, TrafficType::Rpc).await.unwrap();
        assert_eq!(p1.transport, p2.transport);
    }

    #[tokio::test]
    async fn ranked_paths_order() {
        let router = make_router();
        let peer = Uuid::new_v4();

        router.upsert_path(peer, "grpc", 50.0, 1000.0, 0.99).await;
        router.upsert_path(peer, "quic", 5.0, 1000.0, 0.99).await;
        router.upsert_path(peer, "wss", 200.0, 100.0, 0.90).await;

        let ranked = router.ranked_paths(peer, TrafficType::Rpc).await;
        assert_eq!(ranked[0].transport, "quic"); // best latency first
    }

    #[tokio::test]
    async fn no_paths_returns_none() {
        let router = make_router();
        let peer = Uuid::new_v4();
        assert!(router.select_path(peer, TrafficType::Rpc).await.is_none());
    }

    #[tokio::test]
    async fn governance_uses_reliability_policy() {
        let router = make_router();
        let peer = Uuid::new_v4();

        // grpc has higher reliability, quic has lower latency
        router.upsert_path(peer, "grpc", 50.0, 1000.0, 1.0).await;
        router.upsert_path(peer, "quic", 5.0, 1000.0, 0.7).await;

        let path = router
            .select_path(peer, TrafficType::Governance)
            .await
            .unwrap();
        assert_eq!(path.transport, "grpc"); // reliability wins
    }
}
