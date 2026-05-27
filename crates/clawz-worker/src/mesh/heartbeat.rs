//! Multi-path heartbeat protocol.
//!
//! Each peer connection maintains heartbeat state per transport path.
//! Heartbeats run simultaneously on ALL available paths (gRPC + QUIC + WSS)
//! so that path failures are detected quickly without disrupting traffic.
//!
//! # State machine (per path)
//!
//! ```text
//!  Active  ──(3 missed)──► Suspect ──(5+ missed)──► Down
//!    ▲                       │                        │
//!    └──────(pong rx)────────┘                        │
//!    └────────────────────────────(pong rx)───────────┘
//! ```
//!
//! # Role in Networking
//!
//! Heartbeat state drives peer health which in turn affects:
//! - [`crate::mesh::router::MeshRouter`] path selection (unhealthy paths are avoided).
//! - [`crate::mesh::fleet::FleetMesh`] rebalancing (suspect peers are excluded).
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::manager::MeshManager`] — owns the HeartbeatProtocol instance.
//! - `clawz_core::types::mesh::TrafficType` — heartbeats use `Rpc` traffic type.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use uuid::Uuid;

// Dependency: crate::mesh::manager — heartbeat state consumed by MeshManager.
// Dependency: crate::mesh::router — path health directly influences routing decisions.

// ── PathHealthStatus ──────────────────────────────────────────────────────────

/// Health state of a single transport path to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathHealthStatus {
    /// Path is healthy; 0–2 consecutive missed heartbeats.
    Active,
    /// Path may be degraded; 3–4 consecutive missed heartbeats.
    Suspect,
    /// Path is considered down; 5+ consecutive missed heartbeats.
    Down,
    /// Path was down and is recovering (received at least one pong after Down).
    Recovering,
}

impl PathHealthStatus {
    /// Return true if this path may still carry traffic.
    pub fn is_usable(self) -> bool {
        matches!(
            self,
            PathHealthStatus::Active | PathHealthStatus::Recovering
        )
    }
}

impl std::fmt::Display for PathHealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathHealthStatus::Active => write!(f, "active"),
            PathHealthStatus::Suspect => write!(f, "suspect"),
            PathHealthStatus::Down => write!(f, "down"),
            PathHealthStatus::Recovering => write!(f, "recovering"),
        }
    }
}

// ── HeartbeatState ────────────────────────────────────────────────────────────

/// Per-path heartbeat tracking state.
#[derive(Debug, Clone)]
pub struct HeartbeatState {
    /// Name of the transport path (e.g. "grpc", "quic", "wss").
    pub path_name: String,
    /// Time the last ping was sent on this path.
    pub last_sent: Instant,
    /// Time the last pong was received on this path.
    pub last_received: Option<Instant>,
    /// Exponentially-smoothed RTT in milliseconds.
    pub rtt_ms: f64,
    /// Number of consecutive missed heartbeats.
    pub missed_count: u32,
    /// Current health status.
    pub status: PathHealthStatus,
    /// Total heartbeats sent.
    pub sent_total: u64,
    /// Total heartbeats received (pongs).
    pub received_total: u64,
}

impl HeartbeatState {
    /// Create a new heartbeat tracker for the named path.
    pub fn new(path_name: impl Into<String>) -> Self {
        Self {
            path_name: path_name.into(),
            last_sent: Instant::now(),
            last_received: None,
            rtt_ms: 0.0,
            missed_count: 0,
            status: PathHealthStatus::Active,
            sent_total: 0,
            received_total: 0,
        }
    }

    /// Record that a heartbeat was sent on this path.
    pub fn record_sent(&mut self) {
        self.last_sent = Instant::now();
        self.sent_total += 1;
    }

    /// Record a pong received on this path; updates RTT (EWMA) and health.
    pub fn record_received(&mut self, rtt_ms: f64) {
        let now = Instant::now();
        self.last_received = Some(now);
        self.received_total += 1;
        self.missed_count = 0;

        // Exponentially weighted moving average, alpha = 0.2.
        // Alpha was chosen to be responsive to RTT spikes without
        // overreacting to single outliers (common on shared cloud hosts).
        if self.rtt_ms <= 0.0 {
            self.rtt_ms = rtt_ms;
        } else {
            self.rtt_ms = 0.8 * self.rtt_ms + 0.2 * rtt_ms;
        }

        self.status = match self.status {
            PathHealthStatus::Down | PathHealthStatus::Recovering => PathHealthStatus::Recovering,
            _ => PathHealthStatus::Active,
        };
    }

    /// Record a missed heartbeat; transitions state if threshold is crossed.
    pub fn record_missed(&mut self) {
        self.missed_count += 1;
        // Thresholds chosen empirically: 3 missed gives ~15 s warning
        // (at 5 s interval) before the path is marked Down at 5 missed.
        self.status = match self.missed_count {
            0..=2 => PathHealthStatus::Active,
            3..=4 => PathHealthStatus::Suspect,
            _ => PathHealthStatus::Down,
        };
    }

    /// Return true if this path should be considered for routing.
    pub fn is_usable(&self) -> bool {
        self.status.is_usable()
    }

    /// How long ago the last pong was received (or None if never).
    pub fn since_last_received(&self) -> Option<Duration> {
        self.last_received.map(|t| t.elapsed())
    }
}

// ── PeerHeartbeats ────────────────────────────────────────────────────────────

/// Heartbeat state for all paths to a single peer.
#[derive(Debug)]
pub struct PeerHeartbeats {
    /// UUID of the peer these paths belong to.
    pub peer_id: Uuid,
    /// Map from path name → state.
    pub paths: HashMap<String, HeartbeatState>,
}

impl PeerHeartbeats {
    /// Create a new tracker for `peer_id` with the given path names.
    pub fn new(peer_id: Uuid, path_names: &[&str]) -> Self {
        let paths = path_names
            .iter()
            .map(|&name| (name.to_string(), HeartbeatState::new(name)))
            .collect();
        Self { peer_id, paths }
    }

    /// Return the best (lowest RTT) usable path, if any.
    ///
    /// Falls back to any usable path with no RTT data if none have
    /// measured latency yet (common right after peer registration).
    pub fn best_path(&self) -> Option<&HeartbeatState> {
        self.paths
            .values()
            .filter(|s| s.is_usable() && s.rtt_ms > 0.0)
            .min_by(|a, b| {
                a.rtt_ms
                    .partial_cmp(&b.rtt_ms)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .or_else(|| {
                // Fall back to any active/recovering path with no RTT data yet.
                self.paths.values().find(|s| s.is_usable())
            })
    }

    /// Return all usable paths sorted by RTT (ascending).
    pub fn usable_paths(&self) -> Vec<&HeartbeatState> {
        let mut usable: Vec<&HeartbeatState> =
            self.paths.values().filter(|s| s.is_usable()).collect();
        usable.sort_by(|a, b| {
            a.rtt_ms
                .partial_cmp(&b.rtt_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        usable
    }

    /// Overall peer health: Online if any path is active, Suspect if best is suspect, Down if all down.
    pub fn overall_status(&self) -> PeerOverallStatus {
        let statuses: Vec<PathHealthStatus> = self.paths.values().map(|s| s.status).collect();
        if statuses
            .iter()
            .any(|s| matches!(s, PathHealthStatus::Active))
        {
            PeerOverallStatus::Online
        } else if statuses
            .iter()
            .any(|s| matches!(s, PathHealthStatus::Suspect | PathHealthStatus::Recovering))
        {
            PeerOverallStatus::Suspect
        } else {
            PeerOverallStatus::Down
        }
    }

    /// Returns the adaptive heartbeat interval for this peer.
    ///
    /// Healthy peers: standard interval. Suspect peers: 2× frequency
    /// so we detect recovery faster. Down peers: 2× slower to avoid
    /// wasting bandwidth on a dead peer.
    pub fn adaptive_interval_ms(&self, base_interval_ms: u64) -> u64 {
        match self.overall_status() {
            PeerOverallStatus::Online => base_interval_ms,
            PeerOverallStatus::Suspect => base_interval_ms / 2,
            PeerOverallStatus::Down => base_interval_ms * 2,
        }
    }
}

/// Overall connectivity status derived from all paths to a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerOverallStatus {
    /// At least one path is actively passing heartbeats.
    Online,
    /// All paths are either suspect or recovering; no fully active path.
    Suspect,
    /// Every path has missed enough heartbeats to be Down.
    Down,
}

// ── HeartbeatProtocol ─────────────────────────────────────────────────────────

/// Manages heartbeat state for all peers in the mesh.
///
/// # Thread Safety
/// All state is protected by an `Arc<RwLock<>>` so that the heartbeat loop
/// (background tokio task) and the router (another task) can both access it.
pub struct HeartbeatProtocol {
    /// Base heartbeat interval copied from [`MeshConfig`](crate::mesh::config::MeshConfig).
    base_interval_ms: u64,
    /// Heartbeat timeout threshold (ms); peer is marked suspect after this.
    timeout_ms: u64,
    /// Per-peer state.
    peers: Arc<RwLock<HashMap<Uuid, PeerHeartbeats>>>,
}

impl HeartbeatProtocol {
    /// Create a new protocol with the given timing parameters.
    pub fn new(base_interval_ms: u64, timeout_ms: u64) -> Self {
        Self {
            base_interval_ms,
            timeout_ms,
            peers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new peer with the specified path names.
    pub async fn add_peer(&self, peer_id: Uuid, path_names: &[&str]) {
        let mut peers = self.peers.write().await;
        peers
            .entry(peer_id)
            .or_insert_with(|| PeerHeartbeats::new(peer_id, path_names));
    }

    /// Remove a peer from heartbeat tracking.
    pub async fn remove_peer(&self, peer_id: &Uuid) {
        self.peers.write().await.remove(peer_id);
    }

    /// Record that a heartbeat was sent on a specific path to a peer.
    pub async fn record_sent(&self, peer_id: &Uuid, path_name: &str) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            if let Some(path) = peer.paths.get_mut(path_name) {
                path.record_sent();
            }
        }
    }

    /// Record a pong received on a specific path; updates RTT.
    pub async fn record_received(&self, peer_id: &Uuid, path_name: &str, rtt_ms: f64) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            if let Some(path) = peer.paths.get_mut(path_name) {
                path.record_received(rtt_ms);
            }
        }
    }

    /// Mark a specific path as having missed a heartbeat.
    pub async fn mark_missed(&self, peer_id: &Uuid, path_name: &str) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            if let Some(path) = peer.paths.get_mut(path_name) {
                path.record_missed();
            }
        }
    }

    /// Explicitly transition a path to Suspect state.
    pub async fn mark_suspect(&self, peer_id: &Uuid, path_name: &str) {
        let mut peers = self.peers.write().await;
        if let Some(peer) = peers.get_mut(peer_id) {
            if let Some(path) = peer.paths.get_mut(path_name) {
                // Force the missed count to the Suspect threshold so that
                // subsequent missed() calls do not revert to Active.
                if path.missed_count < 3 {
                    path.missed_count = 3;
                }
                path.status = PathHealthStatus::Suspect;
            }
        }
    }

    /// Return the name of the best (lowest RTT) usable path for a peer.
    pub async fn best_path(&self, peer_id: &Uuid) -> Option<String> {
        let peers = self.peers.read().await;
        peers
            .get(peer_id)
            .and_then(|p| p.best_path())
            .map(|s| s.path_name.clone())
    }

    /// Return the overall status of a peer.
    pub async fn peer_status(&self, peer_id: &Uuid) -> Option<PeerOverallStatus> {
        let peers = self.peers.read().await;
        peers.get(peer_id).map(|p| p.overall_status())
    }

    /// Return the adaptive heartbeat interval for a peer.
    pub async fn interval_for_peer(&self, peer_id: &Uuid) -> u64 {
        let peers = self.peers.read().await;
        peers
            .get(peer_id)
            .map(|p| p.adaptive_interval_ms(self.base_interval_ms))
            .unwrap_or(self.base_interval_ms)
    }

    /// Scan all paths across all peers for stale heartbeats and mark missed.
    ///
    /// Called periodically by the background heartbeat loop.
    pub async fn tick_missed_detection(&self) {
        let timeout = Duration::from_millis(self.timeout_ms);
        let mut peers = self.peers.write().await;
        for peer in peers.values_mut() {
            for path in peer.paths.values_mut() {
                if !matches!(path.status, PathHealthStatus::Down) {
                    // If last_sent is more recent than timeout without a pong, record missed.
                    let time_since_sent = path.last_sent.elapsed();
                    let time_since_received = path
                        .last_received
                        .map(|t| t.elapsed())
                        .unwrap_or(Duration::from_secs(u64::MAX));
                    // Both conditions must hold: we sent a ping *and* have not
                    // received a pong since then — this avoids false positives
                    // during the initial registration window before any pings.
                    if time_since_sent > timeout && time_since_received > timeout {
                        path.record_missed();
                    }
                }
            }
        }
    }

    /// Return a snapshot of all peer IDs currently tracked.
    pub async fn peer_ids(&self) -> Vec<Uuid> {
        self.peers.read().await.keys().copied().collect()
    }

    /// Return a snapshot of heartbeat state for a specific peer.
    ///
    /// Each tuple is `(path_name, status, rtt_ms, missed_count)`.
    pub async fn snapshot(
        &self,
        peer_id: &Uuid,
    ) -> Option<Vec<(String, PathHealthStatus, f64, u32)>> {
        let peers = self.peers.read().await;
        peers.get(peer_id).map(|p| {
            p.paths
                .values()
                .map(|s| (s.path_name.clone(), s.status, s.rtt_ms, s.missed_count))
                .collect()
        })
    }

    /// Clone the inner Arc for sharing with background tasks.
    pub fn peers_handle(&self) -> Arc<RwLock<HashMap<Uuid, PeerHeartbeats>>> {
        Arc::clone(&self.peers)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_state_transitions() {
        let mut state = HeartbeatState::new("grpc");
        assert_eq!(state.status, PathHealthStatus::Active);

        // 3 missed → Suspect
        state.record_missed();
        state.record_missed();
        state.record_missed();
        assert_eq!(state.status, PathHealthStatus::Suspect);
        assert!(!state.is_usable());

        // 5 missed → Down
        state.record_missed();
        state.record_missed();
        assert_eq!(state.status, PathHealthStatus::Down);
        assert!(!state.is_usable());

        // Received pong → Recovering
        state.record_received(25.0);
        assert_eq!(state.status, PathHealthStatus::Recovering);
        assert!(state.is_usable());
    }

    #[test]
    fn rtt_ewma() {
        let mut state = HeartbeatState::new("quic");
        state.record_received(100.0);
        assert_eq!(state.rtt_ms, 100.0);
        state.record_received(50.0);
        // 0.8*100 + 0.2*50 = 90
        assert!((state.rtt_ms - 90.0).abs() < 0.01);
    }

    #[test]
    fn best_path_selection() {
        let peer_id = Uuid::new_v4();
        let mut peer = PeerHeartbeats::new(peer_id, &["grpc", "quic", "wss"]);

        // Give grpc high RTT, quic low RTT
        peer.paths.get_mut("grpc").unwrap().record_received(100.0);
        peer.paths.get_mut("quic").unwrap().record_received(10.0);
        peer.paths.get_mut("wss").unwrap().record_received(50.0);

        let best = peer.best_path().unwrap();
        assert_eq!(best.path_name, "quic");
    }

    #[test]
    fn overall_status_all_down() {
        let peer_id = Uuid::new_v4();
        let mut peer = PeerHeartbeats::new(peer_id, &["grpc", "quic"]);
        for _ in 0..5 {
            peer.paths.get_mut("grpc").unwrap().record_missed();
            peer.paths.get_mut("quic").unwrap().record_missed();
        }
        assert_eq!(peer.overall_status(), PeerOverallStatus::Down);
    }

    #[test]
    fn adaptive_interval_suspect() {
        let peer_id = Uuid::new_v4();
        let mut peer = PeerHeartbeats::new(peer_id, &["grpc"]);
        // Mark as suspect
        for _ in 0..3 {
            peer.paths.get_mut("grpc").unwrap().record_missed();
        }
        assert_eq!(peer.overall_status(), PeerOverallStatus::Suspect);
        // Should get 2x frequency (half the interval)
        assert_eq!(peer.adaptive_interval_ms(5000), 2500);
    }

    #[tokio::test]
    async fn protocol_add_and_best_path() {
        let proto = HeartbeatProtocol::new(5000, 15000);
        let peer_id = Uuid::new_v4();
        proto.add_peer(peer_id, &["grpc", "quic", "wss"]).await;

        proto.record_received(&peer_id, "grpc", 80.0).await;
        proto.record_received(&peer_id, "quic", 20.0).await;
        proto.record_received(&peer_id, "wss", 200.0).await;

        let best = proto.best_path(&peer_id).await.unwrap();
        assert_eq!(best, "quic");
    }

    #[tokio::test]
    async fn protocol_missed_marks_suspect() {
        let proto = HeartbeatProtocol::new(5000, 15000);
        let peer_id = Uuid::new_v4();
        proto.add_peer(peer_id, &["grpc"]).await;

        proto.mark_suspect(&peer_id, "grpc").await;
        let snap = proto.snapshot(&peer_id).await.unwrap();
        let (_, status, _, _) = &snap[0];
        assert_eq!(*status, PathHealthStatus::Suspect);
    }
}
