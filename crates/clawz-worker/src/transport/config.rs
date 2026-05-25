//! Transport configuration — loaded from `provider.toml` or programmatically.
//!
//! The top-level [`TransportConfig`] determines which transport to use and
//! carries per-transport settings.  All structs derive `serde::Deserialize`
//! so they can be populated from TOML.
//!
//! # Role in Networking
//!
//! These structs are consumed by [`crate::transport::create_transport`] to
//! instantiate the concrete transport(s) used for inter-peer communication.
//!
//! # Key Dependencies
//!
//! - [`crate::transport::create_transport`] — primary consumer of these configs.
//! - `clawz_core::traits::Transport` — traits that the configured transports implement.

use serde::{Deserialize, Serialize};

// ── Mode ──────────────────────────────────────────────────────────────────────

/// Which transport to activate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    /// Automatically pick the best transport for each peer.
    #[default]
    Auto,
    /// Force TCP binary-framing RPC (LAN-optimised).
    Grpc,
    /// Force QUIC (preferred remote transport).
    Quic,
    /// Force encrypted WebSocket (WSS fallback).
    Wss,
    /// In-process channel — no networking, same binary.
    InProcess,
}

// ── Per-transport configs ──────────────────────────────────────────────────────

/// Settings for the TCP/binary-framing (gRPC-compatible) transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    /// Port to listen on when acting as a server.
    pub listen_port: u16,
    /// Maximum number of pooled connections per peer.
    pub pool_size: usize,
    /// Idle connection timeout in seconds.
    pub idle_timeout_secs: u64,
    /// Whether to wrap TCP in TLS.
    pub tls: bool,
    /// Path to a PEM-encoded certificate file (server mode).
    pub cert_path: Option<String>,
    /// Path to a PEM-encoded private key file (server mode).
    pub key_path: Option<String>,
    /// Connect timeout in milliseconds.
    pub connect_timeout_ms: u64,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            listen_port: 50051,
            // 8 connections is a sweet spot for LAN: enough for pipelining
            // without exhausting ephemeral ports on the client side.
            pool_size: 8,
            idle_timeout_secs: 60,
            tls: false,
            cert_path: None,
            key_path: None,
            connect_timeout_ms: 5_000,
            request_timeout_ms: 30_000,
        }
    }
}

/// Settings for the QUIC transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuicConfig {
    /// UDP port to listen on.
    pub listen_port: u16,
    /// Whether to enable 0-RTT (reduces latency, slight security trade-off).
    pub enable_0rtt: bool,
    /// Maximum bi-directional streams per connection.
    pub max_streams: u32,
    /// Path to a PEM-encoded certificate file; `None` => auto-generate self-signed.
    pub cert_path: Option<String>,
    /// Path to a PEM-encoded private key file.
    pub key_path: Option<String>,
    /// Connect timeout in milliseconds.
    pub connect_timeout_ms: u64,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Maximum idle timeout for a QUIC connection in milliseconds.
    pub max_idle_timeout_ms: u64,
}

impl Default for QuicConfig {
    fn default() -> Self {
        Self {
            listen_port: 4433,
            // 0-RTT is enabled by default because in a private mesh the replay
            // risk is acceptable and the latency win is significant.
            enable_0rtt: true,
            max_streams: 100,
            cert_path: None,
            key_path: None,
            connect_timeout_ms: 5_000,
            request_timeout_ms: 30_000,
            max_idle_timeout_ms: 30_000,
        }
    }
}

/// Settings for the WebSocket (WSS) transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WssConfig {
    /// Port to listen on.
    pub listen_port: u16,
    /// Base URL used when connecting as a client, e.g. `wss://relay.example.com`.
    pub gateway_url: Option<String>,
    /// JWT secret for signing/verifying upgrade-header tokens.
    pub jwt_secret: Option<String>,
    /// Initial reconnect delay in milliseconds.
    pub reconnect_base_ms: u64,
    /// Maximum reconnect delay in milliseconds.
    pub reconnect_max_ms: u64,
    /// Maximum number of reconnect attempts (0 = unlimited).
    pub reconnect_max_attempts: u32,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
}

impl Default for WssConfig {
    fn default() -> Self {
        Self {
            listen_port: 8443,
            gateway_url: None,
            jwt_secret: None,
            // 500 ms base delay: fast retry for transient failures without
            // hammering the server during an outage.
            reconnect_base_ms: 500,
            reconnect_max_ms: 30_000,
            // 0 means unlimited retries: WSS is often the last-resort
            // transport, so we keep trying until the peer comes back.
            reconnect_max_attempts: 0,
            request_timeout_ms: 30_000,
        }
    }
}

// ── Top-level config ──────────────────────────────────────────────────────────

/// Root transport configuration block.
///
/// ```toml
/// [transport]
/// mode = "auto"
///
/// [transport.grpc]
/// listen_port = 50051
/// pool_size = 4
///
/// [transport.quic]
/// listen_port = 4433
/// enable_0rtt = true
///
/// [transport.wss]
/// gateway_url = "wss://relay.example.com"
/// jwt_secret = "super-secret"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TransportConfig {
    /// Which transport(s) to use.
    pub mode: TransportMode,
    /// gRPC / TCP-framing config.
    pub grpc: GrpcConfig,
    /// QUIC config.
    pub quic: QuicConfig,
    /// WebSocket config.
    pub wss: WssConfig,
}

impl TransportConfig {
    /// Load from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_parse_from_empty_toml() {
        let cfg: TransportConfig = toml::from_str("").unwrap();
        assert_eq!(cfg.mode, TransportMode::Auto);
        assert_eq!(cfg.grpc.listen_port, 50051);
        assert_eq!(cfg.quic.listen_port, 4433);
        assert_eq!(cfg.wss.listen_port, 8443);
    }

    #[test]
    fn mode_deserializes() {
        let s = r#"mode = "quic""#;
        let cfg: TransportConfig = toml::from_str(s).unwrap();
        assert_eq!(cfg.mode, TransportMode::Quic);
    }

    #[test]
    fn nested_overrides() {
        let s = r#"
[grpc]
listen_port = 9090
pool_size = 2

[quic]
enable_0rtt = false
"#;
        let cfg: TransportConfig = toml::from_str(s).unwrap();
        assert_eq!(cfg.grpc.listen_port, 9090);
        assert_eq!(cfg.grpc.pool_size, 2);
        assert!(!cfg.quic.enable_0rtt);
    }
}
