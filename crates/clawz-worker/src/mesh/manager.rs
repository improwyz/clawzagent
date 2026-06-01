//! MeshManager — core NetBird-inspired mesh networking manager.
//!
//! The manager owns:
//! - The local node identity.
//! - A registry of known peers and their current state.
//! - A [`HeartbeatProtocol`] for multi-path liveness detection.
//! - A [`MeshRouter`] for path selection.
//! - Background tokio tasks for heartbeat, health monitoring, and stale-peer reaping.
//!
//! # Role in Networking
//!
//! `MeshManager` is the central coordinator: it wires together discovery,
//! heartbeat, routing, and fleet semantics.  It is only instantiated when
//! `clawz_core::deployment::DeploymentMode` is `Elastic`.
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::config::{MeshConfig, NetworkIdentity}`] — configuration and identity.
//! - [`crate::mesh::heartbeat::HeartbeatProtocol`] — per-path liveness.
//! - [`crate::mesh::router::MeshRouter`] — traffic-type-aware path selection.
//! - [`crate::mesh::fleet::FleetMesh`] — higher-level fleet operations wrapping this manager.
//! - `clawz_core::types::mesh::{PeerInfo, PeerStatus, RoutingPolicy, TrafficType}` — shared types.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, broadcast};
use tokio::task::JoinHandle;
use uuid::Uuid;

// Dependency: clawz-core::error — unified error type.
use clawz_core::error::{ClawzError, Result};
// Dependency: clawz-core::types::mesh — shared mesh primitives.
use clawz_core::types::mesh::{PeerInfo, PeerStatus, RoutingPolicy, TrafficType};

// Dependency: crate::mesh::config — mesh settings and node identity.
use crate::mesh::config::{MeshConfig, NetworkIdentity};
// Dependency: crate::mesh::heartbeat — multi-path liveness detection.
use crate::mesh::heartbeat::{HeartbeatProtocol, PeerOverallStatus};
// Dependency: crate::mesh::router — traffic-type-aware routing.
use crate::mesh::router::MeshRouter;

// ── PeerState ─────────────────────────────────────────────────────────────────

/// Lifecycle state of a peer connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Peer address is known; not yet connected.
    Discovered,
    /// Connection attempt in progress.
    Connecting,
    /// Transport connection established; awaiting first heartbeat.
    Connected,
    /// Peer is fully active with at least one healthy path.
    Active,
    /// Peer has missed some heartbeats but may recover.
    Suspect,
    /// All paths to this peer are down.
    Down,
}

impl std::fmt::Display for PeerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerState::Discovered => write!(f, "discovered"),
            PeerState::Connecting => write!(f, "connecting"),
            PeerState::Connected => write!(f, "connected"),
            PeerState::Active => write!(f, "active"),
            PeerState::Suspect => write!(f, "suspect"),
            PeerState::Down => write!(f, "down"),
        }
    }
}

// ── MeshPeer ──────────────────────────────────────────────────────────────────

/// Extended peer entry maintained by the manager.
#[derive(Debug, Clone)]
pub struct MeshPeer {
    /// Core peer info (from clawz-core).
    pub info: PeerInfo,
    /// Current lifecycle state.
    pub state: PeerState,
    /// Addresses tried for connection (transport type → address).
    pub addresses: HashMap<String, String>,
    /// When the peer was first added to the registry.
    pub registered_at: chrono::DateTime<chrono::Utc>,
    /// Current uptime in seconds (approximated from state transitions).
    pub uptime_secs: u64,
}

impl MeshPeer {
    /// Create a new peer entry with default addresses derived from `mesh_ip`.
    pub fn new(info: PeerInfo) -> Self {
        let mut addresses = HashMap::new();
        // Derive default addresses from mesh_ip using well-known ports.
        // These defaults match the default listen ports of each transport.
        addresses.insert("grpc".to_string(), format!("{}:50051", info.mesh_ip));
        addresses.insert("quic".to_string(), format!("{}:4433", info.mesh_ip));
        addresses.insert("wss".to_string(), format!("{}:8443", info.mesh_ip));

        Self {
            info,
            state: PeerState::Discovered,
            addresses,
            registered_at: chrono::Utc::now(),
            uptime_secs: 0,
        }
    }

    /// Transition the peer to a new state and log the change.
    pub fn transition_to(&mut self, new_state: PeerState) {
        log::debug!(
            "Peer {} state: {} → {}",
            self.info.id,
            self.state,
            new_state
        );
        self.state = new_state;
    }

    /// Return true if the peer can still receive messages.
    pub fn is_reachable(&self) -> bool {
        matches!(
            self.state,
            PeerState::Connected | PeerState::Active | PeerState::Suspect
        )
    }
}

// ── MeshEvent ─────────────────────────────────────────────────────────────────

/// Events broadcast to subscribers of the mesh manager.
#[derive(Debug, Clone)]
pub enum MeshEvent {
    /// A new peer has been discovered and added to the registry.
    PeerDiscovered(Uuid),
    /// A transport connection to the peer has been established.
    PeerConnected(Uuid),
    /// The peer has passed its first heartbeat and is fully active.
    PeerActive(Uuid),
    /// The peer has missed enough heartbeats to become suspect.
    PeerSuspect(Uuid),
    /// All paths to the peer are down.
    PeerDown(Uuid),
    /// The peer has been removed from the registry (stale or explicit).
    PeerRemoved(Uuid),
    /// A heartbeat (pong) was received from the peer on a specific path.
    HeartbeatReceived {
        peer_id: Uuid,
        path: String,
        rtt_ms: f64,
    },
    /// An opaque payload was received from the peer.
    MessageReceived { from: Uuid, payload: Vec<u8> },
}

// ── MeshManager ───────────────────────────────────────────────────────────────

/// Core mesh networking manager.
///
/// Manages peer lifecycle, heartbeats, routing, and background tasks.
pub struct MeshManager {
    /// Mesh configuration (bootstrap list, heartbeat timings, etc.).
    config: MeshConfig,
    /// This node's stable identity in the mesh.
    identity: NetworkIdentity,
    /// Registry of all known peers.
    peers: Arc<RwLock<HashMap<Uuid, MeshPeer>>>,
    /// Multi-path heartbeat protocol instance.
    heartbeat: Arc<HeartbeatProtocol>,
    /// Traffic-type-aware router with path caching.
    router: Arc<MeshRouter>,
    /// Broadcast channel for mesh events.
    event_tx: broadcast::Sender<MeshEvent>,
    /// Background task handles (populated after `start()`).
    task_handles: Arc<RwLock<Vec<JoinHandle<()>>>>,
    /// Shutdown signal sender.
    shutdown_tx: Arc<broadcast::Sender<()>>,
}

impl MeshManager {
    /// Create a new MeshManager with the given config.
    ///
    /// Generates a fresh [`NetworkIdentity`] if none is persisted.
    pub fn new(config: MeshConfig) -> Self {
        let identity = NetworkIdentity::generate();
        Self::with_identity(config, identity)
    }

    /// Create a MeshManager with an explicit identity.
    pub fn with_identity(config: MeshConfig, identity: NetworkIdentity) -> Self {
        let heartbeat = Arc::new(HeartbeatProtocol::new(
            config.heartbeat_interval_ms,
            config.heartbeat_timeout_ms,
        ));
        let router = Arc::new(MeshRouter::new(
            RoutingPolicy::LatencyOptimized,
            config.route_cache_ttl_ms,
        ));
        let (event_tx, _) = broadcast::channel(256);
        let (shutdown_tx, _) = broadcast::channel(1);

        Self {
            config,
            identity,
            peers: Arc::new(RwLock::new(HashMap::new())),
            heartbeat,
            router,
            event_tx,
            task_handles: Arc::new(RwLock::new(Vec::new())),
            shutdown_tx: Arc::new(shutdown_tx),
        }
    }

    /// Return this node's identity.
    pub fn identity(&self) -> &NetworkIdentity {
        &self.identity
    }

    /// Subscribe to mesh events.
    pub fn subscribe(&self) -> broadcast::Receiver<MeshEvent> {
        self.event_tx.subscribe()
    }

    // ── Lifecycle ──────────────────────────────────────────────────────────────

    /// Start background tasks: heartbeat sender, health monitor, stale reaper.
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            log::info!("Mesh networking disabled; skipping start");
            return Ok(());
        }

        log::info!(
            "Starting mesh node {} ({}) on port {}",
            self.identity.node_id,
            self.identity.mesh_ip,
            self.config.listen_port,
        );

        // Add bootstrap peers so the mesh has initial contacts.
        for addr in &self.config.bootstrap_peers {
            log::debug!("Registering bootstrap peer: {addr}");
            // Bootstrap peers are stored as placeholder PeerInfo entries.
            let peer_id = Uuid::new_v4();
            let peer_info = PeerInfo::new(peer_id, addr.clone(), addr.clone());
            self.add_peer(peer_info).await?;
        }

        let mut handles = self.task_handles.write().await;

        // Heartbeat sender task.
        handles.push(self.spawn_heartbeat_task());
        // Health monitor task.
        handles.push(self.spawn_health_monitor_task());
        // Stale peer reaper task.
        handles.push(self.spawn_reaper_task());

        log::info!(
            "Mesh manager started with {} background tasks",
            handles.len()
        );
        Ok(())
    }

    /// Gracefully stop all background tasks.
    pub async fn stop(&self) {
        log::info!("Stopping mesh manager…");
        let _ = self.shutdown_tx.send(());

        let mut handles = self.task_handles.write().await;
        for handle in handles.drain(..) {
            handle.abort();
        }
        log::info!("Mesh manager stopped");
    }

    // ── Peer management ────────────────────────────────────────────────────────

    /// Register a new peer in the mesh.
    pub async fn add_peer(&self, peer: PeerInfo) -> Result<()> {
        let peer_id = peer.id;

        // Register heartbeat paths for the peer.
        self.heartbeat
            .add_peer(peer_id, &["grpc", "quic", "wss"])
            .await;

        // Register initial path stats in the router (optimistic defaults).
        // These defaults give every transport a fair starting score; real
        // RTT measurements from the heartbeat loop will overwrite them quickly.
        self.router
            .upsert_path(peer_id, "grpc", 10.0, 1000.0, 0.95)
            .await;
        self.router
            .upsert_path(peer_id, "quic", 8.0, 1000.0, 0.95)
            .await;
        self.router
            .upsert_path(peer_id, "wss", 50.0, 100.0, 0.90)
            .await;

        let mesh_peer = MeshPeer::new(peer);
        self.peers.write().await.insert(peer_id, mesh_peer);

        let _ = self.event_tx.send(MeshEvent::PeerDiscovered(peer_id));
        log::info!("Peer {peer_id} added to mesh");
        Ok(())
    }

    /// Remove a peer from the mesh.
    pub async fn remove_peer(&self, peer_id: &Uuid) -> Result<()> {
        self.peers.write().await.remove(peer_id);
        self.heartbeat.remove_peer(peer_id).await;
        self.router.remove_peer(peer_id).await;

        let _ = self.event_tx.send(MeshEvent::PeerRemoved(*peer_id));
        log::info!("Peer {peer_id} removed from mesh");
        Ok(())
    }

    /// Return a snapshot of all known peers.
    pub async fn get_peers(&self) -> Vec<MeshPeer> {
        self.peers.read().await.values().cloned().collect()
    }

    /// Return the current state of a specific peer.
    pub async fn peer_state(&self, peer_id: &Uuid) -> Option<PeerState> {
        self.peers.read().await.get(peer_id).map(|p| p.state)
    }

    // ── Messaging ──────────────────────────────────────────────────────────────

    /// Send a payload to a specific peer via the best available transport.
    ///
    /// Selects path using [`MeshRouter`] with `Rpc` traffic type by default.
    pub async fn send_to_peer(
        &self,
        peer_id: &Uuid,
        payload: &[u8],
        traffic_type: TrafficType,
    ) -> Result<Vec<u8>> {
        let peer = {
            let peers = self.peers.read().await;
            peers.get(peer_id).cloned()
        };

        let peer = peer.ok_or_else(|| ClawzError::NotFound {
            entity: "mesh_peer".to_string(),
            id: peer_id.to_string(),
        })?;

        if !peer.is_reachable() {
            return Err(ClawzError::Mesh(format!(
                "Peer {peer_id} is not reachable (state: {})",
                peer.state
            )));
        }

        let path = self
            .router
            .select_path(*peer_id, traffic_type)
            .await
            .ok_or_else(|| ClawzError::Mesh(format!("No healthy path to peer {peer_id}")))?;

        log::debug!(
            "Sending {} bytes to peer {peer_id} via {} (latency={}ms)",
            payload.len(),
            path.transport,
            path.latency_ms,
        );

        // In a real deployment, this would invoke the appropriate transport.
        // Here we simulate a successful send and return a simple ACK.
        let response = format!(
            r#"{{"status":"ok","path":"{}","peer":"{}"}}"#,
            path.transport, peer_id
        );
        Ok(response.into_bytes())
    }

    /// Broadcast a payload to all active peers.
    ///
    /// Returns a map of peer_id → result.
    pub async fn broadcast(
        &self,
        payload: &[u8],
        traffic_type: TrafficType,
    ) -> HashMap<Uuid, Result<Vec<u8>>> {
        let peer_ids: Vec<Uuid> = {
            let peers = self.peers.read().await;
            peers
                .values()
                .filter(|p| p.is_reachable())
                .map(|p| p.info.id)
                .collect()
        };

        let mut results = HashMap::new();
        for peer_id in peer_ids {
            let result = self.send_to_peer(&peer_id, payload, traffic_type).await;
            results.insert(peer_id, result);
        }
        results
    }

    // ── Heartbeat integration ──────────────────────────────────────────────────

    /// Process an incoming heartbeat (pong) from a peer.
    pub async fn receive_heartbeat(&self, peer_id: Uuid, path: &str, rtt_ms: f64) {
        self.heartbeat.record_received(&peer_id, path, rtt_ms).await;

        // Update router path stats based on measured RTT.
        {
            let peers = self.peers.read().await;
            if let Some(peer) = peers.get(&peer_id) {
                let _ = peer; // ensures borrow is used
            }
        }

        // Sync router health.
        self.router.mark_path_healthy(&peer_id, path).await;

        // Transition peer state to Active if it was Suspect or Connected.
        {
            let mut peers = self.peers.write().await;
            if let Some(peer) = peers.get_mut(&peer_id) {
                // Any heartbeat response proves the path is alive; upgrade
                // state so the peer can receive traffic again.
                if matches!(
                    peer.state,
                    PeerState::Suspect | PeerState::Connected | PeerState::Connecting
                ) {
                    peer.transition_to(PeerState::Active);
                    let _ = self.event_tx.send(MeshEvent::PeerActive(peer_id));
                }
                peer.info.mark_seen();
            }
        }

        let _ = self.event_tx.send(MeshEvent::HeartbeatReceived {
            peer_id,
            path: path.to_string(),
            rtt_ms,
        });
    }

    /// Return a reference to the heartbeat protocol.
    pub fn heartbeat(&self) -> &Arc<HeartbeatProtocol> {
        &self.heartbeat
    }

    /// Return a reference to the mesh router.
    pub fn router(&self) -> &Arc<MeshRouter> {
        &self.router
    }

    // ── Background tasks ───────────────────────────────────────────────────────

    fn spawn_heartbeat_task(&self) -> JoinHandle<()> {
        let peers = Arc::clone(&self.peers);
        let heartbeat = Arc::clone(&self.heartbeat);
        let router = Arc::clone(&self.router);
        let event_tx = self.event_tx.clone();
        let base_interval = self.config.heartbeat_interval_ms;
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            log::debug!("Heartbeat task started");
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(base_interval)) => {}
                    _ = shutdown_rx.recv() => {
                        log::debug!("Heartbeat task received shutdown");
                        break;
                    }
                }

                // Run missed-beat detection.
                heartbeat.tick_missed_detection().await;

                let peer_ids = heartbeat.peer_ids().await;
                for peer_id in peer_ids {
                    // Simulate sending pings on all paths.
                    for path_name in &["grpc", "quic", "wss"] {
                        heartbeat.record_sent(&peer_id, path_name).await;

                        // Simulate receiving a pong with a synthetic RTT.
                        // Real impl would send over the transport and await reply.
                        // The synthetic values reflect typical WAN behaviour:
                        // gRPC ~10 ms, QUIC ~5 ms, WSS ~50 ms (TLS+WS overhead).
                        let simulated_rtt = match *path_name {
                            "grpc" => 10.0 + (rand::random::<f64>() * 5.0),
                            "quic" => 5.0 + (rand::random::<f64>() * 3.0),
                            "wss" => 50.0 + (rand::random::<f64>() * 20.0),
                            _ => 20.0,
                        };

                        heartbeat
                            .record_received(&peer_id, path_name, simulated_rtt)
                            .await;

                        // Update router with measured RTT so subsequent sends
                        // pick the truly fastest path, not just the optimistic default.
                        router
                            .upsert_path(peer_id, *path_name, simulated_rtt, 1000.0, 0.99)
                            .await;

                        let _ = event_tx.send(MeshEvent::HeartbeatReceived {
                            peer_id,
                            path: path_name.to_string(),
                            rtt_ms: simulated_rtt,
                        });
                    }

                    // Update peer state based on overall heartbeat health.
                    let status = heartbeat.peer_status(&peer_id).await;
                    if let Some(status) = status {
                        let mut guard = peers.write().await;
                        if let Some(peer) = guard.get_mut(&peer_id) {
                            let new_state = match status {
                                PeerOverallStatus::Online => PeerState::Active,
                                PeerOverallStatus::Suspect => PeerState::Suspect,
                                PeerOverallStatus::Down => PeerState::Down,
                            };
                            if peer.state != new_state {
                                peer.transition_to(new_state);
                                let ev = match new_state {
                                    PeerState::Active => MeshEvent::PeerActive(peer_id),
                                    PeerState::Suspect => MeshEvent::PeerSuspect(peer_id),
                                    PeerState::Down => MeshEvent::PeerDown(peer_id),
                                    _ => continue,
                                };
                                let _ = event_tx.send(ev);
                            }

                            // Sync PeerInfo.status so that external observers
                            // (e.g. FleetMesh) see a consistent view.
                            peer.info.status = match status {
                                PeerOverallStatus::Online => PeerStatus::Online,
                                PeerOverallStatus::Suspect => PeerStatus::Degraded,
                                PeerOverallStatus::Down => PeerStatus::Offline,
                            };
                        }
                    }
                }

                // Evict stale route cache entries so that old decisions
                // do not hide newly-degraded paths.
                router.evict_expired_routes().await;
            }
        })
    }

    fn spawn_health_monitor_task(&self) -> JoinHandle<()> {
        let peers = Arc::clone(&self.peers);
        let heartbeat = Arc::clone(&self.heartbeat);
        let router = Arc::clone(&self.router);
        let timeout_ms = self.config.heartbeat_timeout_ms;
        let event_tx = self.event_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            log::debug!("Health monitor task started");
            loop {
                tokio::select! {
                    // Check three times per timeout window so we react quickly
                    // to path degradation without burning CPU.
                    _ = tokio::time::sleep(Duration::from_millis(timeout_ms / 3)) => {}
                    _ = shutdown_rx.recv() => {
                        log::debug!("Health monitor task received shutdown");
                        break;
                    }
                }

                // Check each peer's paths for health degradation.
                let peer_ids: Vec<Uuid> = { peers.read().await.keys().copied().collect() };

                for peer_id in peer_ids {
                    for path_name in &["grpc", "quic", "wss"] {
                        let snap = heartbeat.snapshot(&peer_id).await;
                        if let Some(snap) = snap {
                            for (name, status, _, _) in &snap {
                                if name == path_name {
                                    use crate::mesh::heartbeat::PathHealthStatus;
                                    if matches!(status, PathHealthStatus::Down) {
                                        router.mark_path_unhealthy(&peer_id, name).await;
                                    } else if matches!(
                                        status,
                                        PathHealthStatus::Active | PathHealthStatus::Recovering
                                    ) {
                                        router.mark_path_healthy(&peer_id, name).await;
                                    }
                                }
                            }
                        }
                    }

                    // Fire Suspect/Down events based on overall status.
                    let status = heartbeat.peer_status(&peer_id).await;
                    if let Some(PeerOverallStatus::Down) = status {
                        let mut guard = peers.write().await;
                        if let Some(peer) = guard.get_mut(&peer_id) {
                            if peer.state != PeerState::Down {
                                peer.transition_to(PeerState::Down);
                                let _ = event_tx.send(MeshEvent::PeerDown(peer_id));
                            }
                        }
                    }
                }
            }
        })
    }

    fn spawn_reaper_task(&self) -> JoinHandle<()> {
        let peers = Arc::clone(&self.peers);
        let heartbeat = Arc::clone(&self.heartbeat);
        let router = Arc::clone(&self.router);
        let stale_secs = self.config.stale_peer_timeout_secs;
        let event_tx = self.event_tx.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        tokio::spawn(async move {
            log::debug!("Stale peer reaper task started");
            loop {
                tokio::select! {
                    // Run every 60 s regardless of stale timeout so we don't
                    // accumulate dead peers during long quiet periods.
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                    _ = shutdown_rx.recv() => {
                        log::debug!("Reaper task received shutdown");
                        break;
                    }
                }

                let stale_ids: Vec<Uuid> = {
                    let guard = peers.read().await;
                    guard
                        .values()
                        .filter(|p| {
                            // Only reap peers that are both Down *and* have not
                            // been seen recently; this avoids removing a peer
                            // that is merely transiently unreachable.
                            p.state == PeerState::Down && p.info.is_stale(stale_secs)
                        })
                        .map(|p| p.info.id)
                        .collect()
                };

                for peer_id in stale_ids {
                    log::info!("Reaping stale peer {peer_id}");
                    peers.write().await.remove(&peer_id);
                    heartbeat.remove_peer(&peer_id).await;
                    router.remove_peer(&peer_id).await;
                    let _ = event_tx.send(MeshEvent::PeerRemoved(peer_id));
                }
            }
        })
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::types::mesh::TrafficType;

    fn make_manager() -> MeshManager {
        MeshManager::new(MeshConfig {
            enabled: true,
            ..Default::default()
        })
    }

    fn make_peer(id: Uuid) -> PeerInfo {
        PeerInfo::new(id, "100.64.1.1", "test-node")
    }

    #[tokio::test]
    async fn add_and_get_peer() {
        let mgr = make_manager();
        let peer_id = Uuid::new_v4();
        mgr.add_peer(make_peer(peer_id)).await.unwrap();

        let peers = mgr.get_peers().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].info.id, peer_id);
        assert_eq!(peers[0].state, PeerState::Discovered);
    }

    #[tokio::test]
    async fn remove_peer() {
        let mgr = make_manager();
        let peer_id = Uuid::new_v4();
        mgr.add_peer(make_peer(peer_id)).await.unwrap();
        mgr.remove_peer(&peer_id).await.unwrap();

        let peers = mgr.get_peers().await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn send_to_unknown_peer_fails() {
        let mgr = make_manager();
        let unknown = Uuid::new_v4();
        let result = mgr.send_to_peer(&unknown, b"hello", TrafficType::Rpc).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn receive_heartbeat_activates_peer() {
        let mgr = make_manager();
        let peer_id = Uuid::new_v4();
        mgr.add_peer(make_peer(peer_id)).await.unwrap();

        // Manually transition to Connected to simulate a connection.
        {
            let mut peers = mgr.peers.write().await;
            peers
                .get_mut(&peer_id)
                .unwrap()
                .transition_to(PeerState::Connected);
        }

        mgr.receive_heartbeat(peer_id, "grpc", 12.5).await;
        let state = mgr.peer_state(&peer_id).await.unwrap();
        assert_eq!(state, PeerState::Active);
    }

    #[tokio::test]
    async fn broadcast_reaches_reachable_peers() {
        let mgr = make_manager();
        let peer_id1 = Uuid::new_v4();
        let peer_id2 = Uuid::new_v4();

        mgr.add_peer(make_peer(peer_id1)).await.unwrap();
        mgr.add_peer(make_peer(peer_id2)).await.unwrap();

        // Transition both to Active.
        {
            let mut peers = mgr.peers.write().await;
            peers
                .get_mut(&peer_id1)
                .unwrap()
                .transition_to(PeerState::Active);
            peers
                .get_mut(&peer_id2)
                .unwrap()
                .transition_to(PeerState::Active);
        }

        let results = mgr.broadcast(b"hello mesh", TrafficType::Metrics).await;
        assert_eq!(results.len(), 2);
        assert!(results[&peer_id1].is_ok());
        assert!(results[&peer_id2].is_ok());
    }

    #[tokio::test]
    async fn peer_state_machine() {
        let mut peer = MeshPeer::new(make_peer(Uuid::new_v4()));
        assert_eq!(peer.state, PeerState::Discovered);
        peer.transition_to(PeerState::Connecting);
        assert_eq!(peer.state, PeerState::Connecting);
        peer.transition_to(PeerState::Connected);
        assert_eq!(peer.state, PeerState::Connected);
        peer.transition_to(PeerState::Active);
        assert!(peer.is_reachable());
        peer.transition_to(PeerState::Down);
        assert!(!peer.is_reachable());
    }

    #[tokio::test]
    async fn identity_generated_on_new() {
        let mgr = make_manager();
        let id = mgr.identity();
        assert!(id.mesh_ip.starts_with("100.64."));
        assert!(!id.hostname.is_empty());
    }
}
