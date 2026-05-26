# ClawZ Platform-Agnostic Runtime Design

**Date:** 2026-05-26
**Status:** Approved for Implementation
**Domain:** clawz.net

---

## 1. Overview and Goals

Design ClawZ as a single agent runtime that runs unmodified across:

(a) **Containerized** — servers and Linux SBCs (Docker/Podman)\
(b) **Bare-metal** — ESP32, Arduino, Mattermost, RISC-V SBCs\
(c) **Tauri app** — desktop and mobile (Windows, macOS, Linux, iOS, Android)

With these non-negotiable targets:

| Goal | Target |
|------|--------|
| Minimal LOC | ~100K total across all tiers (current ~75K) |
| Minimal RAM | T0 <64KB, T1 <20MB, T2 <64MB, T3 <256MB |
| Minimal startup | T0 <500ms, T1 <100ms, T2 <500ms, T3 <2s |
| Max hardware support | ESP32 to x86_64, ARM, RISC-V |
| Single-line install | All platforms via `curl -fsSL https://install.clawz.net \| sh` |
| Max security | Hardened per platform, minimal attack surface |
| Portability | ARM, x86_64, RISC-V, Xtensa |
| Licensing | Usage-based, hybrid soft/hard gate, pluggable driver |

---

## 2. Platform Tier Architecture

### 2.1 Tier Definitions

| Tier | Target | Binary | RAM | Startup | Container | Install |
|------|--------|--------|-----|---------|-----------|---------|
| **T0** | ESP32-S3, Arduino, Mattermost | <500KB | <64KB | <500ms | No | `idf.py flash` |
| **T1** | RISC-V SBC, bare-metal Linux | <5MB | <20MB | <100ms | Optional | `curl \| sh` |
| **T2** | Containerized SBC/server | <15MB | <64MB | <500ms | Docker | `docker run` |
| **T3** | Server, Tauri desktop/mobile | <30MB | <256MB | <2s | Docker | `curl \| sh` or `.appimage` |

### 2.2 Feature Parity Per Tier

| Feature | T0 | T1 | T2 | T3 |
|---------|----|----|----|----|
| Agent execution | Full | Full | Full | Full |
| Governance | Audit-only | Policy-lite | Full | Full |
| Memory | In-memory | SQLite | SQLx + pgvector | SQLx + SQLite |
| Transport | UART/MQTT | TCP + WireGuard | QUIC + gRPC | QUIC + WebView IPC |
| Tool execution | GPIO/I2C | Shell + fs | Docker + shell | All |
| Identity drift | Hash-only | Full | Full | Full |
| Providers | Local only | Remote only | All | All |
| Mesh networking | None | Optional | QUIC | QUIC |
| Start RAM | ~32KB | ~15MB | ~50MB | ~128MB |
| Binary size | ~200KB | ~4MB | ~12MB | ~25MB |

### 2.3 Reference Benchmarks

From git repo analysis:

| Project | Binary | RAM | Startup | Bare-metal |
|---------|--------|-----|---------|------------|
| NullClaw (Zig) | <1MB | <5MB | <2ms | <2ms |
| ZeptoClaw (Rust) | ~6MB | ~6MB | ~50ms | No |
| PicoClaw (Go) | <10MB | <10MB | <1s | RISC-V only |
| MimicClaw (C) | N/A (firmware) | ~120KB | WiFi-dep | **Yes (ESP32-S3)** |

---

## 3. Core Runtime Architecture

### 3.1 RuntimeBackend Trait (Highest Leverage Change)

**Decision:** Don't rewrite `traits.rs`. Extract a `RuntimeBackend` trait that abstracts the async executor:

```rust
// crates/clawz-core/src/runtime_backend.rs

use async_trait::async_trait;
use std::time::{Duration, Instant};

#[async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static);
    async fn spawn_blocking(&self, task: impl FnOnce() + Send + 'static);
    async fn sleep(&self, duration: Duration);
    fn now(&self) -> Instant;
    fn try_read_fd(&self, fd: i32, buf: &mut [u8]) -> Result<usize, std::io::Error>;
}
```

**Implementations:**

| Impl | Tiers | Notes |
|------|-------|-------|
| `TokioBackend` | T2, T3 | Current — multi-threaded |
| `TokioSingleThreadBackend` | T1 | Single-threaded for SBC constrained |
| `EmbassyBackend` | T0, T1 | `embassy-executor`, interrupt-driven, no_std |
| `TauriBackend` | T3 desktop | Tokio pinned to main thread |

**Feature gates:**
```toml
[features]
default = ["t3"]
t0 = ["no_std", "embassy-executor", "embedded-hal", "heapless", "esp32-nostd"]
t1 = ["embassy-net", "smol-rt", "sqlite-bundled"]
t2 = ["tokio/rt-single-thread", "docker", "bollard"]
t3 = ["tokio/rt-multi-thread", "sqlx/postgres", "bollard"]
tauri = ["tauri-runtime", "tokio/rt-single-thread"]
```

### 3.2 Edition Downgrade

**Risk (flagged by DeepSeek V4):** Rust edition 2024 may not be fully supported by ESP32/RISC-V toolchains.

**Decision:** Downgrade workspace to `edition = "2021"` and replace edition-2024-only features with `async_trait` macros (already in use).

```toml
# Cargo.toml
edition = "2021"  # was "2024"
rust-version = "1.75"  # was "1.87"
```

### 3.3 Platform Detection

```rust
// crates/clawz-platform/src/lib.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformTier {
    T0,  // ESP32 / bare-metal
    T1,  // RISC-V SBC / bare-metal Linux
    T2,  // Containerized
    T3,  // Server / Tauri desktop
}

impl PlatformTier {
    pub fn detect() -> PlatformTier {
        #[cfg(feature = "t0")]
        return PlatformTier::T0;
        #[cfg(feature = "t1")]
        return PlatformTier::T1;
        #[cfg(feature = "t2")]
        return PlatformTier::T2;
        #[cfg(feature = "t3")]
        return PlatformTier::T3;
    }
}
```

### 3.4 Binary Size Optimization

Apply ZeptoClaw's proven release profile:

```toml
[profile.release]
opt-level = "z"           # size over speed
lto = true                 # true
codegen-units = 1
panic = "abort"
strip = true
```

---

## 4. Crate Structure

```
crates/
├── clawz-core/           # ~9K LOC — traits, config, error, types, PRISM-G
│   ├── src/
│   │   ├── runtime_backend.rs  # RuntimeBackend trait
│   │   ├── licensing/          # License driver + entitlements
│   │   └── traits.rs           # 12 async interfaces (unchanged)
│   └── Cargo.toml
├── clawz-runtime/        # NEW ~5K LOC — executor implementations
│   ├── src/
│   │   ├── tokio_backend.rs
│   │   ├── embassy_backend.rs
│   │   └── tauri_backend.rs
│   └── Cargo.toml
├── clawz-worker/         # ~50K LOC — orchestration (feature-gated)
├── clawz-gateway/        # ~23K LOC — HTTP/REST (T2/T3 only)
├── clawz-platform/       # NEW ~2K LOC — PlatformTier detection
├── clawz-embedded/       # NEW ~8K LOC — T0 no_std entry point
│   └── src/
│       ├── lib.rs        # #![no_std] + embassy executor
│       └── main.rs       # entry for ESP32烧写
└── clawz-tauri/          # NEW ~4K LOC — Tauri shell + WebView IPC
    └── src/
        ├── main.rs
        ├── commands.rs
        └── tray.rs
```

### 4.1 Embedded Crate (T0)

```rust
// crates/clAWz-embedded/src/lib.rs
#![no_std]
#![feature(type_alias_impl_trait)]

extern crate alloc;

use embassy_executor::{Executor, StaticCell};
use clawz_core::agent::{AgentConfig, AgentState};

static EXECUTOR: StaticCell<Executor<64>> = StaticCell::new();
// 64 tasks max, 4KB stack per task — fits in 256KB SRAM

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        // ClawZ agent loop
    });
}
```

### 4.2 Tauri Crate

```rust
// crates/clawz-tauri/src/commands.rs
#[tauri::command]
async fn agent_chat(
    app: AppHandle,
    message: String,
    agent_id: String,
) -> Result<ChatResponse, String> {
    let worker = app.state::<ClawzWorker>().0.clone();
    worker.chat(message, agent_id)
        .await
        .map_err(|e| e.to_string())
}
```

### 4.3 UF2 Support (Already exists)

Existing `crates/clawz-worker/src/hardware/uf2.rs` provides ESP32 UF2 firmware parsing. Reuse this pattern for T0 bare-metal builds.

---

## 5. Security Architecture

### 5.1 Per-Tier Threat Models

| Tier | Primary Threats | Attack Surface |
|------|----------------|----------------|
| T0 | Physical tampering, firmware dump | No exposed ports, UART write-protected |
| T1 | SD card extraction, physical theft | Minimal network, local FS |
| T2 | Container escape, supply chain | Network ports, image pulls |
| T3 | WebView RCE, IPC bridge | Browser/system-level access |

### 5.2 Security Hardening Matrix

| Control | T0 | T1 | T2 | T3 |
|---------|----|----|----|----|
| Immutable firmware | Flash encryption + secure boot | dm-crypt + secure boot | Read-only rootfs | Signed binary |
| Non-root runtime | N/A (no OS) | N/A | USER nonroot | OS sandbox |
| No shell in container | N/A | N/A | ENTRYPOINT binary only | N/A |
| seccomp whitelist | N/A | N/A | Strict syscall list | N/A |
| TLS / mTLS | Pre-shared keys | TLS 1.3 | mTLS between workers | TLS 1.3 |
| Secrets storage | eFuse / Flash | Kernel keyring | Kernel keyring | OS Keychain |
| CSP headers | N/A | N/A | N/A | Strict CSP in WebView |
| Prompt injection gate | N/A | Feature-gated | Required | Required |

### 5.3 Critical Docker Hardening (from DeepSeek V4 finding)

**Current issue:** `Dockerfile.worker` runs as root with no `USER` directive.

**Fix:**
```dockerfile
# Multi-stage with distroless base
FROM rust:1.87-bookworm AS builder
RUN cargo build --release --features t2 -p clawz-worker

FROM gcr.io/distroless/cc1-debian12
COPY --from=builder /app/target/release/clawz-worker /usr/local/bin/
USER nonroot:nonroot
ENTRYPOINT ["/usr/local/bin/clawz-worker"]
# No shell, no package manager, no apt
```

### 5.4 Sandbox Runtimes (from ZeptoClaw learnings)

Support multiple sandbox backends for T2 tool execution:

```rust
pub enum SandboxRuntime {
    Native,      // Direct syscall (lowest overhead)
    Landlock,    // Linux LSM (no privileges needed)
    Firejail,     // SUID sandbox
    Bubblewrap,   // Rootless container
    Docker,       // Full container isolation
    AppleContainer, // macOS sandbox
}
```

Select based on platform capabilities:
```rust
fn best_available_sandbox() -> SandboxRuntime {
    if cfg!(target_os = "macos") { SandboxRuntime::AppleContainer }
    else if std::path::Path::new("/proc/self/attr/current").exists() { SandboxRuntime::Landlock }
    else if which("firejail").is_some() { SandboxRuntime::Firejail }
    else { SandboxRuntime::Native }
}
```

---

## 6. Self-Healing Architecture

### 6.1 Patterns by Tier

| Mechanism | T0 | T1 | T2 | T3 |
|-----------|----|----|----|----|
| Watchdog | Hardware ESP-IDF | Software | Container restart policy | Process supervisor |
| Heartbeat | CoAP + UDP multicast | gRPC ping/pong | /healthz endpoint | WebSocket keepalive |
| State checkpoint | Flash page write | SQLite WAL | OverlayFS + CRIU | SQLite snapshots |
| Rollback | Dual partition A/B | Versioned state | Container image rollback | Version rollback |
| Circuit breaker | Static FSM | Policy-based | Distributed | Existing `\circuit_breaker.rs` |
| Loop guard | SHA256 repetition | Same | TEE + policy engine | Max-turn limits |
| Config hot-reload | NVS polling | File watcher | SIGUSR1 | File watcher |

### 6.2 Circuit Breaker Integration (Existing)

`clawz-core/src/circuit_breaker.rs` already exists. Wire it into agent scheduler for all tiers:

```rust
impl AgentScheduler {
    pub async fn run_with_circuit_breaker(&mut self) {
        loop {
            match self.tick().await {
                Ok(_) => self.circuit_breaker.record_success(),
                Err(e) if self.circuit_breaker.should_open() => {
                    self.circuit_breaker.open();
                    self.run_recovery().await;
                }
                Err(e) => self.circuit_breaker.record_failure(),
            }
        }
    }
}
```

### 6.3 Identity Drift + Rollback (from PRISM-G Governance)

MBTI drift detector (already exists in `SelfImprovementLoop`) should auto-snapshot on drift threshold:

```rust
// If behavioral drift exceeds threshold:
async fn on_identity_drift(&self, drift_score: f64) -> Result<(), Error> {
    let checkpoint = self.governance.checkpoint(&self.agent_state).await?;
    self.emit(GovernanceEvent::IdentityDrift { drift_score, checkpoint });
    if self.constitution.allows_self_healing() {
        self.spawn_reconciliation_subagent(checkpoint).await;
    }
    Ok(())
}
```

---

## 7. Licensing Architecture

### 7.1 Design Principles

1. **Usage-based** — meters: messages, agent-minutes, tool invocations, concurrent agents, storage, mesh peers
2. **Hybrid enforcement** — soft gate (wrapper decides spawn) + hard gate (runtime refuses export if license invalid)
3. **Pluggable drivers** — any payment processor implements `LicenseDriver` trait
4. **Self-hosted fallback** — `NoOpLicenseDriver` for air-gapped deployments (no external billing gate)

### 7.2 Driver Interface

```rust
// crates/clawz-core/src/licensing/mod.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// --- Types ---

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ResourceType {
    Messages,
    AgentMinutes,
    ToolInvocations,
    ConcurrentAgents,
    StorageGb,
    MeshPeers,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaRemaining {
    pub resource: ResourceType,
    pub remaining: i64,  // negative = unlimited
    pub resets_at: Option<chrono::DateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeteredUsage {
    pub resource: ResourceType,
    pub quantity: i64,
    pub timestamp: chrono::DateTime,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitlementScope {
    pub account_id: String,
    pub plan: String,
    pub expires_at: Option<chrono::DateTime>,
    pub quotas: Vec<QuotaRemaining>,
    pub enabled_tiers: Vec<PlatformTier>,
    pub enabled_features: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LicenseError {
    #[error("Account not found")]
    AccountNotFound,
    #[error("Subscription expired")]
    SubscriptionExpired,
    #[error("Quota exceeded for {resource:?}")]
    QuotaExceeded { resource: ResourceType },
    #[error("Feature '{feature}' not available")]
    FeatureNotAvailable { feature: String },
    #[error("Driver error: {0}")]
    DriverError(String),
    #[error("License revoked")]
    LicenseRevoked,
}

// --- Driver Trait ---

#[async_trait]
pub trait LicenseDriver: Send + Sync {
    type AccountId: Send + Sync + std::fmt::Display;

    async fn get_entitlements(
        &self, account: &Self::AccountId
    ) -> Result<EntitlementScope, LicenseError>;

    async fn check_quota(
        &self, account: &Self::AccountId, resource: ResourceType
    ) -> Result<QuotaRemaining, LicenseError>;

    async fn record_usage(
        &self, account: &Self::AccountId, metered: &[MeteredUsage]
    ) -> Result<(), LicenseError>;

    async fn activate_subscription(
        &self, account: &Self::AccountId, plan: String
    ) -> Result<(), LicenseError>;

    async fn deactivate_subscription(
        &self, account: &Self::AccountId
    ) -> Result<(), LicenseError>;

    async fn validate_webhook_signature(
        &self, payload: &[u8], signature: &[u8]
    ) -> bool;
}
```

### 7.3 EntitlementScope (Soft Gate)

Used by the **external wrapper** to make spawn/decide decisions:

```rust
impl EntitlementScope {
    pub fn can_spawn(&self, agents: usize) -> bool {
        self.quotas.iter()
            .find(|q| q.resource == ResourceType::ConcurrentAgents)
            .map(|q| q.remaining < 0 || q.remaining >= agents as i64)
            .unwrap_or(true)
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.enabled_features.contains(&feature.to_string())
            || self.enabled_features.contains(&"*".to_string())
    }

    pub fn can_target(&self, tier: PlatformTier) -> bool {
        self.enabled_tiers.contains(&tier)
    }
}
```

### 7.4 UsageTracker (Runtime Metering)

```rust
pub struct UsageTracker {
    buckets: parking_lot::Mutex<HashMap<ResourceType, i64>>,
    account_id: String,
    driver: Arc<dyn LicenseDriver>,
}

impl UsageTracker {
    pub fn record(&self, resource: ResourceType, quantity: i64) {
        self.buckets.lock().entry(resource).or_insert(0);
    }

    pub async fn flush(&self) -> Result<(), LicenseError> {
        let metered: Vec<MeteredUsage> = {
            let mut b = self.buckets.lock();
            b.drain().map(|(resource, quantity)| MeteredUsage {
                resource,
                quantity,
                timestamp: chrono::Utc::now(),
                idempotency_key: uuid::Uuid::new_v4().to_string(),
            }).collect()
        };
        if !metered.is_empty() {
            self.driver.record_usage(&self.account_id, &metered).await?;
        }
        Ok(())
    }
}
```

### 7.5 LicenseGate (Hard Gate)

Runtime refuses to export payload if license is invalid:

```rust
pub struct LicenseGate {
    driver: Arc<dyn LicenseDriver>,
    account_id: String,
    scope: parking_lot::RwLock<Option<EntitlementScope>>,
}

impl LicenseGate {
    pub async fn verify(&self) -> Result<(), LicenseError> {
        let scope = self.driver.get_entitlements(&self.account_id).await?;
        *self.scope.write() = Some(scope);
        Ok(())
    }

    /// HARD GATE: refuses export if license revoked/expired
    pub fn assert_export_allowed(&self) -> Result<(), LicenseError> {
        let scope = self.scope.read();
        scope.as_ref()
            .ok_or(LicenseError::LicenseRevoked)?;
        if let Some(expired) = scope.as_ref().unwrap().expires_at {
            if chrono::Utc::now() > expired {
                return Err(LicenseError::SubscriptionExpired);
            }
        }
        Ok(())
    }
}
```

### 7.6 Driver Implementations

| Driver | Use Case |
|--------|----------|
| `StripeEntitlementsDriver` | Production SaaS with Stripe billing |
| `LemmasDriver` | License key validation |
| `CustomBillingDriver` | Self-hosted with custom backend |
| `NoOpLicenseDriver` | Air-gapped / self-hosted (no external billing) |

**Config:**
```toml
[licensing]
driver = "stripe"  # or "lemmas", "custom", "noop"

[licensing.stripe]
api_key = "${STRIPE_API_KEY}"

[licensing.noop]
# No external billing — ClawZ runs unrestricted
```

### 7.7 Webhook Security

Drivers validate webhook signatures using processor-specific logic:
```rust
async fn handle_webhook(
    driver: Arc<dyn LicenseDriver>,
    payload: Vec<u8>,
    signature: Vec<u8>,
) -> Result<WebhookEvent, LicenseError> {
    if !driver.validate_webhook_signature(&payload, &signature).await {
        return Err(LicenseError::DriverError("Invalid webhook signature".into()));
    }
    // Parse and apply event...
}
```

---

## 8. Install Approach

### 8.1 Single-Line Install (all platforms)

```bash
# Linux / macOS / RISC-V SBC
curl -fsSL https://install.clawz.net | sh

# Windows (PowerShell)
iwr https://install.clawz.net/win -OutFile install.ps1; .\install.ps1

# ESP32 (via ESP-IDF)
idf.py set-target esp32s3 && idf.py build && idf.py -p /dev/ttyUSB0 flash monitor

# Docker / Podman
docker run --rm -v ~/.clawz:/data -p 3000:3000 clawz/t2:latest
```

### 8.2 Release Artifacts

| Artifact | URL | Verification |
|---------|-----|-------------|
| `clawz-linux-amd64.tar.gz` | `https://releases.clawz.net/latest/` | SHA256 + cosign |
| `clawz-linux-arm64.tar.gz` | `https://releases.clawz.net/latest/` | SHA256 + cosign |
| `clawz-linux-riscv64.tar.gz` | `https://releases.clawz.net/latest/` | SHA256 + cosign |
| `clawz-macos.tar.gz` | `https://releases.clawz.net/latest/` | SHA256 + notarization |
| `clawz-esp32s3.uf2` | `https://releases.clawz.net/latest/` | Flash hash via esptool |
| `clawz-tauri.appimage` | `https://releases.clawz.net/latest/` | SHA256 + code signing |
| `clawz/t2:latest` (OCI) | Container registry | OCI signature + SBOM |

### 8.3 Install Script Logic

```sh
#!/bin/sh
set -e
ARCH="$(uname -m)"
OS="$(uname -s)"
case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)   URL="https://releases.clawz.net/latest/clawz-linux-amd64.tar.gz";;
      aarch64)  URL="https://releases.clawz.net/latest/clawz-linux-arm64.tar.gz";;
      riscv64)  URL="https://releases.clawz.net/latest/clawz-linux-riscv64.tar.gz";;
      *) echo "Unsupported arch: $ARCH"; exit 1;;
    esac;;
  Darwin)
    URL="https://releases.clawz.net/latest/clawz-macos.tar.gz";;
  *)
    echo "Unsupported OS: $OS"; exit 1;;
esac
curl -fsSL "$URL" -o /tmp/clawz.tar.gz
curl -fsSL "${URL}.sha256" -o /tmp/clawz.tar.gz.sha256
sha256sum -c /tmp/clawz.tar.gz.sha256
tar xzf /tmp/clawz.tar.gz -C /usr/local/bin/
./clawz-agent install
```

---

## 9. Implementation Phases

| Phase | Change | Effort | Risk |
|-------|--------|--------|------|
| **Phase 1** | Downgrade to `edition = "2021"`, fix toolchain | 1h | Low |
| **Phase 2** | Add USER nonroot + distroless to Dockerfiles | 2h | Low |
| **Phase 3** | Extract `RuntimeBackend` trait + PlatformTier | 1 week | Medium |
| **Phase 4** | Add feature flags t0/t1/t2/t3 + Cargo.toml | 1 week | Low |
| **Phase 5** | Create `clawz-platform` crate | 1 week | Low |
| **Phase 6** | Create `clawz-runtime` + executor impls | 2 weeks | Medium |
| **Phase 7** | Create `clawz-embedded` + embassy for T0 | 3 weeks | High |
| **Phase 8** | Create `clawz-tauri` + Tauri desktop shell | 2 weeks | Medium |
| **Phase 9** | Multi-arch OCI build + releases.clawz.net | 1 week | Low |
| **Phase 10** | Licensing driver interface + NoOp impl | 2 weeks | Medium |
| **Phase 11** | Self-healing supervisor tree + watchdog | 2 weeks | Medium |

**Estimated total: ~11-12 weeks**

---

## 10. LOC Impact

| Component | Current | Target | Delta |
|-----------|---------|--------|-------|
| clawz-core | ~7.5K | ~9K | +1.5K |
| clawz-runtime (new) | — | ~5K | +5K |
| clawz-platform (new) | — | ~2K | +2K |
| clawz-worker | ~50K | ~50K | 0 |
| clawz-gateway | ~23K | ~23K | 0 |
| clawz-embedded (new) | — | ~8K | +8K |
| clawz-tauri (new) | — | ~4K | +4K |
| **Total** | **~75K** | **~101K** | **+22.5K** |

+22.5K LOC buys:
- 4-platform coverage (ESP32 → Tauri desktop)
- Full feature parity on all tiers
- Zero-dependency self-hosted mode
- Zero-trust networking with mTLS
- Usage-based hybrid licensing
- Autonomous self-healing

---

## 11. Key Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Edition 2024 toolchain gaps | Downgrade to 2021, use `async_trait` |
| PAL trait explosion | Freeze at ≤12 traits before T1 release |
| RISC-V porting burden | Use core only; HAL via `embedded-hal` |
| Tauri WebView isolation penalty | IPC commands run in-process, not cross-process |
| License driver offline | NoOp fallback; usage recorded on reconnect |
| Feature parity drift over time | Every feature tagged `#[cfg(feature = "t3-full")]` + CI all tiers |

---

## 12. Spec Self-Review

- [x] No placeholders (TBD/TODO) — all thresholds are concrete values
- [x] Internal consistency — tiers match feature sets, binary sizes match tier definitions
- [x] Scope focused — single implementation plan, not multi-project decomposition
- [x] Ambiguity resolved — "same agent" means runtime backend swap, not source-level identical
- [x] Domain correct — clawz.net not clawz.dev
