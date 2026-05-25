//! `clawz-worker` — Agent execution layer for the ClawZ platform.
//!
//! This crate is the **middle tier** in the 3-tier architecture:
//! `gateway` (API layer) → `worker` (execution layer) → `core` (shared types / traits).
//!
//! Workers execute agent requests via the [`runtime`] module, which orchestrates
//! pipelines, teams, sub-agents, and provider fan-out. All other modules in this
//! crate provide supporting infrastructure: LLM provider adapters, tool registries,
//! memory backends, transport, mesh networking, governance enforcement, hardware
//! detection, and communication channels.
//!
//! # Module overview
//!
//! | Module | Responsibility |
//! |--------|----------------|
//! | [`channels`] | External comms: Slack, Teams, Discord, WhatsApp, Zoom, etc. |
//! | [`governance`] | Policy engine, audit, trust scoring, approval workflows |
//! | [`hardware`] | CPU/GPU detection, model loading, peripherals, firmware flashing |
//! | [`memory`] | Vector stores, embeddings, RAG, conversation history |
//! | [`mesh`] | Elastic-mode peer-to-peer mesh (discovery, heartbeats, routing) |
//! | [`orchestration`] | Container / in-process scheduler and lifecycle manager |
//! | [`observability`] | Distributed tracing context propagation |
//! | [`providers`] | LLM adapter routing, cost tracking, registry |
//! | [`runtime`] | Agent execution engine, pipelines, teams, sub-agents |
//! | [`tools`] | Built-in and plugin tools, Docker tools, MCP clients |
//! | [`transport`] | gRPC, QUIC, WSS, and in-process transport with auto-selection |
//!
//! # Key external dependencies
//!
//! - `clawz-core` — shared traits (`PipelineStep`, `MemoryBackend`, `Transport`)
//!   and types (`AgentConfig`, `Message`, `AgentState`).
//! - `tokio` — async runtime.
//! - `reqwest` / `http` — HTTP client for provider adapters and tools.
//! - `bollard` — Docker API client for containerised tool / agent execution.
//! - `sqlx` — Postgres access for persistent memory backends.
//! - `quinn` / `tokio-tungstenite` / `rustls` — QUIC and WSS transport implementations.

// ── External communication ──────────────────────────────────────────────────
// Channels connect agents to the outside world (Slack, Teams, WhatsApp, Zoom,
// etc.) and support dynamic plugin loading for additional backends.

/// External communication channel integrations and plugin loader.
///
/// Native channels: Slack, Teams, Discord, WhatsApp, Zoom, Webex, RingCentral,
/// Dialpad, 3CX, and generic webhooks. The plugin system allows dynamic loading
/// of additional channel backends at runtime.
pub mod channels;

// ── Security & governance ───────────────────────────────────────────────────
// These modules enforce security boundaries: policy evaluation, audit trails,
// trust scoring, and multi-party approval for sensitive operations.

/// Policy enforcement, audit logging, trust scoring, approval workflows, and
/// compliance export.
///
/// The `ClawzGovernanceEngine` evaluates every action against configured
/// policies. The `Council` coordinates multi-party approval for sensitive
/// operations. `TrustScorer` maintains per-entity reputation scores.
// Dependency: clawz-core::traits::GovernanceEngine — governance contract defined in core.
pub mod governance;

// ── Hardware & local inference ──────────────────────────────────────────────
// Hardware detection and local model loading support on-premise / edge
// deployments where agents run close to the metal.

/// Hardware detection, local model loading, peripherals, and firmware flashing.
///
/// Detects CPU, GPU, and memory capabilities. Loads GGUF models with quantization
/// awareness. Manages USB / serial peripherals. Supports UF2 firmware flashing
/// for embedded devices.
pub mod hardware;

// ── Memory & state ────────────────────────────────────────────────────────────
// Memory modules let agents remember across turns via vector stores,
// conversation history, and shared blackboards.

/// Persistent and ephemeral memory: vector stores, embeddings, RAG, conversation
/// history, and multi-agent shared blackboard.
///
/// Backends include `PostgresMemoryBackend` (pgvector) and `InMemoryBackend`.
/// Embedding providers cover OpenAI and Ollama. The RAG pipeline handles ingest,
/// retrieve, and rerank.
// Dependency: clawz-core::traits::MemoryBackend — storage contract defined in core.
pub mod memory;

// ── Elastic networking ──────────────────────────────────────────────────────
// Mesh and transport provide distributed communication for elastic deployments.
// In standalone mode `InProcessTransport` is used instead.

/// NetBird-inspired peer-to-peer mesh networking for elastic deployments.
///
/// Maintains simultaneous multi-path connections per peer, runs concurrent
/// heartbeats, and performs traffic-type-aware routing. Disabled in standalone
/// mode where `InProcessTransport` is used instead.
// Dependency: crate::transport — actual packet delivery via selected transport.
// Dependency: clawz-core::types::mesh — shared mesh primitives (PeerInfo, TrafficType).
pub mod mesh;

// ── Execution & orchestration ───────────────────────────────────────────────
// These modules form the primary data path: a request enters through runtime,
// which delegates to providers for inference, tools for side-effects, and
// memory for state. orchestration handles container lifecycle.

/// Scheduling and lifecycle management for agent containers and in-process tasks.
///
/// Selects between Docker-backed (`BollardScheduler`) and standalone
/// (`StandaloneScheduler`) execution based on `DeploymentMode`.
// Dependency: crate::runtime — workers execute via the runtime module.
pub mod orchestration;

/// Distributed tracing and observability support.
///
/// Propagates trace context into containers and tool processes so that
/// downstream spans appear in the same trace as the parent agent request.
pub mod observability;

// ── Intelligence infrastructure ───────────────────────────────────────────────
// Providers, tools, and memory are the three pillars that let an agent reason
// (providers), act (tools), and remember (memory).

/// LLM provider adapters, routing, cost tracking, and registry.
///
/// Supports OpenAI, Anthropic, Gemini, Azure, Bedrock, Ollama, and DeepSeek.
/// The `ProviderRouter` implements least-cost / least-latency routing with
/// fallback and circuit-breaker logic.
// Dependency: clawz-core::traits::Provider — adapter contract defined in core.
// Dependency: clawz-core::types::ProviderConfig — shared configuration types.
pub mod providers;

/// Reality dimension — model the operational environment, detect drift,
/// and version reality snapshots over time.
///
/// Produces [`ContextBundle`](clawz_core::types::ContextBundle) snapshots from
/// static, dynamic, and human discovery sources. Drift detection compares
/// predicted vs observed bundles. Versioned storage is tenant-isolated.
pub mod reality;

/// Agent execution engine: `AgentRuntime`, `Pipeline`, `Team`, `SubAgent`,
/// `FanOut`, and `Workflow`.
///
/// Every request path funnels through a `Pipeline` composed of `PipelineStep`
/// implementations, making the system testable and providing automatic rollback.
///
/// # Cross-module relationships
/// - Builds on `providers` for LLM inference.
/// - Builds on `tools` for tool execution steps.
/// - Builds on `memory` for context retrieval and storage steps.
/// - Builds on `governance` for policy enforcement steps.
/// - Builds on `mesh` and `transport` when operating in elastic mode.
/// - Builds on `channels` for inbound / outbound message steps.
// Dependency: crate::providers — LLM inference routing.
// Dependency: crate::tools — tool execution side-effects.
// Dependency: crate::memory — persistent agent memory.
// Dependency: crate::governance — policy enforcement and audit.
// Dependency: crate::mesh — elastic-mode peer delegation.
// Dependency: crate::transport — message delivery to remote peers.
// Dependency: crate::channels — external communication integrations.
pub mod runtime;

/// Tool registry and implementations: built-in, Docker, browser, and MCP.
///
/// Built-in tools include shell execution, file operations, web search / fetch,
/// calculator, image generation, knowledge-base lookup, PDF reading, and escalation.
/// Docker tools run in isolated containers. MCP clients connect to external
/// MCP servers for extensible tool surfaces.
// Dependency: clawz-core::traits::Tool — tool contract defined in core.
pub mod tools;

// ── Transport layer ───────────────────────────────────────────────────────────
// Transport implementations move bytes between workers. The selector chooses
// the best transport based on network conditions.

/// Transport layer with four concrete implementations and an intelligent selector.
///
/// | Transport | Use case |
/// |-----------|----------|
/// | gRPC | LAN peers |
/// | QUIC | Remote peers |
/// | WSS | Firewall-constrained networks |
/// | In-process | Single-binary standalone mode |
///
/// The selector automatically picks the best path based on health probes.
pub mod transport;
