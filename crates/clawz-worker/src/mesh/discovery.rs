//! Peer discovery for the mesh.
//!
//! Combines three discovery methods:
//!
//! 1. **Static** — bootstrap peer list from `MeshConfig.bootstrap_peers`.
//! 2. **mDNS** — UDP multicast on `224.0.0.251:5353` for LAN-local peers.
//! 3. **API** — query the ClawZ gateway for registered peers.
//!
//! All methods are attempted independently; failures are logged and skipped.
//! Results are deduplicated by `peer_id`.
//!
//! # Role in Networking
//!
//! Discovery is the entry point of the mesh lifecycle: without peers,
//! there is nothing to heartbeat or route to.  The background loop
//! (started via [`PeerDiscovery::start_background_loop`]) continually
//! refreshes the peer list so that new nodes join the mesh automatically.
//!
//! # Key Dependencies
//!
//! - [`crate::mesh::config::MeshConfig`] — bootstrap list and gateway URL.
//! - [`crate::mesh::manager::MeshManager`] — consumes discovered peers.
//! - `clawz_core::types::mesh::PeerInfo` — shared peer descriptor.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use uuid::Uuid;

// Dependency: clawz-core::error — unified error type.
use clawz_core::error::{ClawzError, Result};
// Dependency: clawz-core::types::mesh::PeerInfo — shared peer descriptor.
use clawz_core::types::mesh::PeerInfo;

// Dependency: crate::mesh::config — bootstrap list and gateway URL.
use crate::mesh::config::MeshConfig;

// ── DiscoveryMethod ───────────────────────────────────────────────────────────

/// Which discovery method found a peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMethod {
    /// From the static bootstrap list in config.
    Static,
    /// From UDP multicast on the local network segment.
    Mdns,
    /// From the ClawZ gateway REST API.
    Api,
}

impl std::fmt::Display for DiscoveryMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMethod::Static => write!(f, "static"),
            DiscoveryMethod::Mdns => write!(f, "mdns"),
            DiscoveryMethod::Api => write!(f, "api"),
        }
    }
}

// ── DiscoveredPeer ────────────────────────────────────────────────────────────

/// A peer discovered by a discovery method, before deduplication.
#[derive(Debug, Clone)]
struct DiscoveredPeer {
    /// Peer descriptor from clawz-core.
    info: PeerInfo,
    /// Which method produced this entry.
    method: DiscoveryMethod,
}

// ── mDNS wire format helpers ──────────────────────────────────────────────────

/// Minimal mDNS PTR query for `_clawz._udp.local.`
///
/// We build the DNS wire format manually to avoid pulling in a full
/// DNS library for a single query type.
fn build_mdns_query() -> Vec<u8> {
    // DNS message: ID=0, FLAGS=standard query, QDCOUNT=1
    let mut msg = vec![
        0x00, 0x00, // ID
        0x00, 0x00, // FLAGS: standard query
        0x00, 0x01, // QDCOUNT: 1
        0x00, 0x00, // ANCOUNT: 0
        0x00, 0x00, // NSCOUNT: 0
        0x00, 0x00, // ARCOUNT: 0
    ];

    // QNAME: _clawz._udp.local.
    for label in &["_clawz", "_udp", "local"] {
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0x00); // root label

    // QTYPE=PTR (12), QCLASS=IN|UNICAST (0x8001)
    msg.extend_from_slice(&[0x00, 0x0c, 0x80, 0x01]);
    msg
}

/// Try to parse a ClawZ mDNS announcement from a raw UDP payload.
///
/// A real implementation would parse the full mDNS DNS-SD TXT record.
/// Here we parse a simple JSON-encoded announcement that the worker would
/// broadcast:
///
/// ```json
/// {"peer_id":"<uuid>","mesh_ip":"100.64.x.x","hostname":"worker-1"}
/// ```
fn parse_mdns_response(data: &[u8]) -> Option<PeerInfo> {
    let text = std::str::from_utf8(data).ok()?;
    // Look for our simple JSON marker anywhere in the payload.
    let start = text.find('{')?;
    let end = text.rfind('}')? + 1;
    let json = &text[start..end];

    #[derive(serde::Deserialize)]
    struct Announcement {
        peer_id: Uuid,
        mesh_ip: String,
        hostname: String,
    }

    let ann: Announcement = serde_json::from_str(json).ok()?;
    Some(PeerInfo::new(ann.peer_id, ann.mesh_ip, ann.hostname))
}

// ── PeerDiscovery ─────────────────────────────────────────────────────────────

/// Peer discovery aggregator.
///
/// Call [`discover()`] to run all methods and get a deduplicated peer list.
/// The background loop can be started with [`start_background_loop()`].
pub struct PeerDiscovery {
    /// Mesh config (bootstrap list, gateway URL, interval).
    config: MeshConfig,
    /// Known peer cache: peer_id → (peer_info, discovery_method).
    known_peers: Arc<RwLock<HashMap<Uuid, (PeerInfo, DiscoveryMethod)>>>,
    /// HTTP client for gateway API queries.
    http_client: reqwest::Client,
}

impl PeerDiscovery {
    /// Create a new discovery instance from mesh configuration.
    pub fn new(config: MeshConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        Self {
            config,
            known_peers: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    // ── Main entry point ───────────────────────────────────────────────────────

    /// Run all discovery methods and return a deduplicated list of peers.
    ///
    /// Failures in individual methods are logged and skipped.
    pub async fn discover(&self) -> Vec<PeerInfo> {
        let mut found: Vec<DiscoveredPeer> = Vec::new();

        // 1. Static discovery (always succeeds).
        found.extend(self.discover_static().await);

        // 2. mDNS discovery (may fail on non-LAN environments).
        match self.discover_mdns().await {
            Ok(peers) => found.extend(peers),
            Err(e) => log::debug!("mDNS discovery skipped: {e}"),
        }

        // 3. API discovery (may fail if gateway is unreachable).
        match self.discover_api().await {
            Ok(peers) => found.extend(peers),
            Err(e) => log::debug!("API discovery skipped: {e}"),
        }

        // Deduplicate by peer_id, preferring API > mDNS > Static order.
        // API is preferred because it carries the authoritative public_key
        // and is less susceptible to stale LAN announcements.
        let mut by_id: HashMap<Uuid, DiscoveredPeer> = HashMap::new();
        for peer in found {
            let id = peer.info.id;
            let priority = match peer.method {
                DiscoveryMethod::Api => 3,
                DiscoveryMethod::Mdns => 2,
                DiscoveryMethod::Static => 1,
            };
            let existing_priority = by_id.get(&id).map(|p| match p.method {
                DiscoveryMethod::Api => 3,
                DiscoveryMethod::Mdns => 2,
                DiscoveryMethod::Static => 1,
            }).unwrap_or(0);
            if priority > existing_priority {
                by_id.insert(id, peer);
            }
        }

        // Update cache so `cached_peers()` returns the latest list.
        {
            let mut cache = self.known_peers.write().await;
            for (id, dp) in &by_id {
                cache.insert(*id, (dp.info.clone(), dp.method));
            }
        }

        by_id.into_values().map(|dp| dp.info).collect()
    }

    /// Return the currently cached peer list without re-running discovery.
    pub async fn cached_peers(&self) -> Vec<PeerInfo> {
        self.known_peers
            .read()
            .await
            .values()
            .map(|(p, _)| p.clone())
            .collect()
    }

    // ── Static discovery ───────────────────────────────────────────────────────

    /// Parse bootstrap_peers from config into PeerInfo entries.
    ///
    /// Addresses without an explicit peer_id get a deterministic UUID
    /// derived from the address string so the same bootstrap list always
    /// produces the same IDs (avoiding duplicate entries).
    async fn discover_static(&self) -> Vec<DiscoveredPeer> {
        self.config
            .bootstrap_peers
            .iter()
            .map(|addr| {
                // Derive a deterministic UUID from the address string.
                let id = uuid_from_str(addr);
                let peer = PeerInfo::new(id, addr.clone(), addr.clone());
                DiscoveredPeer {
                    info: peer,
                    method: DiscoveryMethod::Static,
                }
            })
            .collect()
    }

    // ── mDNS discovery ─────────────────────────────────────────────────────────

    /// Send an mDNS PTR query and collect responses for 500ms.
    async fn discover_mdns(&self) -> Result<Vec<DiscoveredPeer>> {
        const MDNS_ADDR: &str = "224.0.0.251:5353";
        const BIND_ADDR: &str = "0.0.0.0:0";
        const LISTEN_MS: u64 = 500;

        let socket = UdpSocket::bind(BIND_ADDR).await.map_err(|e| {
            ClawzError::Mesh(format!("mDNS bind failed: {e}"))
        })?;

        socket.set_multicast_ttl_v4(255).ok();

        let multicast_addr: SocketAddr = MDNS_ADDR.parse().map_err(|e| {
            ClawzError::Mesh(format!("mDNS addr parse failed: {e}"))
        })?;

        let query = build_mdns_query();
        socket
            .send_to(&query, multicast_addr)
            .await
            .map_err(|e| ClawzError::Mesh(format!("mDNS send failed: {e}")))?;

        let mut peers = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(LISTEN_MS);
        let mut buf = vec![0u8; 4096];

        while let Ok(Ok((len, _src))) =
            tokio::time::timeout_at(deadline, socket.recv_from(&mut buf)).await
        {
            if let Some(peer) = parse_mdns_response(&buf[..len]) {
                log::debug!("mDNS discovered peer: {} ({})", peer.id, peer.hostname);
                peers.push(DiscoveredPeer {
                    info: peer,
                    method: DiscoveryMethod::Mdns,
                });
            }
        }

        log::debug!("mDNS discovery found {} peer(s)", peers.len());
        Ok(peers)
    }

    // ── API discovery ──────────────────────────────────────────────────────────

    /// Query the ClawZ gateway API for registered peers.
    async fn discover_api(&self) -> Result<Vec<DiscoveredPeer>> {
        let api_url = match &self.config.gateway_api_url {
            Some(url) => url.clone(),
            None => return Ok(vec![]),
        };

        let url = format!("{api_url}/api/v1/mesh/peers");
        log::debug!("Querying gateway for peers: {url}");

        let response = self
            .http_client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| ClawzError::Mesh(format!("API discovery request failed: {e}")))?;

        if !response.status().is_success() {
            return Err(ClawzError::Mesh(format!(
                "API discovery returned HTTP {}",
                response.status()
            )));
        }

        #[derive(serde::Deserialize)]
        struct ApiPeer {
            id: Uuid,
            mesh_ip: String,
            hostname: String,
            #[serde(default)]
            public_key: Option<String>,
        }

        #[derive(serde::Deserialize)]
        struct ApiResponse {
            peers: Vec<ApiPeer>,
        }

        let body: ApiResponse = response
            .json()
            .await
            .map_err(|e| ClawzError::Mesh(format!("API discovery JSON parse failed: {e}")))?;

        let peers = body
            .peers
            .into_iter()
            .map(|ap| {
                let mut info = PeerInfo::new(ap.id, ap.mesh_ip, ap.hostname);
                info.public_key = ap.public_key;
                DiscoveredPeer {
                    info,
                    method: DiscoveryMethod::Api,
                }
            })
            .collect();

        log::debug!("API discovery found {} peer(s)", {
            let p: &Vec<DiscoveredPeer> = &peers;
            p.len()
        });
        Ok(peers)
    }

    // ── Background loop ────────────────────────────────────────────────────────

    /// Spawn a background task that re-runs discovery every `discovery_interval_secs`.
    ///
    /// Returns the join handle so the caller can abort it on shutdown.
    pub fn start_background_loop(
        self: Arc<Self>,
        new_peer_tx: tokio::sync::mpsc::Sender<PeerInfo>,
    ) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_secs(self.config.discovery_interval_secs);

        tokio::spawn(async move {
            log::debug!("Discovery background loop started (interval={interval:?})");
            loop {
                tokio::time::sleep(interval).await;

                let peers = self.discover().await;
                log::debug!("Discovery loop found {} peer(s)", peers.len());
                for peer in peers {
                    if new_peer_tx.send(peer).await.is_err() {
                        log::debug!("Discovery loop: receiver dropped, exiting");
                        return;
                    }
                }
            }
        })
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Derive a deterministic UUID from a string by hashing its bytes.
///
/// Uses a simple FNV-1a hash to produce 16 bytes, then formats as a UUID.
/// This avoids needing the uuid `v5` feature flag.
fn uuid_from_str(s: &str) -> Uuid {
    // FNV-1a 128-bit hash.
    const FNV_OFFSET: u128 = 0x6c62272e07bb0142_62b821756295c58d;
    const FNV_PRIME: u128 = 0x0000000001000000_000000000000013B;
    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= *byte as u128;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Set version = 4 and variant bits to make a valid UUID.
    let bytes = hash.to_be_bytes();
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&bytes);
    // Version 4: set bits 12-15 of octet 6 to 0100.
    arr[6] = (arr[6] & 0x0F) | 0x40;
    // Variant: set bits 6-7 of octet 8 to 10.
    arr[8] = (arr[8] & 0x3F) | 0x80;
    Uuid::from_bytes(arr)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_with_bootstrap(peers: Vec<&str>) -> MeshConfig {
        let mut c = MeshConfig::default();
        c.bootstrap_peers = peers.into_iter().map(|s| s.to_string()).collect();
        c
    }

    #[tokio::test]
    async fn static_discovery() {
        let cfg = cfg_with_bootstrap(vec!["10.0.0.1:51820", "10.0.0.2:51820"]);
        let disc = PeerDiscovery::new(cfg);
        let peers = disc.discover().await;
        assert_eq!(peers.len(), 2);
    }

    #[tokio::test]
    async fn deduplication_same_bootstrap() {
        // Duplicate entries in bootstrap list should be deduplicated.
        let cfg = cfg_with_bootstrap(vec!["10.0.0.1:51820", "10.0.0.1:51820"]);
        let disc = PeerDiscovery::new(cfg);
        let peers = disc.discover().await;
        assert_eq!(peers.len(), 1);
    }

    #[tokio::test]
    async fn mdns_discovery_no_panic() {
        // mDNS will likely fail in the test environment; should not panic.
        let cfg = MeshConfig::default();
        let disc = PeerDiscovery::new(cfg);
        // We just want to ensure it doesn't panic.
        let _ = disc.discover_mdns().await;
    }

    #[tokio::test]
    async fn api_discovery_skipped_without_url() {
        let cfg = MeshConfig::default(); // no gateway_api_url
        let disc = PeerDiscovery::new(cfg);
        let result = disc.discover_api().await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn cached_peers_populated_after_discover() {
        let cfg = cfg_with_bootstrap(vec!["172.16.0.1:51820"]);
        let disc = PeerDiscovery::new(cfg);
        disc.discover().await;
        let cached = disc.cached_peers().await;
        assert_eq!(cached.len(), 1);
    }

    #[test]
    fn uuid_from_str_is_deterministic() {
        let a = uuid_from_str("10.0.0.1:51820");
        let b = uuid_from_str("10.0.0.1:51820");
        assert_eq!(a, b);
    }

    #[test]
    fn uuid_from_str_different_inputs() {
        let a = uuid_from_str("10.0.0.1:51820");
        let b = uuid_from_str("10.0.0.2:51820");
        assert_ne!(a, b);
    }

    #[test]
    fn parse_mdns_response_valid() {
        let json = r#"{"peer_id":"550e8400-e29b-41d4-a716-446655440000","mesh_ip":"100.64.1.2","hostname":"worker-2"}"#;
        let peer = parse_mdns_response(json.as_bytes()).unwrap();
        assert_eq!(peer.mesh_ip, "100.64.1.2");
        assert_eq!(peer.hostname, "worker-2");
    }

    #[test]
    fn parse_mdns_response_invalid() {
        let bad = b"\x00\x01\x02 not json at all";
        assert!(parse_mdns_response(bad).is_none());
    }
}
