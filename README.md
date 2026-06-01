# ClawZ

<p align="center">
  <img src="web/public/branding/clawz-logo-dark.png" alt="ClawZ" width="320" />
</p>

> A governed swarm of containerized AI agents — the reference implementation of the PRISM-G framework, in Rust.

[![License](https://img.shields.io/badge/License-ELv2-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org/)
[![MSRV](https://img.shields.io/badge/MSRV-1.87%2B-brightgreen.svg)]()

---

## Overview

**ClawZ** is a high-performance, enterprise-grade platform built in **Rust** by [Enterpryz Ventures](https://enterpryz.com) that runs a **governed swarm of containerized AI agents**. A 3-tier cascade — an Orchestrator spawns tenant-scoped Agent containers, and each Agent spawns its own Tool/MCP containers — lets fleets scale elastically while every action is governed. ClawZ is the open reference implementation of **PRISM-G**, Enterpryz Ventures' six-dimension framework for enterprise autonomous AI (Purpose · Reality · Infrastructure · Swarm · Memory & Metrics · Governance).

Designed with compliance and security as first-class concerns, ClawZ embeds the **PRISM-G** governance framework directly into the execution path of every agent action. Whether you are running a single autonomous agent or coordinating hundreds in a distributed fleet, ClawZ provides the runtime, observability, and guardrails required for production AI operations.

The codebase is a **Cargo workspace of eight crates** (`clawz-core`, `clawz-platform`, `clawz-runtime`, `clawz-embedded`, `clawz-services`, `clawz-worker`, `clawz-gateway`, `clawz-tauri`) under `crates/`, plus an optional React dashboard in `web/`.

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
| **Agent Rooms** | Multi-participant rooms (1-many, many-1 hybrid), sequenced messages, async agent turns, side-threads, orchestration binding. |
| **Channels & users** | Tenant-scoped channel registry, user registration, API keys persisted to Postgres. |
| **Cloud Deployment** | 18 cloud deploy adapters (Fly.io, Railway, AWS Lambda, Azure Functions, GCP Cloud Run, Kubernetes, Cloudflare, Vercel, and more). |
| **Desktop App** | Tauri 2 shell (`clawz-tauri`) for local agent control without a separate HTTP gateway. |
| **Embedded (T0)** | `no_std` Embassy backend for ESP32-class devices via `clawz-embedded`. |
| **Platform Tiers** | T0 (bare metal) through T3 (server/desktop) with compile-time feature flags and runtime detection. |

---

## Architecture

### Runtime (3-tier services)

Production deployments center on three cooperating **services** (not to be confused with workspace crate count):

```
┌─────────────────────────────────────────────────────────────┐
│                    ClawZ Platform                           │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────────┐    │
│  │   Gateway   │  │    Worker    │  │      Core        │    │
│  │  (HTTP API) │  │ (Execution)  │  │ (Types/Traits)   │    │
│  │  Axum + TLS │  │ Pipeline +   │  │ Async traits,    │    │
│  │ WebSocket   │  │ Governance + │  │ config, errors,  │    │
│  │ MCP Server  │  │ Mesh + RAG   │  │ circuit breakers │    │
│  └──────┬──────┘  └──────┬───────┘  └──────────────────┘    │
│         │    clawz-services (DTOs, execution client)        │
│         └────────────────┘                                  │
│                     Mesh Transport                          │
│         (gRPC / QUIC / WebSocket / IPC)                     │
└─────────────────────────────────────────────────────────────┘
                    │
        ┌───────────┼───────────┐
        ▼           ▼           ▼
   ┌──────────┐ ┌─────────┐ ┌─────────┐
   │Standalone│ │  Micro  │ │ Elastic │
   │  SQLite  │ │ Postgres│ │  Mesh   │
   │ 1 binary │ │ Docker  │ │  mDNS   │
   │localhost │ │Bollard  │ │  API    │
   └──────────┘ └─────────┘ └─────────┘
```

### Workspace crates (8 members)

The Cargo workspace under `crates/` layers shared libraries beneath the gateway and worker binaries:

```
                    ┌──────────────┐     ┌──────────────┐
                    │ clawz-gateway│     │  clawz-tauri │
                    │  (HTTP API)  │     │   (desktop)  │
                    └──────┬───────┘     └──────┬───────┘
                           │                      │
                    ┌──────▼──────────────────────▼───────┐
                    │           clawz-worker               │
                    │     (runtime, governance, mesh)      │
                    └──────┬───────────────────────────────┘
                           │
              ┌────────────▼────────────┐
              │     clawz-services      │  DTOs, EventBus, ExecutionClient
              └────────────┬────────────┘
                           │
              ┌────────────▼────────────┐
              │       clawz-core        │  traits, config, DB, PRISM-G
              └────────────┬────────────┘
         ┌─────────────────┼─────────────────┐
         │                 │                 │
  ┌──────▼──────┐   ┌──────▼──────┐   ┌──────▼──────┐
  │clawz-platform│   │clawz-runtime│   │clawz-embedded│
  │  T0–T3 tiers │   │ tokio backends│   │ Embassy T0  │
  └─────────────┘   └─────────────┘   └─────────────┘
```

| Crate | Role |
|-------|------|
| **clawz-core** | Shared types, 12+ async traits, `AppConfig`, `ClawzError`, DB repos, metrics, circuit breaker, PRISM-G types |
| **clawz-platform** | Platform tier (`T0`–`T3`) detection and resource budgets |
| **clawz-runtime** | Pluggable `RuntimeBackend` (Tokio multi-thread and single-thread) |
| **clawz-embedded** | `no_std` Embassy executor for ESP32 / bare-metal agents |
| **clawz-services** | Gateway↔worker DTOs, `ExecutionClient`, `EventBus`, platform store traits |
| **clawz-worker** | Agent pipeline, governance, mesh, memory/RAG, providers, tools, orchestration |
| **clawz-gateway** | REST/WebSocket API, auth, rooms, telephony, MCP, 18 deploy adapters, 30+ SaaS connectors |
| **clawz-tauri** | Tauri 2 desktop/mobile shell with local SQLite and tray UI |

### Platform tiers (T0–T3)

| Tier | Typical hardware | Runtime | Containers |
|------|------------------|---------|------------|
| **T0** | ESP32-S3, bare metal | Embassy (`clawz-embedded`) | No |
| **T1** | Raspberry Pi, SBC Linux | Tokio single-thread | No |
| **T2** | Docker / Podman | Tokio multi-thread + Bollard | Yes |
| **T3** | Server or desktop | Full gateway + worker (or Tauri) | Yes |

### Deployment Modes

| Mode | Topology | Best For | Communication | Database |
|------|----------|----------|--------------|----------|
| **Standalone** | Single binary | Local dev, embedded agents, edge devices | In-process channels | SQLite or local Postgres |
| **Micro** | Multi-container | Small teams, CI/CD pipelines, staging | gRPC / QUIC over Docker network | Shared Postgres + pgvector |
| **Elastic** | Full mesh | Production SaaS, multi-region, high availability | Multi-transport with automatic failover | Sharded Postgres, mDNS and API service discovery |

---

## Quick Start

### Interactive TUI Installer (Recommended)

Run the `clawz` binary with no arguments to launch the interactive TUI installer. The installer walks you through the entire setup conversationally — an AI-powered setup assistant detects your system, recommends a deployment strategy, deploys the stack, and transitions into a live chat with your ClawZ agent.

**Option 1 — Standalone binary (no Rust required):**

```bash
# Linux (amd64)
curl -fsSL https://github.com/improwyz/clawzagent/releases/latest/download/clawz-linux-amd64 -o clawz
chmod +x clawz && ./clawz
```

**Option 2 — Shell bootstrap (installs all prerequisites):**

```bash
curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
```

**Option 3 — From source (if you have Rust + Docker):**

```bash
git clone https://github.com/improwyz/clawzagent.git && cd clawzagent
cargo build -p clawz-cli --release
./target/release/clawz
```

### TUI Installer Flow

The installer has three screens:

1. **Splash** — ClawZ branded logo, system detection summary
2. **LLM Provider Setup** — Select from 12+ providers (Anthropic, OpenAI, OpenRouter, Groq, Grok/xAI, DeepSeek, Ollama, Together, Fireworks, Gemini, Bedrock, or any OpenAI-compatible endpoint). Enter your API key — validated inline.
3. **AI-Guided Chat** — A local setup assistant (powered by your chosen LLM) guides you conversationally through deployment mode selection, secrets generation, Docker deployment, health checks, and agent identity configuration. Once the gateway is healthy, the chat seamlessly transitions to the live ClawZ agent.

For headless/CI environments: `clawz --headless` runs the same flow in plain text mode.

### Operator CLI

The same `clawz` binary also serves as the operator CLI when invoked with subcommands:

| Command | Purpose |
|---------|---------|
| `clawz` | Launch TUI installer / setup assistant |
| `clawz doctor` | Gateway, worker, env, and Docker health checks |
| `clawz gateway start/stop/status` | Docker Compose control |
| `clawz agent --message “...”` | Send a message to an agent |
| `clawz cron list/add/run/remove` | Scheduled agent jobs |
| `clawz setup deps` | Install host prerequisites |
| `clawz setup stack` | Prebuilt pull + Compose up |
| `clawz --headless` | Plain text setup (CI/non-TTY) |

### HTTPS & Dashboard

ClawZ uses **Caddy** as a reverse proxy (included in Docker Compose) for automatic HTTPS:

- **localhost** → self-signed cert (zero config)
- **Public IP or domain** → Let's Encrypt auto-TLS

After install, open **https://your-host** in a browser. The dashboard is also installable as a **PWA** on mobile and desktop.

### Stop / Logs

```bash
docker compose down
docker compose logs -f gateway worker caddy
```

Full install guide: **[INSTALL.md](INSTALL.md)**

---

### Manual setup (developers)

#### 1. Clone the repository

```bash
git clone https://github.com/improwyz/clawz.git
cd clawz
cp .env.example .env
```

#### 2. Docker Compose (recommended)

```bash
docker login
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
curl http://localhost:3000/api/v1/system/health
```

Private registry: [docs/private-registry.md](docs/private-registry.md).

#### 3. Or build from source

```bash
export CLAWZ_MODE=standalone
export CLAWZ_DISABLE_AUTH=1
export CLAWZ_STUB_PROVIDER=1
export CLAWZ_JWT_SECRET=dev-secret
export CLAWZ_WORKER_TOKEN=dev-worker-token

cargo build --release -p clawz-worker -p clawz-gateway
./target/release/clawz-worker &
WORKER_URL=http://127.0.0.1:50051 ./target/release/clawz-gateway
```

#### 4. Create your first agent

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

## New features setup

Recent platform capabilities and how to enable them locally.

### Multi-participant agent rooms

Rooms support **1-many** (one user, agent team) and **many-1 hybrid** (shared team room + private side-threads).

```bash
# Create a room with an agent leader
curl -X POST http://localhost:3000/api/v1/rooms \
  -H "Content-Type: application/json" \
  -d '{
    "room_type": "one_many",
    "participants": [{ "agent_id": "<AGENT_ID>", "role": "leader" }]
  }'

# Send a message (returns 202 — agent turn runs async)
curl -X POST http://localhost:3000/api/v1/rooms/<ROOM_ID>/messages \
  -H "Content-Type: application/json" \
  -d '{ "content": "Hello team" }'

# Stream events
# ws://localhost:3000/ws/rooms/<ROOM_ID>
```

Requires `WORKER_URL` pointing at a running worker (set automatically in Docker Compose).

### Conversations ↔ rooms bridge

Legacy `POST /api/v1/conversations` still works; each conversation gets a **direct room** with the same ID. Messages use the room pipeline (Postgres `seq`, async turns).

```bash
curl -X POST http://localhost:3000/api/v1/conversations \
  -H "Content-Type: application/json" \
  -d '{ "agent_id": "<AGENT_ID>", "title": "Support chat" }'
# Response includes "room_id" (same as conversation id)
```

### Multi-tenant isolation

Set `CLAWZ_TENANT_ID` on the gateway. API keys support an optional fourth field: `sha256_hash:user_id:role:tenant_id`.

```bash
export CLAWZ_TENANT_ID=acme-corp
export VALID_API_KEYS="<hash>:alice:owner:acme-corp,<hash>:bob:agent:acme-corp"
# Disable CLAWZ_DISABLE_AUTH in production
```

Rooms, channels, and JWTs are scoped to the caller's tenant.

### Postgres persistence

With `DATABASE_URL` set (Docker Compose includes pgvector Postgres):

- Rooms, messages (server-assigned `seq`), users, API keys, and channels survive restarts
- Gateway hydrates state on startup
- Login falls back to Postgres when the in-memory user cache is cold

### Worker authentication

Gateway → worker calls require a shared secret:

```bash
export CLAWZ_WORKER_TOKEN=<random-secret>
# Same value on gateway and worker
export WORKER_URL=http://worker:50051   # or http://127.0.0.1:50051 locally
```

### Web dashboard (optional)

```bash
./scripts/install.sh --with-web
cd web && npm run preview   # http://localhost:4173
```

Configure `web/vite.config.ts` proxy to `http://localhost:3000` for API/WS.

### Desktop app (Tauri, optional)

Local agent control without running the HTTP gateway separately:

```bash
cd crates/clawz-tauri
cargo tauri dev    # requires Tauri system dependencies
```

See **[INSTALL.md](INSTALL.md)** for platform-specific Tauri setup and icon regeneration.

### Agent telephony (Twilio & Google Voice)

See **[TELEPHONY.md](TELEPHONY.md)**. Set `CLAWZ_PUBLIC_URL` to your public HTTPS base URL.

```bash
export CLAWZ_PUBLIC_URL=https://your-gateway.example.com
curl -X POST http://localhost:3000/api/v1/agents/<AGENT_ID>/phone \
  -H "Content-Type: application/json" \
  -d '{ "provider": "twilio", "phone_number": "+15551234567" }'
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

- Gateway and worker run as separate binaries (or containers via Docker Compose).
- No full mesh or multi-region orchestration required.
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
# Fast typecheck (all workspace members)
cargo check -p clawz-core -p clawz-platform -p clawz-runtime
cargo check -p clawz-embedded -p clawz-services
cargo check -p clawz-worker -p clawz-gateway -p clawz-tauri

# Run services locally
cargo run -p clawz-gateway
cargo run -p clawz-worker

# Desktop shell (requires Tauri CLI — see INSTALL.md)
cd crates/clawz-tauri && cargo tauri dev
```

### MSRV

ClawZ requires **Rust 1.87+**.

```bash
# Verify MSRV compatibility
cargo +1.87 check --workspace
```

---

## Project Structure

The repository is a **Cargo workspace** with **eight member crates** under `crates/`, plus an optional React dashboard in `web/`:

```
clawz/
├── Cargo.toml                  # Workspace manifest (8 members)
├── .env.example
├── docker-compose.yml
├── scripts/
│   ├── install.sh              # Linux/macOS one-click install
│   ├── install.ps1             # Windows installer
│   ├── setup-host-exec.sh      # host bootstrap for clawz-setup / setup API
│   └── migrate-db.sh           # Postgres migrations
├── web/                        # React dashboard (--with-web)
│   └── public/branding/        # Logo assets (silver / copper)
└── crates/
    ├── clawz-platform/         # T0–T3 tier detection & budgets
    ├── clawz-runtime/          # Tokio RuntimeBackend implementations
    ├── clawz-embedded/         # Embassy no_std backend (ESP32)
    ├── clawz-core/
    │   └── src/
    │       ├── traits.rs       # Async trait interfaces
    │       ├── config.rs       # AppConfig + TOML/env loading
    │       ├── error.rs        # ClawzError
    │       ├── types/          # agent, cost, governance, message, tool, …
    │       ├── db.rs           # Repository layer (sqlx)
    │       ├── prism.rs        # PRISM-G framework types
    │       ├── metrics.rs      # Prometheus primitives
    │       └── circuit_breaker.rs
    ├── clawz-services/
    │   └── src/
    │       ├── dto.rs          # Shared API DTOs
    │       ├── execution.rs    # Gateway → worker HTTP client
    │       ├── events.rs       # In-process EventBus
    │       └── store.rs        # PlatformStore trait
    ├── clawz-worker/
    │   └── src/
    │       ├── runtime/        # Pipeline, team, subagent, fan-out
    │       ├── governance/     # PRISM-G engine, audit chain, trust
    │       ├── mesh/           # Fleet, discovery, firewall
    │       ├── memory/         # RAG, embeddings, pgvector
    │       ├── providers/      # LLM adapters & routing
    │       ├── tools/          # Registry, MCP, Docker, browser
    │       ├── channels/       # Slack, Discord, Telegram, …
    │       ├── orchestration/  # Bollard / standalone schedulers
    │       └── control_api.rs  # Worker control plane (port 50051)
    ├── clawz-gateway/
    │   └── src/
    │       ├── server.rs       # Axum HTTP + TLS
    │       ├── routes/         # REST API (/api/v1/…)
    │       ├── ws/             # Agent & room WebSockets
    │       ├── auth/           # API keys, JWT, tenant RBAC
    │       ├── deploy/         # 18 cloud deploy adapters
    │       ├── connectors/     # 30+ SaaS integrations
    │       ├── telephony/      # Twilio / voice webhooks
    │       ├── mcp/            # MCP server
    │       └── tui/            # Terminal debug UI
    └── clawz-tauri/
        ├── src/                # Tauri commands & tray
        ├── src-ui/             # Frontend assets
        └── design/             # design-system.md
```

See **[AGENTS.md](AGENTS.md)** for detailed subsystem documentation and development conventions.

---

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `CLAWZ_MODE` | **Yes** | Deployment mode: `standalone`, `micro`, or `elastic`. |
| `VALID_API_KEYS` | **Yes** (prod) | Comma-separated API keys: `hash:user_id:role:tenant_id`. Optional when `CLAWZ_DISABLE_AUTH=1` (dev only). |
| `DATABASE_URL` | For `micro`/`elastic` | PostgreSQL connection string (gateway persistence + worker memory). |
| `CLAWZ_CONFIG` | No | Path to TOML configuration file. |
| `CLAWZ_TENANT_ID` | No | Default tenant for dev auth and unscoped records (default `default`). |
| `CLAWZ_WORKER_TOKEN` | Prod recommended | Shared secret for gateway → worker control API. |
| `CLAWZ_PUBLIC_URL` | Telephony/webhooks | Public HTTPS base URL for Twilio and channel webhooks. |
| `CLAWZ_SECRETS_KEY` | Prod recommended | AES-256-GCM key for provider API keys and sensitive tool config at rest. Without it, secrets are stored with a `plain:` prefix (dev only). |
| `CLAWZ_JWT_SECRET` / `JWT_SECRET` | Prod recommended | Signs gateway JWT access tokens. |
| `CLAWZ_LISTEN_ADDR` | No | Gateway bind address (default `0.0.0.0:3000`). |
| `WORKER_URL` | No | When set, gateway delegates execution to remote worker control API (e.g. `http://127.0.0.1:50051`). |
| `CLAWZ_DISABLE_AUTH` | Dev only | Set to `1` to bypass auth (never use in production). |
| `CLAWZ_STUB_PROVIDER` | Dev only | Stub LLM responses without provider API keys. |

### Cloud deploy provider tokens

Set on the gateway host for `POST /api/v1/cloud/deploy` and `DELETE .../deployments/...` (or pass `credentials` in the deploy body).

| Provider | Environment variables |
|----------|----------------------|
| Fly.io | `FLY_API_TOKEN` |
| Railway | `RAILWAY_TOKEN` |
| AWS Lambda | `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION` |
| Azure Functions | `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` (destroy); deploy may use `credentials` in body |
| Google Cloud Run | `GOOGLE_CLOUD_ACCESS_TOKEN`, `GOOGLE_CLOUD_REGION` (optional) |
| Kubernetes | `KUBERNETES_TOKEN`, API server URL configured on adapter |
| Cloudflare Workers | `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID` |
| Fastly Compute | `FASTLY_API_TOKEN` |
| Vercel | `VERCEL_TOKEN` |
| Hetzner | `HETZNER_API_TOKEN` |
| Northflank | `NORTHFLANK_API_TOKEN` |
| Sliplane | `SLIPLANE_API_TOKEN` |
| MassiveGrid (Jelastic) | `MASSIVEGRID_SESSION` |
| OpenTofu | `tofu` CLI on `PATH`; workdir via `CLAWZ_TOFU_WORKDIR` |

LLM provider keys for the worker (`OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.) are separate from cloud deploy tokens.

### Voice WebSocket (`/ws/voice`)

| Variable | Description |
|----------|-------------|
| `CLAWZ_VOICE_PROVIDER` | Set to `openai` to enable Whisper STT + optional TTS (requires `OPENAI_API_KEY`). |
| `CLAWZ_VOICE_AUDIO_NAME` | Filename hint for binary frames (default `audio.webm`). |
| `CLAWZ_VOICE_TTS_MODEL` | OpenAI speech model (default `tts-1`). |
| `CLAWZ_VOICE_TTS_VOICE` | OpenAI voice name (default `alloy`). |

Text control messages: `{"type":"config","agent_id":"…"}`, `{"type":"run_turn","agent_id":"…","message":"…","tts":true}`, `{"type":"transcribe","audio_b64":"…","filename":"audio.webm","auto_turn":true}`.

Set `CLAWZ_VOICE_AUTO_TURN=true` to run an agent turn automatically after each successful binary STT frame. Optional `CLAWZ_VOICE_AUTO_TTS=true` returns MP3 audio for that turn.

### Agent telephony (Twilio & Google Voice)

Bind SMS/voice to an agent with `POST /api/v1/agents/{id}/phone`, then point Twilio or a Google Voice bridge at the returned webhook URLs. Set **`CLAWZ_PUBLIC_URL`** to your gateway’s public HTTPS base URL. See **[TELEPHONY.md](TELEPHONY.md)** for bind fields, webhook paths, and channel config.

### Cursor: Superpowers plugin

Install the [Superpowers](https://github.com/obra/superpowers) plugin in **Agent chat** with `/add-plugin superpowers` (or `/plugin-add superpowers`). See [AGENTS.md](AGENTS.md) for architecture and development conventions.

Oracle Cloud deploy/destroy also accepts `OCI_API_KEY`, `OCI_API_SECRET`, and `OCI_COMPARTMENT_ID`.

### Example

```bash
export CLAWZ_MODE=micro
export VALID_API_KEYS="prod-key-alpha,prod-key-beta"
export DATABASE_URL="postgresql://clawz:secret@db.internal:5432/clawz"
export CLAWZ_CONFIG="/etc/clawz/production.toml"
export CLAWZ_SECRETS_KEY="change-me-in-production"
```

---

## Tech Stack

| Layer | Technology |
|-------|------------|
| **Language** | Rust (2021/2024 editions; MSRV **1.87+** for gateway, worker, services) |
| **Desktop** | [Tauri 2](https://v2.tauri.app) (`clawz-tauri`) |
| **Web UI** | React + Vite (`web/`) |
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

## Branding

Logo assets live in **`web/public/branding/`** and **`crates/clawz-tauri/src-ui/assets/branding/`**. Use **silver** (`clawz-logo-dark.png`, `clawz-mark-dark.png`) on dark backgrounds and **copper** on light.

Colors, typography, and UI tokens: **[crates/clawz-tauri/design/design-system.md](crates/clawz-tauri/design/design-system.md)**.

---

## License & Activation

ClawZ is licensed under the **Elastic License 2.0 (ELv2)** — see [LICENSE](LICENSE).

### Free Trial

ClawZ includes a **30-day free trial** with full functionality. No license key required — just install and start using it. The trial is per-machine and begins on first run.

### License Key

After the trial, purchase a license key at **[clawz.net](https://clawz.net)** to continue using ClawZ. License keys are available for 1-year terms and support up to 2 machines per key.

Activate during setup (the TUI installer prompts for a key) or later via the web dashboard settings page. Keys are hardware-bound and verified against the ClawZ license server.

| Scenario | Behavior |
|----------|----------|
| No key (first 30 days) | Full access — trial mode |
| Trial expired | Gateway stops — enter a license key to continue |
| Paid license expires | 7-day grace period with dashboard warnings, then stops |
| Offline > 30 days | Must reconnect to validate — then resumes normally |

---

## Maintainers

Developed and maintained by **Enterpryz Ventures**.

For questions, support, or enterprise inquiries, please contact the team or open an issue in this repository.

---

<p align="center">
  <em>Built for agents. Engineered for scale. Governed by design.</em>
</p>
