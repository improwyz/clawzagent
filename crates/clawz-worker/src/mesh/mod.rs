//! Mesh networking module — NetBird-inspired multi-path peer mesh.
//!
//! Provides no-single-point-of-failure communication between ClawZ nodes
//! by maintaining simultaneous gRPC, QUIC, and WSS paths per peer and
//! running heartbeats on all paths concurrently.
//!
//! This module is only active in **Elastic** deployment mode.
//! In Standalone mode the in-process transport is used instead.
//!
//! # Architecture
//!
//! ```text
//! FleetMesh
//!   └── MeshManager
//!         ├── PeerDiscovery  (static + mDNS + API)
//!         ├── HeartbeatProtocol (per-peer, per-path)
//!         └── MeshRouter    (traffic-type-aware path selection)
//! ```
//!
//! # Key Dependencies
//!
//! - [`crate::transport`] — actual message delivery via selected transport.
//! - `clawz_core::types::mesh` — shared mesh types (PeerInfo, TrafficType, etc.).
//! - `clawz_core::deployment::DeploymentMode` — mesh is elastic-mode only.

// Dependency: clawz-core::types::mesh — shared mesh primitives.
// Dependency: crate::transport — transport layer for actual packet delivery.

pub mod config;
pub mod discovery;
pub mod firewall;
pub mod fleet;
pub mod heartbeat;
pub mod manager;
pub mod router;

pub use config::{MeshConfig, NetworkIdentity};
pub use manager::{MeshManager, MeshPeer, PeerState};
pub use heartbeat::{HeartbeatProtocol, HeartbeatState, PathHealthStatus};
pub use router::{MeshRouter, RouteEntry};
pub use fleet::{FleetMesh, AgentLocation, LeaderElectionResult};
pub use discovery::{PeerDiscovery, DiscoveryMethod};
