//! Transport selector — picks the best transport for each peer.
//!
//! The [`TransportSelector`] wraps all available transports and chooses
//! which one to use for a given [`PeerInfo`] based on:
//!
//! 1. **Configured mode** — if the config says `grpc`, always use gRPC.
//! 2. **Network topology** — if the peer is on the same /24 subnet, prefer
//!    the gRPC (TCP) transport (low latency, no UDP NAT issues).
//! 3. **Error-rate health** — if the current preferred transport has a high
//!    error rate, fall back to the next one.
//! 4. **Latency tracking** — exponentially-weighted moving average per
//!    transport per peer.
//!
//! # Role in Networking
//!
//! The selector is the default transport when `TransportMode::Auto` is
//! configured.  It delegates to the concrete transports (gRPC, QUIC, WSS,
//! in-process) and records health statistics so that future calls avoid
//! transports that are currently failing.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportConnection, TransportListener}` — implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.
//! - [`crate::transport::config::{TransportConfig, TransportMode}`] — selector behaviour.
//! - [`crate::transport::grpc::GrpcTransport`] — LAN-optimised transport.
//! - [`crate::transport::quic::QuicTransport`] — remote-optimised transport.
//! - [`crate::transport::wss::WssTransport`] — firewall-friendly fallback.
//! - [`crate::transport::in_process::InProcessTransport`] — same-binary transport.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
use clawz_core::{
    error::Result,
    traits::{Transport, TransportConnection, TransportListener},
    types::mesh::PeerInfo,
};
use tokio::sync::RwLock;

// Dependency: crate::transport::config — mode and per-transport settings.
use crate::transport::{
    TransportType,
    config::{TransportConfig, TransportMode},
    grpc::GrpcTransport,
    in_process::InProcessTransport,
    quic::QuicTransport,
    wss::WssTransport,
};

// ── Health tracking ───────────────────────────────────────────────────────────

/// Rolling statistics for a single transport×peer combination.
#[derive(Debug, Clone)]
struct TransportStats {
    /// Exponential moving average of latency in milliseconds.
    pub latency_ms_ema: f64,
    /// Number of consecutive errors.
    pub consecutive_errors: u32,
    /// Total requests sent.
    pub total_requests: u64,
    /// Total errors.
    pub total_errors: u64,
    /// Last request time.
    pub last_request: Option<Instant>,
}

impl TransportStats {
    fn new() -> Self {
        Self {
            latency_ms_ema: 0.0,
            consecutive_errors: 0,
            total_requests: 0,
            total_errors: 0,
            last_request: None,
        }
    }

    /// EMA smoothing factor (0 < α ≤ 1). Higher = more weight on recent samples.
    /// 0.2 was chosen to be responsive to spikes without overreacting to outliers.
    const ALPHA: f64 = 0.2;

    /// Record a successful request with the given latency.
    fn record_success(&mut self, latency: Duration) {
        let ms = latency.as_secs_f64() * 1_000.0;
        if self.total_requests == 0 {
            self.latency_ms_ema = ms;
        } else {
            self.latency_ms_ema = Self::ALPHA * ms + (1.0 - Self::ALPHA) * self.latency_ms_ema;
        }
        self.consecutive_errors = 0;
        self.total_requests += 1;
        self.last_request = Some(Instant::now());
    }

    /// Record a failed request.
    fn record_error(&mut self) {
        self.consecutive_errors += 1;
        self.total_requests += 1;
        self.total_errors += 1;
        self.last_request = Some(Instant::now());
    }

    /// Error rate as a fraction [0, 1].
    fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            0.0
        } else {
            self.total_errors as f64 / self.total_requests as f64
        }
    }

    /// Returns true when this transport should be considered unhealthy.
    fn is_unhealthy(&self) -> bool {
        // Fail-fast: 3+ consecutive errors, or >50% error rate with ≥10 requests.
        // These thresholds balance quick detection of broken transports against
        // false positives from transient network blips.
        self.consecutive_errors >= 3 || (self.total_requests >= 10 && self.error_rate() > 0.5)
    }
}

// ── Selector ──────────────────────────────────────────────────────────────────

type StatsMap = HashMap<(String, TransportType), TransportStats>;

/// Intelligent transport selector that picks and health-tracks transports.
pub struct TransportSelector {
    /// Effective mode (Auto or forced transport).
    mode: TransportMode,
    /// LAN-optimised TCP transport.
    grpc: Arc<GrpcTransport>,
    /// Remote-optimised QUIC transport.
    quic: Arc<QuicTransport>,
    /// Firewall-friendly WebSocket transport.
    wss: Arc<WssTransport>,
    /// Same-binary in-process transport.
    in_process: Arc<InProcessTransport>,
    /// Stats keyed by (peer_id_string, TransportType).
    stats: Arc<RwLock<StatsMap>>,
}

impl TransportSelector {
    /// Build a selector from the full transport config.
    pub fn from_config(config: &TransportConfig) -> Result<Self> {
        Ok(Self {
            mode: config.mode.clone(),
            grpc: Arc::new(GrpcTransport::new(config.grpc.clone())),
            quic: Arc::new(QuicTransport::new(config.quic.clone())?),
            wss: Arc::new(WssTransport::new(config.wss.clone())),
            in_process: Arc::new(InProcessTransport::new(
                config.wss.request_timeout_ms, // reuse WSS timeout as a sensible default
            )),
            stats: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    // ── Network topology helpers ──────────────────────────────────────────────

    /// Returns true if `peer_ip` appears to be on the same /24 as our local
    /// interface addresses.
    ///
    /// We use a conservative heuristic: if the peer is in RFC-1918 space
    /// (10.x, 172.16-31.x, 192.168.x) we treat it as LAN.  This avoids
    /// false positives for public IPs that happen to share a /24 with us.
    fn is_lan_peer(peer_ip: &str) -> bool {
        let peer: IpAddr = match peer_ip.parse() {
            Ok(ip) => ip,
            Err(_) => return false,
        };

        match peer {
            IpAddr::V4(peer_v4) => {
                let octets = peer_v4.octets();
                // RFC-1918 ranges: 10.x.x.x, 172.16-31.x.x, 192.168.x.x

                octets[0] == 10
                    || (octets[0] == 172 && (16..=31).contains(&octets[1]))
                    || (octets[0] == 192 && octets[1] == 168)
            }
            IpAddr::V6(_) => false, // conservative: don't assume LAN for IPv6
        }
    }

    // ── Selection logic ───────────────────────────────────────────────────────

    /// Choose the preferred transport type for a peer in `Auto` mode.
    async fn preferred_type(&self, peer: &PeerInfo) -> TransportType {
        let stats = self.stats.read().await;
        let peer_key = peer.id.to_string();

        // Check if this is an in-process peer (special mesh_ip prefix).
        if peer.mesh_ip.starts_with("inproc://") || peer.mesh_ip == "localhost" {
            return TransportType::InProcess;
        }

        // LAN peers prefer gRPC (TCP, low overhead).
        if Self::is_lan_peer(&peer.mesh_ip) {
            let grpc_stats = stats.get(&(peer_key.clone(), TransportType::Grpc));
            if grpc_stats.map(|s| !s.is_unhealthy()).unwrap_or(true) {
                return TransportType::Grpc;
            }
        }

        // Remote peers: prefer QUIC, fall back to WSS.
        let quic_stats = stats.get(&(peer_key.clone(), TransportType::Quic));
        if quic_stats.map(|s| !s.is_unhealthy()).unwrap_or(true) {
            return TransportType::Quic;
        }

        // WSS as final fallback.
        TransportType::Wss
    }

    // ── Instrumented send ─────────────────────────────────────────────────────

    async fn send_with_stats(
        &self,
        transport_type: TransportType,
        peer: &PeerInfo,
        payload: &[u8],
    ) -> Result<Vec<u8>> {
        let start = Instant::now();
        let result = match transport_type {
            TransportType::Grpc => self.grpc.send(peer, payload).await,
            TransportType::Quic => self.quic.send(peer, payload).await,
            TransportType::Wss => self.wss.send(peer, payload).await,
            TransportType::InProcess => self.in_process.send(peer, payload).await,
        };

        let key = (peer.id.to_string(), transport_type);
        let mut stats = self.stats.write().await;
        let entry = stats.entry(key).or_insert_with(TransportStats::new);
        match &result {
            Ok(_) => entry.record_success(start.elapsed()),
            Err(_) => entry.record_error(),
        }
        result
    }

    /// Get a snapshot of current health stats for diagnostics.
    pub async fn health_snapshot(&self) -> HashMap<(String, TransportType), HealthInfo> {
        self.stats
            .read()
            .await
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    HealthInfo {
                        latency_ms_ema: v.latency_ms_ema,
                        consecutive_errors: v.consecutive_errors,
                        error_rate: v.error_rate(),
                        healthy: !v.is_unhealthy(),
                    },
                )
            })
            .collect()
    }
}

/// Public health information for a transport×peer combination.
#[derive(Debug, Clone)]
pub struct HealthInfo {
    /// EWMA latency in milliseconds.
    pub latency_ms_ema: f64,
    /// Consecutive failed requests.
    pub consecutive_errors: u32,
    /// Fraction of requests that failed [0.0, 1.0].
    pub error_rate: f64,
    /// True when the transport is considered usable.
    pub healthy: bool,
}

// ── Transport impl ────────────────────────────────────────────────────────────

/// A dummy listener that delegates to whatever transport the selector chooses.
///
/// Since the selector is primarily a client-side construct, the listen() call
/// delegates to the transport type matching the current mode.
pub struct SelectorListener {
    /// The concrete listener chosen at listen() time.
    inner: Box<dyn TransportListener>,
}

#[async_trait]
impl TransportListener for SelectorListener {
    async fn accept(&mut self) -> Result<TransportConnection> {
        self.inner.accept().await
    }
}

#[async_trait]
impl Transport for TransportSelector {
    fn name(&self) -> &str {
        "selector"
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        let transport_type = match &self.mode {
            TransportMode::Grpc => TransportType::Grpc,
            TransportMode::Quic => TransportType::Quic,
            TransportMode::Wss => TransportType::Wss,
            TransportMode::InProcess => TransportType::InProcess,
            TransportMode::Auto => self.preferred_type(peer).await,
        };

        log::debug!(
            target: "clawz::transport::selector",
            "routing peer {} via {:?}",
            peer.id,
            transport_type
        );

        let result = self.send_with_stats(transport_type, peer, payload).await;

        // In Auto mode, try next transport on failure.
        // The fallback chain is: gRPC → QUIC → WSS.  In-process has no fallback.
        if result.is_err() && self.mode == TransportMode::Auto {
            let fallback = match transport_type {
                TransportType::Grpc => TransportType::Quic,
                TransportType::Quic => TransportType::Wss,
                TransportType::Wss => {
                    return result; // no more fallbacks
                }
                TransportType::InProcess => {
                    return result; // in-process has no network fallback
                }
            };
            log::warn!(
                target: "clawz::transport::selector",
                "transport {:?} failed for peer {}, trying {:?}",
                transport_type, peer.id, fallback
            );
            return self.send_with_stats(fallback, peer, payload).await;
        }

        result
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>> {
        let inner: Box<dyn TransportListener> = match &self.mode {
            // In Auto mode we default to gRPC for the listener because it is
            // the simplest to expose behind a reverse proxy.
            TransportMode::Grpc | TransportMode::Auto => self.grpc.listen(addr).await?,
            TransportMode::Quic => self.quic.listen(addr).await?,
            TransportMode::Wss => self.wss.listen(addr).await?,
            TransportMode::InProcess => self.in_process.listen(addr).await?,
        };
        Ok(Box::new(SelectorListener { inner }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::config::TransportConfig;

    fn make_selector() -> TransportSelector {
        TransportSelector::from_config(&TransportConfig::default()).unwrap()
    }

    #[test]
    fn is_lan_peer_private() {
        assert!(TransportSelector::is_lan_peer("10.0.0.1"));
        assert!(TransportSelector::is_lan_peer("192.168.1.100"));
        assert!(TransportSelector::is_lan_peer("172.16.0.5"));
    }

    #[test]
    fn is_lan_peer_public() {
        assert!(!TransportSelector::is_lan_peer("8.8.8.8"));
        assert!(!TransportSelector::is_lan_peer("1.1.1.1"));
        assert!(!TransportSelector::is_lan_peer("not-an-ip"));
    }

    #[tokio::test]
    async fn auto_mode_picks_lan_transport_for_private_ip() {
        let selector = make_selector();
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "192.168.1.50", "lan-host");
        let chosen = selector.preferred_type(&peer).await;
        assert_eq!(chosen, TransportType::Grpc);
    }

    #[tokio::test]
    async fn auto_mode_picks_quic_for_public_ip() {
        let selector = make_selector();
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "8.8.8.8", "remote-host");
        let chosen = selector.preferred_type(&peer).await;
        assert_eq!(chosen, TransportType::Quic);
    }

    #[tokio::test]
    async fn auto_mode_picks_inprocess_for_inproc_prefix() {
        let selector = make_selector();
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "inproc://local", "local");
        let chosen = selector.preferred_type(&peer).await;
        assert_eq!(chosen, TransportType::InProcess);
    }

    #[tokio::test]
    async fn stats_track_success() {
        let selector = make_selector();
        let key = ("peer-abc".to_string(), TransportType::Grpc);
        {
            let mut stats = selector.stats.write().await;
            let entry = stats.entry(key.clone()).or_insert_with(TransportStats::new);
            entry.record_success(Duration::from_millis(10));
        }
        let snapshot = selector.health_snapshot().await;
        let info = snapshot.get(&key).unwrap();
        assert!(info.healthy);
        assert_eq!(info.consecutive_errors, 0);
    }

    #[tokio::test]
    async fn unhealthy_after_three_errors() {
        let selector = make_selector();
        let key = ("peer-abc".to_string(), TransportType::Quic);
        {
            let mut stats = selector.stats.write().await;
            let entry = stats.entry(key.clone()).or_insert_with(TransportStats::new);
            entry.record_error();
            entry.record_error();
            entry.record_error();
        }
        let snapshot = selector.health_snapshot().await;
        let info = snapshot.get(&key).unwrap();
        assert!(!info.healthy);
    }
}
