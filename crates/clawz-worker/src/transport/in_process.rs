//! In-process transport — zero-network communication via tokio channels.
//!
//! Used when the gateway and worker run in the **same binary** (single-binary
//! deployment).  Message passing uses [`tokio::sync::mpsc`] so there is no
//! serialisation overhead for the framing layer, though payloads are still
//! passed as `Vec<u8>` to satisfy the [`Transport`] trait.
//!
//! Architecture
//! ------------
//! A global [`Registry`] maps peer IDs to sender halves of mpsc channels.
//! When a listener is created for a peer it registers itself in the registry.
//! `send()` looks up the target peer's channel, posts the request, and
//! awaits the response on a one-shot channel.
//!
//! # Role in Networking
//!
//! This transport is used in **Standalone** deployment mode when there is no
//! real mesh.  It allows the same [`Transport`] trait to be used everywhere
//! so that higher-level code (e.g. [`crate::mesh::manager::MeshManager`])
//! does not need a special "local-only" code path.
//!
//! # Key Dependencies
//!
//! - `clawz_core::traits::{Transport, TransportConnection, TransportListener}` — implemented here.
//! - `clawz_core::types::mesh::PeerInfo` — destination peer descriptor.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;

// Dependency: clawz-core::traits — Transport / TransportListener trait definitions.
use clawz_core::{
    error::{ClawzError, Result},
    traits::{Transport, TransportConnection, TransportListener},
    types::mesh::PeerInfo,
};
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};

// ── Message types ─────────────────────────────────────────────────────────────

/// A request posted through the in-process channel.
struct Request {
    /// Raw payload bytes to deliver to the peer.
    payload: Vec<u8>,
    /// One-shot channel for the peer to send its response back.
    reply_tx: oneshot::Sender<Vec<u8>>,
}

// ── Global registry ───────────────────────────────────────────────────────────

type RegistryMap = HashMap<String, mpsc::Sender<Request>>;

/// Shared registry of in-process listener endpoints.
///
/// Keyed by the `addr` string passed to `listen()`.
/// The registry is a `static` so that multiple `InProcessTransport` instances
/// can all reach the same set of listeners without explicit hand-off.
fn global_registry() -> &'static Arc<RwLock<RegistryMap>> {
    static REGISTRY: std::sync::OnceLock<Arc<RwLock<RegistryMap>>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
}

async fn register(addr: String, tx: mpsc::Sender<Request>) {
    global_registry().write().await.insert(addr, tx);
}

async fn deregister(addr: &str) {
    global_registry().write().await.remove(addr);
}

async fn lookup(addr: &str) -> Option<mpsc::Sender<Request>> {
    global_registry().read().await.get(addr).cloned()
}

// ── Listener ──────────────────────────────────────────────────────────────────

/// Receives inbound request messages and exposes them as [`TransportConnection`]s.
pub struct InProcessListener {
    /// The registered address (used for deregistration on drop).
    addr: String,
    /// Request receiver (wrapped in Mutex so [`TransportListener::accept`]
    /// can be called from an `&mut self` reference).
    rx: Mutex<mpsc::Receiver<Request>>,
}

impl Drop for InProcessListener {
    fn drop(&mut self) {
        let addr = self.addr.clone();
        // Spawn a detached task because Drop is synchronous but deregister
        // is async.  The 50 ms grace in tests is enough for this to complete.
        tokio::spawn(async move {
            deregister(&addr).await;
        });
    }
}

#[async_trait]
impl TransportListener for InProcessListener {
    async fn accept(&mut self) -> Result<TransportConnection> {
        let req = self
            .rx
            .lock()
            .await
            .recv()
            .await
            .ok_or_else(|| ClawzError::Transport("in-process listener closed".into()))?;

        // Create a duplex pair:
        // - The `TransportConnection` reader gives the caller the request payload.
        // - Anything written to the writer is sent back as the response.
        let (conn_tx, conn_rx) = tokio::io::duplex(req.payload.len().max(64) + 64);
        let (resp_tx, resp_rx) = tokio::io::duplex(64 * 1024);

        // Feed request payload into the reader side.
        let payload = req.payload.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut writer = conn_tx;
            let len = (payload.len() as u32).to_be_bytes();
            let _ = writer.write_all(&len).await;
            let _ = writer.write_all(&payload).await;
        });

        // Read response from writer side and forward to reply channel.
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = resp_rx;
            let mut len_buf = [0u8; 4];
            if reader.read_exact(&mut len_buf).await.is_err() {
                let _ = req.reply_tx.send(vec![]);
                return;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut buf = vec![0u8; len];
            if reader.read_exact(&mut buf).await.is_err() {
                let _ = req.reply_tx.send(vec![]);
                return;
            }
            let _ = req.reply_tx.send(buf);
        });

        Ok(TransportConnection {
            peer_id: format!("in-process@{}", self.addr),
            reader: Box::new(conn_rx),
            writer: Box::new(resp_tx),
        })
    }
}

// ── Transport ─────────────────────────────────────────────────────────────────

/// In-process transport — no network, uses tokio mpsc channels.
#[derive(Clone)]
pub struct InProcessTransport {
    /// Timeout for waiting on the one-shot reply channel.
    request_timeout_ms: u64,
}

impl InProcessTransport {
    /// Create a new in-process transport.
    pub fn new(request_timeout_ms: u64) -> Self {
        Self { request_timeout_ms }
    }
}

impl Default for InProcessTransport {
    fn default() -> Self {
        Self::new(5_000)
    }
}

#[async_trait]
impl Transport for InProcessTransport {
    fn name(&self) -> &str {
        "in-process"
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    async fn send(&self, peer: &PeerInfo, payload: &[u8]) -> Result<Vec<u8>> {
        // Derive the listener address from the peer's mesh_ip.
        // By convention the in-process listener registers itself under
        // "inproc://<mesh_ip>".
        let addr = format!("inproc://{}", peer.mesh_ip);

        let tx = lookup(&addr).await.ok_or_else(|| {
            ClawzError::Transport(format!("no in-process listener registered for '{addr}'"))
        })?;

        let (reply_tx, reply_rx) = oneshot::channel();
        tx.send(Request {
            payload: payload.to_vec(),
            reply_tx,
        })
        .await
        .map_err(|_| {
            ClawzError::Transport(format!(
                "in-process send to '{addr}' failed (channel closed)"
            ))
        })?;

        let response =
            tokio::time::timeout(Duration::from_millis(self.request_timeout_ms), reply_rx)
                .await
                .map_err(|_| {
                    ClawzError::Transport(format!("in-process response timeout from '{addr}'"))
                })?
                .map_err(|_| {
                    ClawzError::Transport(format!("in-process reply channel dropped for '{addr}'"))
                })?;

        Ok(response)
    }

    async fn listen(&self, addr: &str) -> Result<Box<dyn TransportListener>> {
        let (tx, rx) = mpsc::channel(256);
        let listener_addr = if addr.starts_with("inproc://") {
            addr.to_string()
        } else {
            format!("inproc://{addr}")
        };
        register(listener_addr.clone(), tx).await;
        log::info!(
            target: "clawz::transport::in_process",
            "in-process listener registered at '{listener_addr}'"
        );
        Ok(Box::new(InProcessListener {
            addr: listener_addr,
            rx: Mutex::new(rx),
        }))
    }
}

// ── once_cell ─────────────────────────────────────────────────────────────────

// We need once_cell for the static registry.  It ships with tokio's ecosystem;
// add it to the dependency list if not already present via tokio itself.
// (tokio depends on once_cell internally, but to be explicit we use it directly.)

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn simple_echo_server(addr: &str) {
        let transport = InProcessTransport::default();
        let mut listener = transport.listen(addr).await.unwrap();

        tokio::spawn(async move {
            loop {
                if let Ok(mut conn) = listener.accept().await {
                    tokio::spawn(async move {
                        // Read the length-prefixed request.
                        let mut len_buf = [0u8; 4];
                        if conn.reader.read_exact(&mut len_buf).await.is_err() {
                            return;
                        }
                        let len = u32::from_be_bytes(len_buf) as usize;
                        let mut payload = vec![0u8; len];
                        if conn.reader.read_exact(&mut payload).await.is_err() {
                            return;
                        }
                        // Echo back length-prefixed.
                        let resp_len = (payload.len() as u32).to_be_bytes();
                        let _ = conn.writer.write_all(&resp_len).await;
                        let _ = conn.writer.write_all(&payload).await;
                    });
                }
            }
        });
    }

    #[tokio::test]
    async fn test_inprocess_echo() {
        let addr = "test-echo-peer-1";
        simple_echo_server(addr).await;

        // Give the listener a moment to register.
        tokio::time::sleep(Duration::from_millis(10)).await;

        let transport = InProcessTransport::default();
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), addr, "test-host");
        let payload = b"hello in-process";
        let resp = transport.send(&peer, payload).await.unwrap();
        assert_eq!(resp, payload);
    }

    #[tokio::test]
    async fn test_missing_listener_returns_error() {
        let transport = InProcessTransport::default();
        let peer = PeerInfo::new(uuid::Uuid::new_v4(), "nonexistent-peer-xyz", "test");
        let result = transport.send(&peer, b"test").await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no in-process listener"));
    }

    #[tokio::test]
    async fn test_listener_deregisters_on_drop() {
        let addr = "drop-test-peer";
        {
            let transport = InProcessTransport::default();
            let _listener = transport.listen(addr).await.unwrap();
            // listener is alive here
            let entry = lookup(&format!("inproc://{addr}")).await;
            assert!(entry.is_some());
        }
        // After drop, the deregister runs asynchronously. Give it a tick.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let entry = lookup("inproc://drop-test-peer").await;
        assert!(entry.is_none());
    }
}
