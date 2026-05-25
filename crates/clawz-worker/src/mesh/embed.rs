//! Embedding / vector-aware mesh operations.
//!
//! This module provides a simplified [`MeshManager`] used when the ClawZ
//! worker embeds a lightweight mesh stack directly (e.g. for local dev
//! or single-node deployments).  It re-uses the same [`MeshConfig`] and
//! [`MeshRouter`] types as the full mesh but drops the multi-peer
//! orchestration in favour of a local-only peer registry.
//!
//! # Role in Networking
//!
//! In **Standalone** mode the real mesh is disabled; this module lets
//! the worker still answer "who are my peers?" queries with a local Vec
//! so that higher-level code does not need two code paths.
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::config::MeshConfig`] — shared configuration struct.
//! - [`crate::mesh::heartbeat::HeartbeatProtocol`] — heartbeat logic reused here.
//! - [`crate::mesh::router::MeshRouter`] — routing table reused here.

// Dependency: crate::mesh::config — configuration primitives shared with full mesh.
// Dependency: crate::mesh::heartbeat — heartbeat state machine reused.
// Dependency: crate::mesh::router — route selection reused.

use crate::mesh::config::{HeartbeatConfig, MeshConfig, RoutingConfig};
use crate::mesh::heartbeat::HeartbeatProtocol;
use crate::mesh::router::MeshRouter;
use clawz_core::error::ClawzError;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Simplified peer entry for the embedded mesh registry.
///
/// Unlike the full [`crate::mesh::manager::MeshPeer`], this struct stores
/// only the fields needed for local display and basic health tracking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshPeer {
    /// Opaque peer identifier (may be a UUID string or synthetic ID).
    pub id: String,
    /// Network address or mesh IP of the peer.
    pub ip: String,
    /// Current health state derived from heartbeat state.
    pub state: PeerState,
    /// UTC timestamp of the last received heartbeat.
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}

/// Health states for an embedded mesh peer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PeerState {
    /// Peer is responding to heartbeats normally.
    Active,
    /// Peer missed at least one heartbeat window but may recover.
    Suspect,
    /// Peer has missed enough heartbeats to be considered unreachable.
    Down,
}

/// Lightweight mesh manager for embedded / standalone deployments.
///
/// Keeps a local [`Vec`] of peers rather than the full HashMap+background-task
/// machinery used by [`crate::mesh::manager::MeshManager`].
pub struct MeshManager {
    config: MeshConfig,
    /// Heartbeat protocol instance (reused from the full mesh stack).
    heartbeat: HeartbeatProtocol,
    /// Router instance for local route queries.
    router: MeshRouter,
    /// Local peer registry — no background reaper, caller must prune.
    peers: Arc<RwLock<Vec<MeshPeer>>>,
    /// Mesh IP assigned to this node (placeholder until real allocation).
    mesh_ip: Option<String>,
}

impl MeshManager {
    /// Create a new embedded mesh manager with the given configuration.
    pub fn new(config: MeshConfig) -> Self {
        Self {
            config,
            heartbeat: HeartbeatProtocol::new(HeartbeatConfig::default()),
            router: MeshRouter::new(RoutingConfig::default()),
            peers: Arc::new(RwLock::new(Vec::new())),
            mesh_ip: None,
        }
    }

    /// Initialise the mesh stack.
    ///
    /// In production this would start the embedded NetBird FFI layer.
    /// For now it simply records a placeholder mesh IP.
    pub async fn start(&mut self) -> Result<(), ClawzError> {
        if !self.config.enabled {
            return Ok(());
        }

        // In production: initialize NetBird via FFI
        // netbird-embed would be started here
        
        // Placeholder until real mesh IP allocation is wired up.
        self.mesh_ip = Some("100.64.0.1".to_string());

        Ok(())
    }

    /// Shut down the mesh stack.
    ///
    /// In production this would cleanly stop the embedded NetBird layer.
    pub async fn stop(&self) -> Result<(), ClawzError> {
        // In production: shutdown NetBird
        Ok(())
    }

    /// Return a snapshot of the current peer list.
    pub async fn get_peers(&self) -> Vec<MeshPeer> {
        self.peers.read().await.clone()
    }

    /// Add a peer to the local registry.
    pub async fn add_peer(&self, peer: MeshPeer) {
        self.peers.write().await.push(peer);
    }

    /// Return the mesh IP assigned to this node, if any.
    pub fn get_mesh_ip(&self) -> Option<&str> {
        self.mesh_ip.as_deref()
    }

    /// Return a reference to the local router.
    pub fn router(&self) -> &MeshRouter {
        &self.router
    }
}
