# ClawZ

<p align="center">
  <img src="web/public/branding/clawz-logo-dark.png" alt="ClawZ" width="320" />
</p>

> A governed swarm of containerized AI agents — the reference implementation of the PRISM-G framework, in Rust.

**Brand assets:** `web/public/branding/` — use **silver** variants on dark backgrounds, **copper** on light. See [design system](crates/clawz-tauri/design/design-system.md).

[![License](https://img.shields.io/badge/License-ELv2-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)
[![MSRV](https://img.shields.io/badge/MSRV-1.87%2B-brightgreen.svg)]()

---

## Overview

**ClawZ** is a high-performance, enterprise-grade platform built in **Rust** by [Enterpryz Ventures](https://enterpryz.com) that runs a **governed swarm of containerized AI agents**. A 3-tier cascade — an Orchestrator spawns tenant-scoped Agent containers, and each Agent spawns its own Tool/MCP containers — lets fleets scale elastically while every action is governed. ClawZ is the open reference implementation of **PRISM-G**, Enterpryz Ventures' six-dimension framework for enterprise autonomous AI (Purpose · Reality · Infrastructure · Swarm · Memory & Metrics · Governance).

Designed with compliance and security as first-class concerns, ClawZ embeds the **PRISM-G** governance framework directly into the execution path of every agent action. Whether you are running a single autonomous agent or coordinating hundreds in a distributed fleet, ClawZ provides the runtime, observability, and guardrails required for production AI operations.

---

## Features

| Category | Capabilities |
|----------|-------------|
| **Agent Execution** | Pipeline-based runtime with reversible steps, subagent spawning, team coordination, and fan-out/fan-in parallelism. |
| **Governance (G dimension)** | Runtime guardrails, trust scoring, SHA-256 audit chain, council consensus, and SOC2/GDPR/EU AI Act export — the enforcement layer of PRISM-G's Governance dimension. |
| **Mesh Networking** | Multi-node clusters with leader election (quorum-based), heartbeat health scoring, service discovery (static, mDNS, API), and transport failover. |
| **Memory & RAG** | Conversation threading, embedding storage via `pgvector`, similarity search, and Retrieve-Augment-Generate pipelines. |
| **Multi-LLM Providers** | Pluggable provider system with native adapters for OpenAI (GPT-4), Anthropic (Claude 3), and local inference (Ollama, Llama.cpp). |
| **Tools Ecosystem** | Extensible tool registry with built-in capabilities: file I/O, HTTP, bash execution (sandboxed), Docker automation, browser automation (CDP), and MCP servers. |
| **Observability** | Prometheus metrics export, OpenTelemetry tracing, structured logging, and circuit-breaker health dashboards. |
| **Multi-Tenant Security** | API-key-based tenant isolation, RBAC, budget sub-leasing, and SHA-256 audit hash chains for tamper detection. |
| **Cloud Deployment** | 15 cloud adapters including AWS, Azure, GCP, Cloudflare Workers, Vercel, Fly.io, and more. |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    ClawZ Platform                            │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────┐  │
│  │   Gateway   │  │    Worker    │  │      Core        │  │
│  │  (HTTP API) │  │ (Execution)  │  │ (Types/Traits)   │  │
│  │  Axum + TLS │  │ Pipeline +   │  │ Async traits,    │  │
│  │ WebSocket   │  │ Governance + │  │ config, errors,  │  │
│  │ MCP Server  │  │ Mesh + RAG   │  │ circuit breakers │  │
│  └──────┬──────┘  └──────┬───────┘  └──────────────────┘  │
│         │                │                                  │
│         └────────────────┘                                  │
│                     Mesh Transport                           │
│         (gRPC / QUIC / WebSocket / IPC)                     │
└─────────────────────────────────────────────────────────────┘
                    │
        ┌───────────┼───────────┐
        ▼           ▼           ▼
   ┌─────────┐ ┌─────────┐ ┌─────────┐
   │Standalone│ │  Micro  │ │ Elastic │
   │  SQLite  │ │ Postgres│ │  Mesh   │
   │ 1 binary │ │ Docker  │ │  mDNS   │
   │localhost │ │Bollard  │ │  API    │
   └─────────┘ └─────────┘ └─────────┘
```

### Deployment Modes

| Mode | Topology | Best For | Communication | Database |
|------|----------|----------|--------------|----------|
| **Standalone** | Single binary | Local dev, embedded agents, edge devices | In-process channels | SQLite or local Postgres |
| **Micro** | Multi-container | Small teams, CI/CD pipelines, staging | gRPC / QUIC over Docker network | Shared Postgres + pgvector |
| **Elastic** | Full mesh | Production SaaS, multi-region, high availability | Multi-transport with automatic failover | Sharded Postgres, mDNS and API service discovery |

---

## Quick Start

### One-click install

| Platform | Command |
|----------|---------|
| **Linux / macOS** | `curl -fsSL https://raw.githubusercontent.com/improwyz/clawz/main/scripts/install.sh \| bash` |
| **Linux / macOS** (from clone) | `./install.sh` or `./scripts/install.sh` |
| **Windows (PowerShell)** | `irm https://raw.githubusercontent.com/improwyz/clawz/main/scripts/install.ps1 \| iex` |
| **Windows** (from clone) | `.\install.ps1` or `.\scripts\install.ps1` |

Options: `--docker` / `-Docker`, `--source` / `-Source`, `--with-web` / `-WithWeb`, `--dir PATH` / `-InstallDir PATH`.

After install, verify: `curl http://localhost:3000/api/v1/system/health`

Full install guide (Docker vs source, web dashboard, production, troubleshooting): **[INSTALL.md](INSTALL.md)**.

### Create your first agent

```bash
curl -X POST http://localhost:3000/api/v1/agents \
  -H "Content-Type: application/json" \
  -d '{
    "name": "hello-agent",
    "model": "stub",
    "system_prompt": "You are a helpful assistant."
  }'
```

---

## Configuration

ClawZ uses a layered configuration system:

1. **TOML file** — Set via `CLAWZ_CONFIG` environment variable.
2. **Environment overrides** — Using the `CLAWZ__SECTION__KEY` pattern.
3. **Required variables** — `CLAWZ_MODE` and `VALID_API_KEYS` must always be set.

### Example `config.toml`

```toml
[server]
bind = "0.0.0.0"
port = 3000

[database]
url = "postgresql://user:pass@localhost/clawz"
pool_size = 20
pgvector_dimensions = 1536

[mesh]
gossip_interval_ms = 1000
discovery = "mdns"  # "static", "mdns", or "api"

[governance]
compliance_level = "strict"  # "moderate" | "permissive"

[providers]
[providers.openai]
api_key = "sk-..."
rate_limit_rpm = 60
[providers.anthropic]
api_key = "sk-ant-..."
```

### Environment Overrides

```bash
export CLAWZ__SERVER__PORT=8080
export CLAWZ__DATABASE__URL="postgresql://prod-db/clawz"
export CLAWZ__PROVIDERS__OPENAI__RATE_LIMIT_RPM=120
```

---

## Deployment Modes in Detail

### Standalone

Ideal for development, prototyping, and single-node edge deployments.

- All subsystems run in a single process.
- No container runtime required.
- Uses SQLite or a local Postgres instance.
- No mesh networking overhead.

```bash
export CLAWZ_MODE=standalone
cargo run -p clawz-gateway
```

### Micro

Designed for containerized environments and small production clusters.

- Runs as multiple Docker containers orchestrated via the Bollard API.
- Workers communicate over gRPC/QUIC.
- Requires a shared Postgres instance with `pgvector` enabled.
- Automatic health probes and graceful container lifecycle management.

```bash
export CLAWZ_MODE=micro
export DATABASE_URL="postgresql://user:pass@postgres/clawz"
cargo run -p clawz-gateway
cargo run -p clawz-worker  # Run on multiple hosts
```

### Elastic

Built for high-scale, mission-critical SaaS platforms.

- Full mesh with dynamic node discovery via mDNS and gateway API.
- Leader election using quorum-based consensus.
- Transport-layer failover across gRPC, QUIC, and WebSocket.
- TLS 1.3+ enforced for all inter-node traffic.
- Sticky tenant routing and load shedding under pressure.

```bash
export CLAWZ_MODE=elastic
export CLAWZ_CONFIG="/etc/clawz/elastic.toml"
cargo run -p clawz-gateway --release
```

---

## Building & Testing

### Workspace Commands

```bash
# Debug build all crates
cargo build --workspace

# Optimized release build
cargo build --workspace --release

# Run all tests
cargo test --workspace

# Run tests with output
cargo test --workspace -- --nocapture

# Lint (zero warnings tolerance)
cargo clippy --workspace -- -D warnings

# Check formatting
cargo fmt --all -- --check
```

### Per-Crate Commands

```bash
# Fast typecheck
cargo check -p clawz-core
cargo check -p clawz-worker
cargo check -p clawz-gateway

# Run gateway locally
cargo run -p clawz-gateway

# Run worker node
cargo run -p clawz-worker
```

### MSRV

ClawZ requires **Rust 1.87+**.

```bash
# Verify MSRV compatibility
cargo +1.87 check --workspace
```

---

## Project Structure

The project is organized as a Cargo workspace with three primary crates:

```
clawz/
├── Cargo.toml                  # Workspace manifest
├── clawz-core/
│   └── src/
│       ├── lib.rs              # Public exports
│       ├── traits.rs           # 12 async trait definitions
│       ├── config.rs           # AppConfig + sub-configs
│       ├── error.rs            # ClawzError enum
│       ├── types/
│       │   ├── agent.rs        # Agent state machine
│       │   ├── cost.rs         # Budget & cost tracking
│       │   ├── governance.rs   # PRISM-G types
│       │   ├── message.rs      # Conversation threading
│       │   ├── tool.rs         # Tool definitions
│       │   ├── channel.rs      # Communication channels
│       │   ├── deploy.rs       # Deployment modes
│       │   └── mesh.rs         # Node discovery & health
│       ├── db.rs               # 8 repository implementations
│       ├── metrics.rs          # Prometheus instrumentation
│       └── circuit_breaker.rs  # Lock-free fault tolerance
├── clawz-worker/
│   └── src/
│       ├── runtime/
│       │   ├── agent.rs        # Single-agent execution
│       │   ├── pipeline.rs     # 5-step reversible pipeline
│       │   ├── team.rs         # Multi-agent coordination
│       │   ├── subagent.rs     # Spawn child agents
│       │   └── fan_out.rs      # Parallel execution
│       ├── governance/
│       │   ├── engine.rs # Governance engine (policy+trust+guardrails+approval)
│       │   ├── guardrails.rs # Governance-dimension runtime guardrails (Vol 9)
│       │   ├── policy.rs       # Hot-reloadable policies
│       │   ├── trust.rs        # 5-tier trust scoring
│       │   ├── approval.rs     # Approval workflows
│       │   ├── council.rs      # Multi-approver councils
│       │   ├── audit.rs        # SHA-256 hash chain
│       │   └── compliance.rs   # SOC2 / GDPR / EU-AI-Act reports
│       ├── mesh/
│       │   ├── fleet.rs        # Leader election
│       │   ├── heartbeat.rs    # EWMA RTT & health
│       │   ├── router.rs       # Inter-node routing
│       │   ├── discovery.rs    # Service discovery backends
│       │   └── firewall.rs     # Network policies
│       ├── transport/          # gRPC, QUIC, WebSocket, IPC
│       ├── memory/
│       │   ├── store.rs        # Conversation storage
│       │   ├── rag.rs          # pgvector similarity search
│       │   ├── embedding.rs    # Text embedding providers
│       │   ├── conversation.rs # Context window management
│       │   └── blackboard.rs   # Shared team state
│       ├── providers/
│       │   ├── registry.rs     # Provider lookup
│       │   ├── router.rs       # Retry + circuit breaker routing
│       │   ├── cost_budget.rs  # Budget enforcement
│       │   └── adapters/       # OpenAI, Anthropic, Ollama
│       ├── tools/
│       │   ├── registry.rs     # Tool registration & validation
│       │   ├── browser.rs      # CDP web automation
│       │   ├── docker.rs       # Container lifecycle
│       │   ├── mcp.rs          # Model Context Protocol
│       │   └── builtin/        # File, HTTP, bash (sandboxed)
│       ├── channels/           # WebSocket, gRPC, Slack, etc.
│       ├── hardware/           # GPU detection & model loading
│       ├── orchestration/      # Docker / standalone schedulers
│       └── observability/      # OpenTelemetry context propagation
└── clawz-gateway/
    └── src/
        ├── server.rs           # Axum HTTP server + TLS
        ├── shutdown.rs         # Graceful cancellation cascade
        ├── auth/               # Bearer token API key validation
        ├── routes/             # REST endpoints
        ├── ws/                 # WebSocket agent interaction
        ├── mcp/                # Model Context Protocol server
        ├── deploy/             # 15 cloud deployment adapters
        ├── scheduling/         # Admission control & tenant routing
        ├── tui/                # Terminal UI for debugging
        ├── cloudflare/         # Edge deployment integration
        └── connectors/         # 31 SaaS integrations
```

---

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `CLAWZ_MODE` | **Yes** | Deployment mode: `standalone`, `micro`, or `elastic`. |
| `VALID_API_KEYS` | **Yes** | Comma-separated API keys for authentication. |
| `DATABASE_URL` | For `micro`/`elastic` | PostgreSQL connection string. |
| `CLAWZ_CONFIG` | No | Path to TOML configuration file. |

### Example

```bash
export CLAWZ_MODE=micro
export VALID_API_KEYS="prod-key-alpha,prod-key-beta"
export DATABASE_URL="postgresql://clawz:secret@db.internal:5432/clawz"
export CLAWZ_CONFIG="/etc/clawz/production.toml"
```

---

## Tech Stack

| Layer | Technology |
|-------|------------|
| **Language** | Rust 2024 Edition (MSRV 1.87+) |
| **Async Runtime** | [tokio](https://tokio.rs) |
| **Web Framework** | [axum](https://docs.rs/axum) |
| **Serialization** | [serde](https://serde.rs) |
| **Error Handling** | [thiserror](https://docs.rs/thiserror) |
| **Database** | [sqlx](https://github.com/launchbadge/sqlx) |
| **Vector Search** | [pgvector](https://github.com/pgvector/pgvector) + [pgvector-rs](https://github.com/pgvector/pgvector-rust) |
| **Container Orchestration** | [bollard](https://github.com/fussybeaver/bollard) (Docker API) |
| **Transports** | gRPC, QUIC, WebSocket (WSS), in-process IPC |
| **Observability** | Prometheus, OpenTelemetry W3C context propagation |
| **TLS** | rustls / system-native TLS |

---

## License

This project is licensed under the **Elastic License 2.0 (ELv2)**.

See [LICENSE](LICENSE) for the full license text.

---

## Maintainers

Developed and maintained by **Enterpryz Ventures**.

For questions, support, or enterprise inquiries, please contact the team or open an issue in this repository.

---

<p align="center">
  <em>Built for agents. Engineered for scale. Governed by design.</em>
</p>
