//! TCP binary-framing transport ("gRPC-style" for LAN peers).
//!
//! Protocol
//! --------
//! Every message is framed as:
//!
//! ```text
//! ┌──────────────────────┬──────────────────────────────────────┐
//! │ length : u32 (BE)    │ payload : [u8; length]               │
//! └──────────────────────┴──────────────────────────────────────┘
//! ```
//!
//! The server reads one framed request, writes one framed response, then
//! keeps the connection alive for the next request (pipelining on the
//! *connection* level; requests on a single connection are serialised).
//!
//! Connection pooling
//! ------------------
//! [`GrpcTransport`] maintains one [`ConnectionPool`] that keeps up to
//! `config.pool_size` TCP connections open per peer address.  Idle
//! connections are reaped after `config.idle_timeout_secs`.
//!
//! # Role in Networking
//!
//! gRPC is the default transport for LAN peers because TCP has lower
//! overhead than QUIC handshakes and avoids UDP NAT traversal issues.
//! It is chosen by [`crate::transport::selector::TransportSelector`] when
//! the peer IP is in RFC-1918 space.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportConnection, TransportListener}` — implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.
//! - [`crate::transport::config::GrpcConfig`] — listen port, pool size, timeouts.

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use bytes::{Buf, BufMut, BytesMut};

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{Transport, TransportConnection, TransportListener},
    types::mesh::PeerInfo,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
    time::timeout,
};

// Dependency: crate::transport::config — gRPC-specific settings.
use crate::transport::config::GrpcConfig;

// ── Frame helpers ─────────────────────────────────────────────────────────────

/// Write a length-prefixed frame to `writer`.
///
/// The 4-byte big-endian header makes it trivial for the receiver to
/// know exactly how many bytes to read, avoiding length-delimiter ambiguity.
async fn write_frame(writer: &mut (impl AsyncWriteExt + Unpin), data: &[u8]) -> Result<()> {
    let len = data.len() as u32;
    let mut header = BytesMut::with_capacity(4);
    header.put_u32(len);
    writer
        .write_all(&header)
        .await
        .map_err(|e| ClawzError::Transport(format!("frame write header: {e}")))?;
    writer
        .write_all(data)
        .await
        .map_err(|e| ClawzError::Transport(format!("frame write body: {e}")))?;
    writer
        .flush()
        .await
        .map_err(|e| ClawzError::Transport(format!("frame flush: {e}")))?;
    Ok(())
}

/// Read a length-prefixed frame from `reader`.
async fn read_frame(reader: &mut (impl AsyncReadExt + Unpin)) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .await
        .map_err(|e| ClawzError::Transport(format!("frame read header: {e}")))?;
    let len = (&header[..]).get_u32() as usize;
    // 64 MiB is a generous upper bound that prevents OOM from a malformed
    // frame while still allowing large bulk transfers.
    if len > 64 * 1024 * 1024 {
        return Err(ClawzError::Transport(format!(
            "frame too large: {len} bytes"
        )));
    }
    let mut buf = vec![0u8; len];
    reader
        .read_exact(&mut buf)
        .await
        .map_err(|e| ClawzError::Transport(format!("frame read body: {e}")))?;
    Ok(buf)
}

// ── Connection pool ───────────────────────────────────────────────────────────

/// A pooled TCP stream with its last-use timestamp.
struct PooledConn {
    stream: TcpStream,
    last_used: Instant,
}

/// Per-address LIFO connection pool.
///
/// Newly-returned connections are pushed to the back so that the most
/// recently used connection is reused first (likely still warm in TCP buffers).
struct AddressPool {
    idle: Vec<PooledConn>,
}

impl AddressPool {
    fn new() -> Self {
        Self { idle: Vec::new() }
    }

    /// Pop an idle connection if one exists and is not stale.
    fn pop(&mut self, idle_timeout: Duration) -> Option<TcpStream> {
        while let Some(conn) = self.idle.pop() {
            if conn.last_used.elapsed() < idle_timeout {
                return Some(conn.stream);
            }
            // stale — discard and try next
        }
        None
    }

    /// Return a connection to the pool.
    fn push(&mut self, stream: TcpStream, max_size: usize) {
        if self.idle.len() < max_size {
            self.idle.push(PooledConn {
                stream,
                last_used: Instant::now(),
            });
        }
        // otherwise discard — pool is full
    }

    /// Evict connections that have been idle longer than `idle_timeout`.
    fn evict_stale(&mut self, idle_timeout: Duration) {
        self.idle.retain(|c| c.last_used.elapsed() < idle_timeout);
    }
}

/// Thread-safe connection pool shared across all send() calls.
struct ConnectionPool {
    pools: Mutex<HashMap<String, AddressPool>>,
    max_per_addr: usize,
    idle_timeout: Duration,
}

impl ConnectionPool {
    fn new(max_per_addr: usize, idle_timeout_secs: u64) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            max_per_addr,
            idle_timeout: Duration::from_secs(idle_timeout_secs),
        }
    }

    /// Obtain a TCP stream for `addr`, reusing an idle one if available.
    async fn acquire(&self, addr: &str, connect_timeout: Duration) -> Result<TcpStream> {
        {
            let mut pools = self.pools.lock().await;
            let ap = pools
                .entry(addr.to_string())
                .or_insert_with(AddressPool::new);
            if let Some(stream) = ap.pop(self.idle_timeout) {
                return Ok(stream);
            }
        }

        let stream = timeout(connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| ClawzError::Transport(format!("connect timeout to {addr}")))?
            .map_err(|e| ClawzError::Transport(format!("connect to {addr}: {e}")))?;

        stream
            .set_nodelay(true)
            .map_err(|e| ClawzError::Transport(format!("set_nodelay: {e}")))?;
        Ok(stream)
    }

    /// Return a stream to the idle pool.
    async fn release(&self, addr: &str, stream: TcpStream) {
        let mut pools = self.pools.lock().await;
        let ap = pools
            .entry(addr.to_string())
            .or_insert_with(AddressPool::new);
        ap.push(stream, self.max_per_addr);
    }

    /// Evict all stale connections across all pools.
    async fn evict_stale(&self) {
        let mut pools = self.pools.lock().await;
        for ap in pools.values_mut() {
            ap.evict_stale(self.idle_timeout);
        }
    }
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// Wraps a [`TcpListener`] and implements [`TransportListener`].
pub struct GrpcListener {
    inner: TcpListener,
}

#[async_trait]
impl TransportListener for GrpcListener {
    async fn accept(&mut self) -> Result<TransportConnection> {
        let (stream, peer_addr) = self
            .inner
            .accept()
            .await
            .map_err(|e| ClawzError::Transport(format!("accept: {e}")))?;
        stream
            .set_nodelay(true)
            .map_err(|e| ClawzError::Transport(format!("set_nodelay: {e}")))?;

        let peer_id = peer_addr.to_string();
        let (reader, writer) = stream.into_split();
        Ok(TransportConnection {
            peer_id,
            reader: Box::new(reader),
            writer: Box::new(writer),
        })
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

/// TCP binary-framing transport (LAN-optimised, gRPC-compatible framing).
pub struct GrpcTransport {
    config: GrpcConfig,
    pool: Arc<ConnectionPool>,
}

impl GrpcTransport {
    /// Create a new [`GrpcTransport`] from config.
    pub fn new(config: GrpcConfig) -> Self {
        let pool = Arc::new(ConnectionPool::new(
            config.pool_size,
            config.idle_timeout_secs,
        ));
        // Background eviction task runs at half the idle timeout (+1 s jitter)
        // so that stale connections are cleaned up without racing active reuse.
        {
            let pool_clone = Arc::clone(&pool);
            let idle_secs = config.idle_timeout_secs;
            tokio::spawn(async move {
                let interval = Duration::from_secs(idle_secs / 2 + 1);
                loop {
                    tokio::time::sleep(interval).await;
                    pool_clone.evict_stale().await;
                }
            });
        }
        Self { config, pool }
    }

    /// Derive a `host:port` address string from a [`PeerInfo`].
    fn peer_addr(peer: &PeerInfo) -> String {
        // Use mesh_ip; port comes from config default (workers all listen on same port)
        // Callers can embed port in mesh_ip as "ip:port".
        if peer.mesh_ip.contains(':') {
            peer.mesh_ip.clone()
        } else {
            format!("{}:{}", peer.mesh_ip, 50051_u16)
        }
    }
}

#[async_trait]
impl Transport for GrpcTransport {
    fn name(&self) -> &str {
        "grpc-tcp"
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        let addr = Self::peer_addr(peer);
        let connect_timeout = Duration::from_millis(self.config.connect_timeout_ms);
        let request_timeout = Duration::from_millis(self.config.request_timeout_ms);

        let mut stream = self.pool.acquire(&addr, connect_timeout).await?;

        let result = timeout(request_timeout, async {
            write_frame(&mut stream, payload).await?;
            read_frame(&mut stream).await
        })
        .await
        .map_err(|_| ClawzError::Transport(format!("request timeout to {addr}")))?;

        match result {
            Ok(response) => {
                // Return the connection to the pool for reuse.
                self.pool.release(&addr, stream).await;
                Ok(response)
            }
            Err(e) => {
                // Drop the stream on error — don't return a broken connection.
                drop(stream);
                Err(e)
            }
        }
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>> {
        let bind_addr: SocketAddr = addr
            .parse()
            .map_err(|e| ClawzError::Transport(format!("invalid listen addr '{addr}': {e}")))?;
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| ClawzError::Transport(format!("bind {addr}: {e}")))?;
        log::info!(target: "clawz::transport::grpc", "listening on {addr}");
        Ok(Box::new(GrpcListener { inner: listener }))
    }
}

// ── Server-side request handler ───────────────────────────────────────────────

/// Utility: drive a server-side request/response loop on an accepted connection.
///
/// `handler` receives the raw request bytes and must return response bytes.
/// This is used by tests and can be used by a gateway that wants to speak
/// the same framing protocol.
pub async fn serve_connection<F, Fut>(mut conn: TransportConnection, handler: F)
where
    F: Fn(Vec<u8>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Vec<u8>> + Send,
{
    loop {
        match read_frame(&mut conn.reader).await {
            Ok(req) => {
                let resp = handler(req).await;
                if let Err(e) = write_frame(&mut conn.writer, &resp).await {
                    log::warn!(target: "clawz::transport::grpc", "write response: {e}");
                    break;
                }
            }
            Err(e) => {
                // EOF or error — close connection
                log::debug!(target: "clawz::transport::grpc", "connection closed: {e}");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clawz_core::traits::TransportListener as _;
    use std::net::SocketAddr;
    use tokio::task;

    async fn echo_server(addr: &str) -> SocketAddr {
        let listener = TcpListener::bind(addr).await.unwrap();
        let bound = listener.local_addr().unwrap();
        task::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                loop {
                    match read_frame(&mut stream).await {
                        Ok(data) => {
                            if write_frame(&mut stream, &data).await.is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        });
        bound
    }

    #[tokio::test]
    async fn test_send_receive() {
        let server_addr = echo_server("127.0.0.1:0").await;
        let config = GrpcConfig {
            listen_port: server_addr.port(),
            ..Default::default()
        };
        let transport = GrpcTransport::new(config);
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), server_addr.to_string(), "test-host");
        let payload = b"hello grpc";
        let resp = transport.send(&peer, payload).await.unwrap();
        assert_eq!(resp, payload);
    }

    #[tokio::test]
    async fn test_listen_accept() {
        let config = GrpcConfig::default();
        let transport = GrpcTransport::new(config);
        let listener = transport.listen("127.0.0.1:0").await.unwrap();
        // Just verify we can create a listener without error.
        drop(listener);
    }

    #[tokio::test]
    async fn test_connection_reuse() {
        let server_addr = echo_server("127.0.0.1:0").await;
        let config = GrpcConfig {
            pool_size: 2,
            ..Default::default()
        };
        let transport = GrpcTransport::new(config);
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), server_addr.to_string(), "test-host");
        // First request — creates connection
        let r1 = transport.send(&peer, b"ping1").await.unwrap();
        assert_eq!(r1, b"ping1");
        // Second request — should reuse pooled connection
        let r2 = transport.send(&peer, b"ping2").await.unwrap();
        assert_eq!(r2, b"ping2");
    }
}
