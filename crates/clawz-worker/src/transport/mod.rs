//! Transport layer for clawz-worker.
//!
//! Provides four concrete transport implementations plus an intelligent
//! selector that automatically picks the best one based on network conditions:
//!
//! | Transport   | Best for            | Protocol                        |
//! |-------------|---------------------|---------------------------------|
//! | `grpc`      | LAN peers           | TCP length-prefixed frames      |
//! | `quic`      | Remote peers        | QUIC / UDP + TLS 1.3            |
//! | `wss`       | Firewall fallback   | WebSocket binary frames         |
//! | `in_process`| Single-binary mode  | tokio mpsc channels             |
//!
//! # Role in Networking
//!
//! The transport layer is the bottom of the ClawZ networking stack.
//! It is used directly by [`crate::mesh::manager::MeshManager`] to send
//! payloads to peers and by [`crate::mesh::router::MeshRouter`] to
//! deliver messages on the selected path.
//!
//! # Quick start
//!
//! ```rust,ignore
//! use clawz_worker::transport::{create_transport, config::TransportConfig};
//!
//! let config = TransportConfig::default(); // Auto mode
//! let transport = create_transport(&config)?;
//!
//! // Send a payload to a peer — selector picks the right transport.
//! let response = transport.send(&peer_info, &payload).await?;
//! ```
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportListener}` — core traits implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
// Dependency: clawz-core::types::mesh::PeerInfo — peer address descriptor.

pub mod config;
pub mod grpc;
pub mod in_process;
pub mod quic;
pub mod selector;
pub mod wss;

pub use config::{GrpcConfig, QuicConfig, TransportConfig, TransportMode, WssConfig};
pub use grpc::GrpcTransport;
pub use in_process::InProcessTransport;
pub use quic::QuicTransport;
pub use selector::{HealthInfo, TransportSelector};
pub use wss::WssTransport;

use clawz_core::{error::Result, traits::Transport};

// ── TransportType ─────────────────────────────────────────────────────────────

/// Discriminant enum used by the selector and health tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportType {
    /// TCP binary-framing (gRPC-compatible).
    Grpc,
    /// QUIC over UDP with TLS 1.3.
    Quic,
    /// Encrypted WebSocket binary frames.
    Wss,
    /// In-process tokio mpsc channels (no network).
    InProcess,
}

impl std::fmt::Display for TransportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportType::Grpc => write!(f, "grpc"),
            TransportType::Quic => write!(f, "quic"),
            TransportType::Wss => write!(f, "wss"),
            TransportType::InProcess => write!(f, "in-process"),
        }
    }
}

// ── Factory ───────────────────────────────────────────────────────────────────

/// Create a boxed [`Transport`] from a [`TransportConfig`].
///
/// * `Auto` → returns a [`TransportSelector`] that wraps all four transports.
/// * `Grpc` → returns a [`GrpcTransport`].
/// * `Quic` → returns a [`QuicTransport`].
/// * `Wss` → returns a [`WssTransport`].
/// * `InProcess` → returns an [`InProcessTransport`].
pub fn create_transport(config: &TransportConfig) -> Result<Box<dyn Transport>> {
    match config.mode {
        TransportMode::Auto => Ok(Box::new(TransportSelector::from_config(config)?)),
        TransportMode::Grpc => Ok(Box::new(GrpcTransport::new(config.grpc.clone()))),
        TransportMode::Quic => Ok(Box::new(QuicTransport::new(config.quic.clone())?)),
        TransportMode::Wss => Ok(Box::new(WssTransport::new(config.wss.clone()))),
        TransportMode::InProcess => Ok(Box::new(InProcessTransport::new(
            config.wss.request_timeout_ms,
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_type_display() {
        assert_eq!(TransportType::Grpc.to_string(), "grpc");
        assert_eq!(TransportType::Quic.to_string(), "quic");
        assert_eq!(TransportType::Wss.to_string(), "wss");
        assert_eq!(TransportType::InProcess.to_string(), "in-process");
    }

    #[tokio::test]
    async fn factory_grpc() {
        let config = TransportConfig {
            mode: TransportMode::Grpc,
            ..Default::default()
        };
        let transport = create_transport(&config).unwrap();
        assert_eq!(transport.name(), "grpc-tcp");
    }

    #[tokio::test]
    async fn factory_quic() {
        let config = TransportConfig {
            mode: TransportMode::Quic,
            ..Default::default()
        };
        let transport = create_transport(&config).unwrap();
        assert_eq!(transport.name(), "quic");
    }

    #[test]
    fn factory_wss() {
        let config = TransportConfig {
            mode: TransportMode::Wss,
            ..Default::default()
        };
        let transport = create_transport(&config).unwrap();
        assert_eq!(transport.name(), "wss");
    }

    #[test]
    fn factory_in_process() {
        let config = TransportConfig {
            mode: TransportMode::InProcess,
            ..Default::default()
        };
        let transport = create_transport(&config).unwrap();
        assert_eq!(transport.name(), "in-process");
    }

    #[tokio::test]
    async fn factory_auto() {
        let config = TransportConfig::default(); // mode = Auto
        let transport = create_transport(&config).unwrap();
        assert_eq!(transport.name(), "selector");
    }

    #[tokio::test]
    async fn factory_supports_streaming_auto() {
        let config = TransportConfig::default();
        let transport = create_transport(&config).unwrap();
        assert!(transport.supports_streaming());
    }
}
