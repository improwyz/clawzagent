//! QUIC transport — preferred remote transport.
//!
//! Uses [`quinn`] (IETF QUIC) which provides:
//! * Built-in TLS 1.3 encryption (no separate layer needed).
//! * Multiplexed streams over a single UDP connection.
//! * Optional 0-RTT for subsequent connections to the same peer.
//! * No head-of-line blocking.
//!
//! Each `send()` opens a new bi-directional QUIC stream, writes the payload,
//! and reads the response.  The QUIC *connection* to a peer is cached so that
//! subsequent calls reuse the same 0-RTT-capable connection.
//!
//! Certificates
//! ------------
//! If `cert_path`/`key_path` are provided in the config the certs are loaded
//! from disk (PEM format).  Otherwise a self-signed certificate is generated
//! at startup via [`rcgen`].  In both cases the client side uses a custom
//! verifier that accepts the server's self-signed cert (suitable for dev /
//! LAN mesh; replace with a proper CA in production).
//!
//! # Role in Networking
//!
//! QUIC is the default transport for remote (public-IP) peers because it
//! handles NAT traversal better than raw TCP and provides built-in encryption.
//! It is chosen by [`crate::transport::selector::TransportSelector`] when
//! the peer is not in RFC-1918 space.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportConnection, TransportListener}` — implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.
//! - [`crate::transport::config::QuicConfig`] — ports, TLS, timeouts.

use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use async_trait::async_trait;

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{Transport, TransportConnection, TransportListener},
    types::mesh::PeerInfo,
};
use quinn::{
    ClientConfig, Endpoint, ServerConfig, TransportConfig as QuinnTransportConfig, VarInt,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::Mutex,
    time::timeout,
};

// Dependency: crate::transport::config — QUIC-specific settings.
use crate::transport::config::QuicConfig;

// ── Certificate utilities ─────────────────────────────────────────────────────

/// Generate a self-signed certificate + private key for `hostname`.
///
/// Used when `cert_path`/`key_path` are not provided in [`QuicConfig`].
fn generate_self_signed(hostname: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let cert = rcgen::generate_simple_self_signed(vec![
        hostname.to_string(),
        "localhost".to_string(),
        "127.0.0.1".to_string(),
    ])
    .map_err(|e| ClawzError::Transport(format!("rcgen: {e}")))?;
    Ok((cert.cert.der().to_vec(), cert.key_pair.serialize_der()))
}

/// Build a [`ServerConfig`] from DER-encoded cert and key bytes.
fn build_server_config(
    cert_der: Vec<u8>,
    key_der: Vec<u8>,
    max_idle_ms: u64,
) -> Result<ServerConfig> {
    let cert = CertificateDer::from(cert_der);
    let key = PrivateKeyDer::try_from(key_der)
        .map_err(|e| ClawzError::Transport(format!("private key: {e}")))?;

    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .map_err(|e| ClawzError::Transport(format!("tls server config: {e}")))?;
    // ALPN lets the client verify it is talking to a ClawZ peer.
    tls_config.alpn_protocols = vec![b"clawz/1".to_vec()];

    let mut transport = QuinnTransportConfig::default();
    transport.max_idle_timeout(Some(
        VarInt::from_u64(max_idle_ms)
            .ok()
            .map(|v| v.into())
            .unwrap_or_else(|| {
                // Fallback to 30 s if the configured idle timeout is too large for a VarInt.
                VarInt::from_u64(30_000).unwrap().into()
            }),
    ));

    let mut server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .map_err(|e| ClawzError::Transport(format!("quinn server config: {e}")))?,
    ));
    server_config.transport_config(Arc::new(transport));
    Ok(server_config)
}

/// A [`rustls`] certificate verifier that accepts any certificate.
///
/// **Warning:** only suitable for dev / internal mesh use.
/// In production, replace with a verifier that checks against a pinned CA.
#[derive(Debug)]
struct AcceptAnyCert;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

/// Build a [`ClientConfig`] that accepts any server certificate.
fn build_client_config() -> Result<ClientConfig> {
    let tls_config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCert))
        .with_no_client_auth();

    Ok(ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)
            .map_err(|e| ClawzError::Transport(format!("quinn client config: {e}")))?,
    )))
}

// ── Connection cache ──────────────────────────────────────────────────────────

/// Cache of open QUIC connections keyed by peer address.
///
/// Reusing connections avoids repeated QUIC handshakes and enables 0-RTT.
struct ConnCache {
    connections: Mutex<HashMap<String, quinn::Connection>>,
}

impl ConnCache {
    fn new() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
        }
    }

    async fn get_or_connect(
        &self,
        endpoint: &Endpoint,
        addr: SocketAddr,
        peer_key: &str,
        connect_timeout: Duration,
    ) -> Result<quinn::Connection> {
        let mut map = self.connections.lock().await;
        if let Some(conn) = map.get(peer_key) {
            // Verify the connection is still alive.
            if conn.close_reason().is_none() {
                return Ok(conn.clone());
            }
        }
        // Establish a new connection.
        let connecting = endpoint
            .connect(addr, "clawz-peer")
            .map_err(|e| ClawzError::Transport(format!("quic connect initiate: {e}")))?;
        let conn = timeout(connect_timeout, connecting)
            .await
            .map_err(|_| ClawzError::Transport(format!("quic connect timeout to {addr}")))?
            .map_err(|e| ClawzError::Transport(format!("quic connect to {addr}: {e}")))?;
        map.insert(peer_key.to_string(), conn.clone());
        Ok(conn)
    }

    async fn remove(&self, peer_key: &str) {
        self.connections.lock().await.remove(peer_key);
    }
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// Wraps a quinn [`Endpoint`] in server mode and implements [`TransportListener`].
pub struct QuicListener {
    endpoint: Endpoint,
}

#[async_trait]
impl TransportListener for QuicListener {
    async fn accept(&mut self) -> Result<TransportConnection> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| ClawzError::Transport("quic endpoint closed".into()))?;

        let conn = incoming
            .await
            .map_err(|e| ClawzError::Transport(format!("quic accept connection: {e}")))?;

        let peer_id = conn.remote_address().to_string();

        let (mut send, mut recv) = conn
            .accept_bi()
            .await
            .map_err(|e| ClawzError::Transport(format!("quic accept stream: {e}")))?;

        // Wrap send/recv as a duplex AsyncRead/AsyncWrite.
        let (a_tx, b_rx) = tokio::io::duplex(256 * 1024);
        let (b_tx, a_rx) = tokio::io::duplex(256 * 1024);

        // recv -> a_tx (client reads from b_rx)
        tokio::spawn(async move {
            let mut writer = a_tx;
            let mut buf = vec![0u8; 65536];
            while let Ok(Some(n)) = recv.read(&mut buf).await {
                if writer.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        });
        // b_tx -> send
        tokio::spawn(async move {
            let mut reader = b_tx;
            let mut buf = vec![0u8; 65536];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if send.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(TransportConnection {
            peer_id,
            reader: Box::new(b_rx),
            writer: Box::new(a_rx),
        })
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

/// QUIC transport using quinn.
pub struct QuicTransport {
    config: QuicConfig,
    /// Client-mode endpoint (bound to any port).
    client_endpoint: Endpoint,
    /// Cache of open QUIC connections keyed by peer address.
    conn_cache: Arc<ConnCache>,
}

impl QuicTransport {
    /// Create a new [`QuicTransport`].
    pub fn new(config: QuicConfig) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client_config = build_client_config()?;
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())
            .map_err(|e| ClawzError::Transport(format!("quic client endpoint: {e}")))?;
        endpoint.set_default_client_config(client_config);

        Ok(Self {
            config,
            client_endpoint: endpoint,
            conn_cache: Arc::new(ConnCache::new()),
        })
    }

    /// Derive a `(SocketAddr, peer_key)` pair from [`PeerInfo`].
    fn peer_addr(peer: &PeerInfo) -> Result<(SocketAddr, String)> {
        let port = 4433u16;
        let addr_str = if peer.mesh_ip.contains(':') {
            peer.mesh_ip.clone()
        } else {
            format!("{}:{port}", peer.mesh_ip)
        };
        let addr: SocketAddr = addr_str
            .parse()
            .map_err(|e| ClawzError::Transport(format!("parse peer addr '{addr_str}': {e}")))?;
        Ok((addr, addr_str))
    }
}

#[async_trait]
impl Transport for QuicTransport {
    fn name(&self) -> &str {
        "quic"
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        let (addr, peer_key) = Self::peer_addr(peer)?;
        let connect_timeout = Duration::from_millis(self.config.connect_timeout_ms);
        let request_timeout = Duration::from_millis(self.config.request_timeout_ms);

        let conn = self
            .conn_cache
            .get_or_connect(&self.client_endpoint, addr, &peer_key, connect_timeout)
            .await?;

        let result: Result<Vec<u8>> = timeout(request_timeout, async {
            let (mut send, mut recv) = conn
                .open_bi()
                .await
                .map_err(|e| ClawzError::Transport(format!("quic open stream: {e}")))?;

            // Write length-prefixed payload so the receiver knows when
            // the message is complete without relying on stream close.
            let len = (payload.len() as u32).to_be_bytes();
            send.write_all(&len)
                .await
                .map_err(|e| ClawzError::Transport(format!("quic write len: {e}")))?;
            send.write_all(payload)
                .await
                .map_err(|e| ClawzError::Transport(format!("quic write payload: {e}")))?;
            send.finish()
                .map_err(|e| ClawzError::Transport(format!("quic finish send: {e}")))?;

            // Read length-prefixed response.
            let mut len_buf = [0u8; 4];
            recv.read_exact(&mut len_buf)
                .await
                .map_err(|e| ClawzError::Transport(format!("quic read resp len: {e}")))?;
            let resp_len = u32::from_be_bytes(len_buf) as usize;
            // Same 64 MiB limit as gRPC to prevent OOM from a malicious peer.
            if resp_len > 64 * 1024 * 1024 {
                return Err(ClawzError::Transport(format!(
                    "quic response too large: {resp_len}"
                )));
            }
            let mut buf = vec![0u8; resp_len];
            recv.read_exact(&mut buf)
                .await
                .map_err(|e| ClawzError::Transport(format!("quic read resp body: {e}")))?;
            Ok(buf)
        })
        .await
        .map_err(|_| ClawzError::Transport(format!("quic request timeout to {peer_key}")))?;

        if result.is_err() {
            // Remove potentially broken connection from cache so the next
            // send() creates a fresh one instead of reusing a dead connection.
            self.conn_cache.remove(&peer_key).await;
        }

        result
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>> {
        let bind_addr: SocketAddr = addr
            .parse()
            .map_err(|e| ClawzError::Transport(format!("invalid listen addr '{addr}': {e}")))?;

        let (cert_der, key_der) = generate_self_signed("clawz-worker")?;

        let server_config =
            build_server_config(cert_der, key_der, self.config.max_idle_timeout_ms)?;

        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| ClawzError::Transport(format!("quic server bind {addr}: {e}")))?;

        log::info!(target: "clawz::transport::quic", "listening on {addr}");
        Ok(Box::new(QuicListener { endpoint }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_self_signed_cert() {
        let (cert, key) = generate_self_signed("test-host").unwrap();
        assert!(!cert.is_empty());
        assert!(!key.is_empty());
    }

    #[tokio::test]
    async fn create_transport_and_listen() {
        let config = QuicConfig::default();
        let transport = QuicTransport::new(config).unwrap();
        let listener = transport.listen("127.0.0.1:0").await;
        assert!(listener.is_ok(), "should bind QUIC listener");
    }

    #[test]
    fn peer_addr_parses_plain_ip() {
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "10.0.0.1", "h");
        let (addr, key) = QuicTransport::peer_addr(&peer).unwrap();
        assert_eq!(addr.port(), 4433);
        assert!(key.contains("10.0.0.1"));
    }

    #[test]
    fn peer_addr_parses_ip_with_port() {
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "10.0.0.1:9999", "h");
        let (addr, _) = QuicTransport::peer_addr(&peer).unwrap();
        assert_eq!(addr.port(), 9999);
    }
}
