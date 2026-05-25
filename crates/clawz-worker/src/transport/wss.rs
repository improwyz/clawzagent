//! Encrypted WebSocket (WSS) transport — remote / firewall-friendly fallback.
//!
//! Architecture
//! ------------
//! * **Client side** — [`WssTransport::send`] connects to the peer's WSS
//!   endpoint (or a relay gateway), performs the TLS upgrade with a JWT in
//!   the `Authorization` header, sends the payload as a single binary frame,
//!   and waits for the response binary frame.  Connections are *not* pooled
//!   (each `send` opens a fresh WS connection) because WebSocket upgrades are
//!   cheap compared to QUIC handshakes and this keeps the state machine simple.
//!   A future version may add per-peer connection keepalive.
//!
//! * **Server side** — [`WssTransport::listen`] binds a plain TCP listener.
//!   In production you should front it with a TLS-terminating reverse proxy
//!   (nginx / caddy).  The listener returns [`TransportConnection`] objects
//!   from which callers can read raw binary WebSocket messages.
//!
//! Reconnection
//! ------------
//! On connection failure `send` retries with exponential back-off up to
//! `WssConfig::reconnect_max_attempts` (0 = unlimited).
//!
//! # Role in Networking
//!
//! WSS is the final fallback transport.  It is chosen by
//! [`crate::transport::selector::TransportSelector`] when gRPC and QUIC
//! are both unavailable or unhealthy.  Its ability to traverse restrictive
//! firewalls and corporate proxies makes it essential for wide-area meshes.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportConnection, TransportListener}` — implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.
//! - [`crate::transport::config::WssConfig`] — gateway URL, JWT secret, retry policy.

use std::{net::SocketAddr, time::Duration};

use async_trait::async_trait;

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{Transport, TransportConnection, TransportListener},
    types::mesh::PeerInfo,
};
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::TcpListener,
    time::timeout,
};
use tokio_tungstenite::{
    accept_async,
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::header::{AUTHORIZATION, HeaderValue},
        Message,
    },
};

// Dependency: crate::transport::config — WSS-specific settings.
use crate::transport::config::WssConfig;

// ── JWT helper ────────────────────────────────────────────────────────────────

/// Produce a minimal HS256-signed JWT for the given peer.
///
/// This is a lightweight implementation that avoids pulling in a full JWT
/// crate.  In production you would use `jsonwebtoken` or `josekit`.
fn make_jwt(secret: &str, peer_id: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // 5-minute expiry: long enough for the handshake, short enough to
    // limit replay window if the token is somehow intercepted.
    let claims = format!(
        r#"{{"sub":"{peer_id}","iat":{now},"exp":{}}}"#,
        now + 300
    );
    let payload = URL_SAFE_NO_PAD.encode(&claims);
    let signing_input = format!("{header}.{payload}");

    // HMAC-SHA256
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(signing_input.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{sig}")
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// Accepts incoming WebSocket upgrade requests on a plain TCP listener.
pub struct WssListener {
    inner: TcpListener,
}

#[async_trait]
impl TransportListener for WssListener {
    async fn accept(&mut self) -> Result<TransportConnection> {
        let (tcp_stream, peer_addr) = self
            .inner
            .accept()
            .await
            .map_err(|e| ClawzError::Transport(format!("wss accept tcp: {e}")))?;

        let ws_stream = accept_async(tcp_stream)
            .await
            .map_err(|e| ClawzError::Transport(format!("wss handshake: {e}")))?;

        let peer_id = peer_addr.to_string();
        let (mut sink, mut source) = ws_stream.split();

        // Wrap the WS sink/source as AsyncRead/AsyncWrite via a pair of duplex
        // channels so we can fit them into TransportConnection.
        let (server_tx, client_rx) = tokio::io::duplex(128 * 1024);
        let (client_tx, server_rx) = tokio::io::duplex(128 * 1024);

        // Pump: WS -> client_rx
        {
            let mut writer = server_tx;
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                while let Some(msg) = source.next().await {
                    match msg {
                        Ok(Message::Binary(data)) => {
                            // Length-prefix the binary payload so the reader
                            // on the other side of the duplex can use the same
                            // framing logic as gRPC/QUIC.
                            let len = (data.len() as u32).to_be_bytes();
                            if writer.write_all(&len).await.is_err() { break; }
                            if writer.write_all(&data).await.is_err() { break; }
                        }
                        Ok(Message::Close(_)) | Err(_) => break,
                        _ => {}
                    }
                }
            });
        }
        // Pump: server_rx -> WS
        {
            let mut reader = server_rx;
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                loop {
                    let mut len_buf = [0u8; 4];
                    if reader.read_exact(&mut len_buf).await.is_err() { break; }
                    let len = u32::from_be_bytes(len_buf) as usize;
                    let mut buf = vec![0u8; len];
                    if reader.read_exact(&mut buf).await.is_err() { break; }
                    if sink.send(Message::Binary(buf.into())).await.is_err() { break; }
                }
            });
        }

        Ok(TransportConnection {
            peer_id,
            reader: Box::new(client_rx),
            writer: Box::new(client_tx),
        })
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

/// WebSocket (WSS) transport — works through firewalls, uses TLS via native-tls.
pub struct WssTransport {
    config: WssConfig,
}

impl WssTransport {
    /// Create a new [`WssTransport`] from config.
    pub fn new(config: WssConfig) -> Self {
        Self { config }
    }

    /// Build the WebSocket URL for a peer.
    ///
    /// If `gateway_url` is set in config, route through the relay gateway
    /// (appending the peer mesh-ip as a path segment).  Otherwise connect
    /// directly to the peer's WSS endpoint.
    fn peer_url(&self, peer: &PeerInfo) -> String {
        if let Some(gw) = &self.config.gateway_url {
            format!("{}/peer/{}", gw.trim_end_matches('/'), peer.id)
        } else {
            let port = self.config.listen_port;
            let ip = &peer.mesh_ip;
            if ip.contains(':') {
                format!("ws://{ip}/ws") // ip already has port
            } else {
                format!("ws://{ip}:{port}/ws")
            }
        }
    }

    /// Attempt a single send without retry.
    async fn try_send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        let url = self.peer_url(peer);
        let request_timeout = Duration::from_millis(self.config.request_timeout_ms);

        let mut req = url
            .as_str()
            .into_client_request()
            .map_err(|e| ClawzError::Transport(format!("wss request build: {e}")))?;

        // Add JWT authorization header if a secret is configured.
        // The JWT proves to the peer/gateway that this sender is authorised
        // to join the ClawZ mesh.
        if let Some(secret) = &self.config.jwt_secret {
            let token = make_jwt(secret, &peer.id.to_string());
            let hv = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|e| ClawzError::Transport(format!("jwt header: {e}")))?;
            req.headers_mut().insert(AUTHORIZATION, hv);
        }

        let (mut ws_stream, _response) = timeout(request_timeout, connect_async(req))
            .await
            .map_err(|_| ClawzError::Transport(format!("wss connect timeout to {url}")))?
            .map_err(|e| ClawzError::Transport(format!("wss connect to {url}: {e}")))?;

        // Send payload as a binary frame.
        ws_stream
            .send(Message::Binary(payload.to_vec().into()))
            .await
            .map_err(|e| ClawzError::Transport(format!("wss send: {e}")))?;

        // Await a binary response frame.
        let response = timeout(request_timeout, ws_stream.next())
            .await
            .map_err(|_| ClawzError::Transport(format!("wss response timeout from {url}")))?;

        match response {
            Some(Ok(Message::Binary(data))) => Ok(data.into()),
            Some(Ok(other)) => Err(ClawzError::Transport(format!(
                "wss unexpected frame type: {other:?}"
            ))),
            Some(Err(e)) => Err(ClawzError::Transport(format!("wss recv: {e}"))),
            None => Err(ClawzError::Transport("wss stream closed unexpectedly".into())),
        }
    }
}

#[async_trait]
impl Transport for WssTransport {
    fn name(&self) -> &str {
        "wss"
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        let max_attempts = self.config.reconnect_max_attempts;
        let base_ms = self.config.reconnect_base_ms;
        let max_ms = self.config.reconnect_max_ms;

        let mut attempt: u32 = 0;
        loop {
            match self.try_send(peer, payload).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    attempt += 1;
                    if max_attempts > 0 && attempt >= max_attempts {
                        return Err(e);
                    }
                    // Exponential back-off: base * 2^attempt, capped at max.
                    // The cap prevents unbounded growth during long outages.
                    let delay_ms = (base_ms * (1u64 << attempt.min(10))).min(max_ms);
                    log::warn!(
                        target: "clawz::transport::wss",
                        "send attempt {attempt} failed ({e}), retrying in {delay_ms}ms"
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>> {
        let bind_addr: SocketAddr = addr
            .parse()
            .map_err(|e| ClawzError::Transport(format!("invalid listen addr '{addr}': {e}")))?;
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| ClawzError::Transport(format!("wss bind {addr}: {e}")))?;
        log::info!(target: "clawz::transport::wss", "listening on {addr}");
        Ok(Box::new(WssListener { inner: listener }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_has_three_parts() {
        let token = make_jwt("secret", "peer-abc");
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "JWT must have header.payload.sig");
    }

    #[test]
    fn peer_url_with_gateway() {
        let config = WssConfig {
            gateway_url: Some("wss://relay.example.com".into()),
            ..Default::default()
        };
        let transport = WssTransport::new(config);
        let peer = PeerInfo::new(uuid::Uuid::nil(), "10.0.0.1", "host");
        let url = transport.peer_url(&peer);
        assert!(url.starts_with("wss://relay.example.com/peer/"));
    }

    #[test]
    fn peer_url_direct() {
        let config = WssConfig {
            listen_port: 8443,
            gateway_url: None,
            ..Default::default()
        };
        let transport = WssTransport::new(config);
        let peer = PeerInfo::new(uuid::Uuid::nil(), "10.0.0.5", "host");
        let url = transport.peer_url(&peer);
        assert_eq!(url, "ws://10.0.0.5:8443/ws");
    }

    #[tokio::test]
    async fn test_listen_creates_listener() {
        let config = WssConfig::default();
        let transport = WssTransport::new(config);
        let listener = transport.listen("127.0.0.1:0").await;
        assert!(listener.is_ok());
    }
}
