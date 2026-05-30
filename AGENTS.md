# ClawZ Agent Orchestration Platform — Development Reference

**ClawZ** is a Rust-based agent orchestration framework with multi-deployment support (Standalone, Micro, Elastic) for building scalable, compliant, multi-tenant AI agent systems. Built by Enterpryz Ventures. Licensed under Elastic License 2.0 (ELv2).

---

## Architecture Overview

### 3-Tier Design
```
┌─────────────────────────────────────────────────┐
│ Gateway (HTTP API, WebSocket, MCP)             │
│ • Axum-based REST endpoints                    │
│ • Tenant routing & admission control           │
│ • Cloud deployment integrations (18 adapters)  │
│ • TUI, Cloudflare, 31 SaaS connectors         │
└────────────┬────────────────────────────────────┘
             │
┌────────────▼────────────────────────────────────┐
│ Worker (Agent Execution & Orchestration)        │
│ • Runtime engine (pipeline + conditional steps)│
│ • Team load balancing & subagent coordination  │
│ • Governance enforcement (PRISM-G compliance)  │
│ • Mesh networking (leader election, discovery) │
│ • Memory & RAG (pgvector)                       │
│ • Hardware detection & allocation               │
│ • Tools, channels, providers (plugin system)   │
└────────────┬────────────────────────────────────┘
             │
┌────────────▼────────────────────────────────────┐
│ Core (Shared Types & Trait Interfaces)         │
│ • 12 async trait definitions                   │
│ • Configuration structs (7 sub-configs)        │
│ • Domain types (Agent, Cost, Governance, etc.) │
│ • Circuit breaker, error handling              │
│ • Database repositories (8 repos)              │
└─────────────────────────────────────────────────┘
```

### Deployment Modes
The `DeploymentMode` enum (clawz-core::deployment) controls which subsystems activate:

- **Standalone**: Single binary, in-process communication, SQLite or local Postgres, no mesh
- **Micro**: Multiple Docker containers, container orchestration (Bollard), gRPC/QUIC transport, Postgres with pgvector
- **Elastic**: Full mesh with leader election, dynamic discovery, mDNS/API service discovery, TLS everywhere

---

## Repository Structure

### clawz-core/src/ — Shared Types & Interfaces (~8K LOC)

**Core Traits (traits.rs)**
- `RuntimeProvider` — Execute user code, manage lifecycle
- `ToolRegistry` — Lookup and execute tools by name
- `MemoryStore` — Store/retrieve conversation history, embeddings
- `MetricsCollector` — Record counters, histograms, gauges
- `ConfigLoader` — Load and validate config
- `GovernanceEngine` — Evaluate compliance policies
- `AuditLog` — Record governance decisions with hash chain
- `CircuitBreaker` — Fault tolerance for external calls
- `CostTracker` — Calculate and enforce budgets
- `TrustScoring` — Evaluate agent trustworthiness
- `NetworkDiscovery` — Find mesh nodes
- `MessageRouter` — Route messages between agents

**Configuration (config.rs)**
- `AppConfig` — Top-level config struct
  - `ServerConfig` — Gateway bind address, port, TLS
  - `DatabaseConfig` — PostgreSQL connection, pool size, pgvector settings
  - `MeshConfig` — Gossip interval, discovery backend, firewall rules
  - `GovernanceConfig` — PRISM-G thresholds, approval requirements
  - `HardwareConfig` — GPU detection, memory limits
  - `ProviderConfig` — Per-provider rate limits, cost budgets
  - `ObservabilityConfig` — Prometheus metrics endpoint, trace sampling

- Load order: TOML file (CLAWZ_CONFIG env) → env overrides via `CLAWZ__SECTION__KEY` pattern
- Validation on startup; missing required fields fail fast

**Domain Types (types/)**
- `agent.rs` — Agent definition, state machine (Idle→Running→Paused→Complete), permissions
- `cost.rs` — CostRecord, Budget, Provider pricing, daily/monthly enforcement
- `governance.rs` — CompliancePolicy, AuditEntry, PRISM-G dimensions, approval decision
- `message.rs` — AgentMessage, conversation threading, metadata
- `tool.rs` — ToolDefinition, parameter schema, execution result
- `channel.rs` — Communication channel (WebSocket, gRPC, queue), plugin interface
- `deploy.rs` — DeploymentMode, cloud provider config, resource request
- `mesh.rs` — NodeInfo, discovery record, heartbeat, trust score

**Database (db.rs) — 8 Repositories**
- `AgentRepo` — CRUD agents, version history
- `MessageRepo` — Store/query conversation logs, embeddings
- `CostRepo` — Insert costs, query budgets, enforce limits
- `GovernanceRepo` — Insert audit entries, build hash chain
- `MetricsRepo` — Store time-series metrics
- `TrustRepo` — Update trust scores, query by agent
- `ConfigRepo` — Store configuration versions
- `ToolRepo` — Register/lookup tools, parameter schemas

**Error Handling (error.rs)**
- `ClawzError` — Enum covering: Config, Database, Governance, Provider, Circuit, Cost, Mesh, Auth
- Implements `thiserror::Error`, suitable for `?` operator
- No panics in production code

**Metrics (metrics.rs)**
- Prometheus exporter (Prometheus client library)
- Counters: agents_created, messages_processed, governance_rejections
- Histograms: agent_execution_ms, cost_per_request, trust_score
- Gauges: active_agents, mesh_nodes, api_requests_in_flight
- Export on `/metrics` endpoint

**Circuit Breaker (circuit_breaker.rs)**
- Lock-free atomic state machine (Open → Half-Open → Closed)
- Failure threshold (e.g., 5 failures), recovery timeout (e.g., 30s)
- Protects all provider calls
- Returns `ClawzError::CircuitOpen` when tripped

---

### clawz-worker/src/ — Agent Execution & Orchestration (~30K LOC)

**Runtime (runtime/)**
- `agent.rs` — `AgentRuntime` struct: execute single agent, manage state, track costs
  - Methods: spawn, step, pause, resume, terminate
  - Cost accumulation per request
  - Trap governance checks via trait injection
- `pipeline.rs` — `PipelineExecutor`: ordered steps with rollback
  - Step 1: `ContextStep` — Gather inputs, load conversation history
  - Step 2: `GovernanceStep` — Check compliance (PRISM-G)
  - Step 3: `ProviderStep` — Route to LLM/tool provider
  - Step 4: `ToolsStep` — Execute returned tools
  - Step 5: `PersistStep` — Save messages, costs, audit log
  - Each step: `execute()` → result, `rollback()` → undo side effects
  - Conditional branching based on step output
- `team.rs` — `TeamCoordinator`: manage multiple agents
  - Load balancing: round-robin, least-busy, cost-aware
  - Budget sub-leasing to team members
  - Consensus on approval decisions
- `subagent.rs` — Spawn sub-agents within agent context, inherit parent's permissions/budget
- `fan_out.rs` — Parallel execution with fan-out/fan-in pattern, error aggregation

**Governance (governance/)**
- `engine.rs` — `GovernanceEngine`: PRISM-G policy evaluation
  - Input: AgentAction (execute_tool, send_message, access_data)
  - Output: Approve, Reject with reason, RequireApproval with approvers list
  - Caches policies in memory; revalidates on policy change
- `guardrails.rs` — Governance-dimension runtime guardrails (Vol 9)
  - **P**rivacy: no access to PII without consent, data minimization
  - **R**eliability: error budgets, SLA enforcement
  - **I**ntegrity: audit chain validation, no tamper
  - **S**afety: rate limits, memory bounds, output constraints
  - **M**onitoring: mandatory metric export, trace sampling
  - **G**overnance: approval thresholds, role-based permissions
  - Each dimension: threshold (0.0–1.0), pass/fail decision
- `policy.rs` — Policy definition, version control, hot reload
- `trust.rs` — Trust scoring (5-tier: Untrusted, Low, Medium, High, Full)
  - Inputs: action history, approval rate, anomaly score
  - Output: affects scheduling priority, approval thresholds
- `approval.rs` — Approval workflow: request → route to approvers → collect votes → decide
- `council.rs` — Multi-approver council, quorum settings
- `audit.rs` — SHA-256 hash chain for tamper detection
  - Each entry: hash(prev_hash + action + timestamp + approver)
  - Query chain integrity via chain_valid()
- `compliance.rs` — Compliance report generation (SOC2, GDPR, EU-AI-Act)

**Mesh (mesh/)**
- `fleet.rs` — Leader election (quorum-based consensus), fleet membership
- `heartbeat.rs` — EWMA round-trip-time (RTT) calculation, node health scoring
- `router.rs` — Message routing between nodes, load shedding
- `discovery.rs` — Service discovery (static, mDNS, API backends)
- `firewall.rs` — Network policies, allowed node/port pairs, rate limiting

**Transport (transport/)**
- Pluggable transports: gRPC, QUIC, WebSocket (WSS), in-process IPC
- `selector.rs` — Choose transport based on DeploymentMode and latency
- Automatic failover between transports

**Memory & RAG (memory/)**
- `store.rs` — Conversation storage, full-text search
- `rag.rs` — Retrieve-Augment-Generate: similarity search in pgvector
- `embedding.rs` — Embed text via provider (OpenAI, Ollama, or local)
- `conversation.rs` — Thread management, message context window
- `blackboard.rs` — Shared state dictionary across agents in team

**Providers (providers/)**
- `registry.rs` — `ProviderRegistry`: lookup provider by name (gpt-4, claude-3-opus, etc.)
- `router.rs` — Route requests with circuit breaker, cost tracking, retry logic
- `cost_budget.rs` — Enforce per-request, daily, monthly cost limits
- `adapters/` — Implementation for each provider
  - OpenAI (gpt-4, gpt-4-turbo), Anthropic (Claude 3 family), local (Ollama, Llama.cpp)
  - Each adapter: Prompt → ProviderResponse (text, tokens, cost)

**Tools (tools/)**
- `registry.rs` — Register tool, parameter schema validation
- `browser.rs` — CDP (Chrome DevTools Protocol) for web automation
- `docker.rs` — Container lifecycle, image pull, run, exec
- `mcp.rs` — Model Context Protocol: call external servers
- `builtin/` — Standard tools: file I/O, HTTP, bash execution
  - Sandboxed execution via seccomp/apparmor

**Channels (channels/)**
- Plugin system: traits `ChannelSender`, `ChannelReceiver`
- `native/` — Built-in channels (WebSocket, gRPC, stdout)
- User-defined channels: Slack, Discord, Teams, Telegram via adapters

**Hardware (hardware/)**
- `detect.rs` — GPU detection (NVIDIA, AMD, Intel Arc, Apple Silicon)
  - Query CUDA, ROCm, Metal, Intel GPU APIs
  - Return: GPU model, VRAM, compute capability
- `loader.rs` — Load model to GPU, allocation strategy (largest-first, least-busy)
- `uf2.rs` — Flash firmware to microcontrollers (UF2 format)

**Orchestration (orchestration/)**
- `bollard_scheduler.rs` — Docker container scheduling via Bollard API
- `standalone.rs` — In-process execution (no containers)
- `factory.rs` — Select orchestration backend by DeploymentMode
- `lifecycle.rs` — Container startup, probe health checks, graceful shutdown

**Observability (observability/)**
- `context_propagation.rs` — Inject trace context (OpenTelemetry W3C) into container env
  - Propagate request IDs, parent span ID to child agents

---

### clawz-gateway/src/ — HTTP API & Deployment (~17K LOC)

**Server (server.rs)**
- Axum web framework, TLS support (rustls or system-native)
- Request logging middleware, CORS
- `/health` → JSON health status
- `/metrics` → Prometheus scrape endpoint

**Shutdown (shutdown.rs)**
- `ShutdownCoordinator`: cascade CancellationToken to all subsystems
- Graceful shutdown: stop accepting requests → drain in-flight → close DB → exit
- Timeout configurable (default 30s)

**Authentication (auth/)**
- API key validation from `VALID_API_KEYS` environment variable
- Comma-separated keys: "key1,key2,key3"
- `Authorization: Bearer <key>` header check on all protected routes
- Return 401 if missing/invalid

**Routes (routes/)**
- `POST /agents` — Create agent (Agent struct in JSON body)
- `GET /agents/{id}` — Fetch agent state
- `PATCH /agents/{id}` — Update permissions/budget
- `POST /agents/{id}/execute` — Execute agent, return results
- `POST /messages` — Send message to agent
- `GET /messages` — Query conversation history with pagination
- `POST /tools` — Register tool
- `GET /tools` — List registered tools
- `POST /governance/policies` — Define policy
- `GET /governance/audit` — Audit log with hash chain validation

**WebSocket (ws/)**
- `GET /ws/{agent_id}` — Bidirectional agent interaction
- Server pushes agent events (running, paused, complete)
- Client sends commands (pause, resume, send_message)

**MCP (mcp/)**
- Model Context Protocol server
- Expose agents, tools, governance as MCP resources
- External clients (Claude, other AI tools) integrate via MCP

**Deployment (deploy/)**
- 15 cloud adapters: AWS Lambda, Azure Functions, Google Cloud Run, Cloudflare Workers, Vercel, Fly.io, Railway, Kubernetes, Hetzner, Fastly, Northflank, MassiveGrid, Oracle Cloud, Sliplane, OpenTofu
- Each adapter: upload binary/container → create resource → return public URL + API endpoint

**Scheduling (scheduling/)**
- `AdmissionController` — Validate incoming agent requests
  - Check API key, tenant quota, resource limits
  - Return 429 if over quota, 400 if invalid
- `TenantRouter` — Route requests to correct worker instance in mesh
  - Sticky sessions (same tenant → same worker)
  - Load balance across healthy workers

**TUI (tui/)**
- Terminal user interface for local development/debugging
- List agents, view logs, trigger actions, monitor metrics

**Cloudflare (cloudflare/)**
- Workers integration: deploy agent endpoints to Cloudflare edge
- KV store for rate limiting, D1 database for metadata
- Signed requests to backend

**Connectors (connectors/)** — 31 SaaS integrations
- Slack, Discord, Teams, Telegram, Twilio
- GitHub, GitLab, Gitea (code integration)
- Jira, Linear, Asana (issue tracking)
- Stripe, Shopify (e-commerce)
- Notion, Airtable, Google Sheets (data)
- HubSpot, Salesforce, Pipedrive (CRM)
- OpenAI, Anthropic APIs (direct provider access)
- Hugging Face (model hub)

---

## Installation

For local development, use the one-click installer (Docker Compose **micro/fleet** by default):

```bash
docker login ghcr.io              # required for private prebuilt images on GHCR
export GITHUB_TOKEN=ghp_xxx GITHUB_USER=you
git clone https://github.com/improwyz/clawz.git ~/clawz && cd ~/clawz
./scripts/install.sh              # prebuilt pull → db → migrate → worker + gateway
./scripts/install.sh --build      # local image build (docker-compose.build.yml)
./scripts/install.sh --bootstrap-only  # host deps only (no stack)
./scripts/install.sh --wizard       # install + clawz onboard --install-daemon
./scripts/install.sh --with-web     # include React dashboard
./scripts/install.sh --source       # cargo build without Docker
```

**CLI stack commands** (`cargo build -p clawz-cli`):

```bash
clawz setup deps                  # install-deps.sh on host
clawz setup stack                 # same Compose sequence as install.sh
clawz onboard --install-daemon    # wizard then setup stack
```

**Windows:** `.\scripts\install.ps1 -Docker -Prebuilt -Wizard` (prebuilt pull, migrate-db, optional winget Docker via `-InstallDocker`).

Compose overlays: `docker-compose.prebuilt.yml` (registry pull), `docker-compose.build.yml` (local build). Host bootstrap scripts: `scripts/setup-host-exec.sh` (used by **`clawz-setup`** `HostScriptRunner` and gateway `POST /api/v1/setup/stack`).  
Private registry: **[docs/private-registry.md](docs/private-registry.md)**. Full guide: **[INSTALL.md](INSTALL.md)**.

**Onboarding wizard:** **`clawz-setup`** powers `clawz onboard` (Linux TUI), web **`/setup`**, and gateway setup API — deploy mode, dependencies, stack, agent identity/skills, LLM OAuth, and `doctor` verification. Design: [docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md](docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md); Docker bootstrap: [docs/superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md](docs/superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md); tasks: [docs/install-onboarding-wizard-tasks.md](docs/install-onboarding-wizard-tasks.md).

**Branding:** Logo assets in **`web/public/branding/`** — silver on dark, copper on light. UI tokens: **[crates/clawz-tauri/design/design-system.md](crates/clawz-tauri/design/design-system.md)**.

---

## Build Commands

```bash
# Workspace
cargo build --workspace              # Debug build all crates
cargo build --workspace --release    # Optimized build
cargo test --workspace               # Run all tests
cargo test --workspace -- --nocapture  # With println output
cargo clippy --workspace -- -D warnings  # Lint (deny all warnings)
cargo fmt --all -- --check           # Check formatting

# Individual crates
cargo check -p clawz-core            # Fast typecheck
cargo check -p clawz-worker
cargo check -p clawz-gateway
cargo build -p clawz-gateway --release  # Ship binary
cargo test -p clawz-core --lib       # Unit tests only

# Development
cargo run -p clawz-gateway           # Run gateway (localhost:3000)
cargo run -p clawz-worker            # Run worker agent
cargo watch -x 'test --lib'          # Auto-test on save (requires `cargo-watch`)
```

---

## Key Design Patterns

### Trait-Driven Architecture
All subsystem interfaces are async traits in `clawz-core::traits`. Implementations live in `clawz-worker`. This enables:
- Easy mocking for tests
- Hot-swapping providers (OpenAI ↔ Claude ↔ local LLM)
- Plugin system for new tools, channels, providers

### Pipeline Pattern
Agents execute through ordered, reversible steps:
1. **Context** — Load history, embeddings, parent context
2. **Governance** — Check PRISM-G, get approvals if needed
3. **Provider** — Call LLM (with retry, circuit breaker)
4. **Tools** — Execute returned function calls (parallel batch)
5. **Persist** — Save messages, audit, metrics, costs

Each step implements `PipelineStep` trait:
```rust
#[async_trait]
pub trait PipelineStep {
    async fn execute(&self, ctx: &mut StepContext) -> Result<()>;
    async fn rollback(&self, ctx: &mut StepContext) -> Result<()>;
    fn priority(&self) -> u32;  // Lower = earlier
}
```

Rollback is automatic if later step fails, ensuring consistency.

### Circuit Breaker Pattern
All external calls (providers, external tools) go through `CircuitBreaker`:
- Tracks failure rate
- Opens (rejects calls) if threshold exceeded
- Half-opens after timeout to test recovery
- Closes on success
- Returns `ClawzError::CircuitOpen` instead of cascading failures

Lock-free implementation using `AtomicU64` for state + timestamp.

### Cost Tracking
Every agent action has associated cost:
- Provider calls: token count × model price
- Tool calls: fixed cost per tool type
- Network egress: bytes transferred
- GPU compute: minutes × instance type price

Tracked per-request via `CostRecord` → `CostRepo`. Daily/monthly budgets enforced:
```rust
pub struct Budget {
    tenant_id: String,
    daily_limit: f64,
    monthly_limit: f64,
    current_day_spend: f64,
    current_month_spend: f64,
}
```

If spend exceeds limit, `PersistStep` rejects agent action and returns error.

### PRISM-G Compliance
PRISM-G defines six canonical dimensions. The governance checks in this document implement the **G (Governance) dimension's** runtime guardrails — safety, compliance, and oversight on every agent action:

| Dimension | Focus |
|---|---|
| **P**urpose | Goal decomposition, intent alignment |
| **R**eality | Environment discovery, factual grounding |
| **I**nfrastructure | Tool/container scheduling, resource bounds |
| **S**warm | Multi-agent coordination, team protocols |
| **M**emory & Metrics | Conversation history, observability |
| **G**overnance | Guardrails, trust scoring, approval workflows, audit chain |

Encoded as policy YAML/JSON, versioned, hot-reloadable.

### Trust Scoring (5-Tier)
Agents scored by history:
1. **Untrusted** — New agent, no approvals automated
2. **Low** — Few successful actions, require senior approval
3. **Medium** — Regular usage, some autonomous actions allowed
4. **High** — Long track record, require only spot-check approvals
5. **Full** — Trusted, autonomous execution with monitoring

Approval thresholds and scheduling priority adjust per tier.

### Audit & Tamper Detection
Hash chain (like blockchain):
```
Entry 0: hash("genesis" + timestamp + approver) = H0
Entry 1: hash(H0 + action + timestamp + approver) = H1
Entry 2: hash(H1 + action + timestamp + approver) = H2
```

Verify chain: recompute H0, H1, H2 from audit entries. If any hash mismatch, tamper detected.

### Multi-Tenant Isolation
- **RBAC**: Each API key tied to tenant ID; gateway checks tenant on every request
- **Budget sub-leasing**: Parent tenant can delegate budget to sub-tenants
- **Network isolation**: Mesh firewall rules prevent cross-tenant traffic
- **Database**: Tenant ID in WHERE clause on all queries; no cross-tenant visibility

---

## Configuration & Environment

### Config File (TOML)
Path: `CLAWZ_CONFIG` environment variable. Example:
```toml
[server]
bind = "0.0.0.0"
port = 3000
tls_cert = "/etc/certs/server.crt"
tls_key = "/etc/certs/server.key"

[database]
url = "postgresql://user:pass@localhost/clawz"
pool_size = 20
pgvector_dimensions = 1536

[mesh]
gossip_interval_ms = 1000
discovery = "mdns"  # or "static", "api"

[governance]
compliance_level = "strict"  # or "moderate", "permissive"

[providers]
[providers.openai]
api_key = "sk-..."
rate_limit_rpm = 60
[providers.anthropic]
api_key = "sk-ant-..."
```

### Environment Overrides
Pattern: `CLAWZ__SECTION__KEY` (double-underscore, uppercase)
```bash
export CLAWZ__SERVER__PORT=8080
export CLAWZ__DATABASE__URL="postgresql://prod-db"
export CLAWZ__PROVIDERS__OPENAI__RATE_LIMIT_RPM=120
```

### Required Environment Variables
```bash
CLAWZ_MODE=standalone|micro|elastic  # Deployment mode (required)
VALID_API_KEYS="key1,key2,key3"      # API keys (required, comma-separated)
DATABASE_URL="postgresql://..."      # DB connection (required for micro/elastic)
CLAWZ_CONFIG="/etc/clawz/config.toml" # Config file path (optional)
```

---

## Workflow Rules

### Before Submitting PRs
1. Run tests: `cargo test --workspace`
2. Lint: `cargo clippy --workspace -- -D warnings` (must pass)
3. Format: `cargo fmt --all` (auto-format code)
4. One concern per PR (feat, fix, refactor, docs, test, chore)
5. Small PRs preferred (ideally <300 lines)
6. Never skip pre-commit hooks
7. Never push directly to main (use feature branches)

### Commit Message Style (Conventional Commits)
```
feat(runtime): add subagent fan-out execution

Implement parallel execution for multiple subagents with
fan-in result aggregation. Adds PipelineStep trait for
conditional branching.

Closes #42
```

Prefix: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`
Scope: subsystem name (runtime, governance, mesh, etc.)
Body: why the change, not what changed
Footer: issue references, breaking changes

---

## Code Style

### Rust Standards
- **Edition**: Rust 2024
- **MSRV**: 1.87+ (Minimum Supported Rust Version)
- **No unwrap/expect**: Only in tests and infallible operations
- **Error handling**: Return `ClawzError` via `?` operator
- **Async-first**: tokio runtime, `#[tokio::test]` for tests
- **Trait-driven**: Prefer trait objects over concrete types where extensible

### Naming Conventions
- Structs: PascalCase (`AgentRuntime`, `GovernanceEngine`)
- Functions/methods: snake_case (`execute_agent`, `check_compliance`)
- Constants: SCREAMING_SNAKE_CASE (`DEFAULT_TIMEOUT_MS`)
- Type parameters: Single letters (T, U, E) or descriptive (Provider, State)

### Documentation
- Public items: doc comments with examples
- Private items: brief comments explaining why
- Complex algorithms: doc comment with references or pseudocode

```rust
/// Execute agent with pipeline steps and rollback on failure.
///
/// # Arguments
/// * `agent_id` - Unique agent identifier
/// * `steps` - Ordered list of pipeline steps
///
/// # Returns
/// Agent result or error
///
/// # Errors
/// Returns `ClawzError::Governance` if policy rejects action.
pub async fn execute(agent_id: &str, steps: Vec<Box<dyn PipelineStep>>) -> Result<AgentResult> {
    // ...
}
```

### Module Organization
```
src/
├── lib.rs                 # Public exports
├── error.rs               # Error types
├── types.rs               # Domain types
├── traits.rs              # Trait definitions
├── circuit_breaker.rs     # Shared utilities
└── subsystem/             # Sub-modules
    ├── mod.rs             # Public API
    ├── implementation.rs   # Core logic
    └── tests.rs           # Unit tests
```

---

## Testing

### Unit Tests
Colocate with code using `#[cfg(test)]` modules:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_execution() {
        let agent = AgentRuntime::new(agent_config()).await.unwrap();
        let result = agent.execute().await;
        assert!(result.is_ok());
    }
}
```

### Integration Tests
In `crates/clawz-gateway/tests/`:
```rust
#[tokio::test]
async fn test_api_create_agent() {
    let client = test::setup_server().await;
    let response = client
        .post("/agents")
        .json(&CreateAgentRequest { /* ... */ })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
}
```

### Mock at Trait Boundaries
Mock traits, not concrete implementations:
```rust
struct MockProvider {
    // ...
}

#[async_trait]
impl RuntimeProvider for MockProvider {
    async fn execute(&self, code: &str) -> Result<String> {
        Ok("mocked result".into())
    }
}
```

### Test Coverage
- Target: 70%+ on critical paths (governance, cost, audit)
- Use `cargo tarpaulin` or `cargo llvm-cov` to measure
- Mark non-critical code as `#[cfg_attr(coverage, allow(unused))]`

---

## Anti-Patterns to Avoid

### Configuration
- **Don't**: Hardcode values; use env variables and TOML
- **Don't**: Add speculative config fields "just in case"
- **Don't**: Reload config without validation

### Error Handling
- **Don't**: Use `unwrap()` or `expect()` in production code
- **Don't**: Silently ignore errors with `.ok()`
- **Don't**: Propagate generic "Internal Error"

### Database
- **Don't**: Execute raw SQL outside repo structs
- **Don't**: Bypass RBAC checks in queries
- **Don't**: Duplicate state across structs

### Code Changes
- **Don't**: Mix formatting and logic in same PR
- **Don't**: Add dependencies without justification (check MSRV, security, maintainability)
- **Don't**: Bypass governance checks for "admin accounts"

### Governance
- **Don't**: Make PRISM-G checks optional
- **Don't**: Skip approval without documented exception
- **Don't**: Audit log without hash chain validation

---

## Security

### API Keys
- Load from `VALID_API_KEYS` environment variable (never hardcoded)
- Hash in database if storing custom keys
- Rotate regularly (recommend monthly)
- Log key usage for suspicious patterns

### Tenant Isolation
- Mesh: Firewall rules enforced at network layer
- Database: Tenant ID in WHERE clause on every query
- Cost: Budgets isolated per tenant
- Approval: Cannot approve own actions

### Data Protection
- Encryption at rest: database encrypted via OS (dm-crypt, EBS encryption)
- Encryption in transit: TLS 1.3+ for all network communication
- PRISM-G Privacy checks: no PII without consent
- GDPR compliance: right to be forgotten (delete_tenant_data)

### Audit & Compliance
- SHA-256 audit chain: detect tampering
- Immutable audit log: append-only database table
- SOC2/GDPR/EU-AI-Act compliance report generation
- Export audit data for external audits

### Container Security
- Run non-root user in all containers
- No dangerous capabilities (CAP_SYS_ADMIN, etc.)
- Read-only root filesystem where possible
- Network policies: ingress/egress rules via Kubernetes NetworkPolicy or firewall

### Supply Chain
- All dependencies: check for vulnerabilities (`cargo audit`)
- License compliance: Elastic License 2.0 (ELv2) → vet external crate licenses
- Signed commits recommended (git commit -S)

---

## Performance Considerations

### Scaling
- **Standalone**: 10–100 concurrent agents per binary
- **Micro**: Horizontal scale via container replicas; sticky sessions for session affinity
- **Elastic**: Mesh + auto-scaling; leader election ensures consistency

### Optimization Tips
- Cache governance policies (reload on policy version change)
- Batch tool calls in PipelineStep
- Use connection pooling (default 20 connections)
- Enable compression on large responses (gzip)
- Monitor circuit breaker trips; tune thresholds
- pgvector HNSW index on embedding columns for fast similarity search

### Bottlenecks
- Database: Queries can block pipeline; use indexes
- Provider calls: Add retry + exponential backoff in circuit breaker
- Mesh gossip: Reduce gossip_interval_ms if high latency
- Memory: Large conversation history; implement retention policy

---

## Useful Resources

- **Axum**: Web framework docs (https://docs.rs/axum/)
- **tokio**: Async runtime (https://tokio.rs)
- **thiserror**: Error handling (https://docs.rs/thiserror/)
- **serde**: Serialization (https://serde.rs)
- **sqlx**: Database (https://github.com/launchbadge/sqlx)
- **pgvector-rs**: Vector store (https://github.com/pgvector/pgvector-rust)
- **OpenTelemetry**: Observability (https://opentelemetry.io)

---

## Quick Reference

| Task | Command |
|------|---------|
| Build | `cargo build --workspace --release` |
| Test | `cargo test --workspace` |
| Lint | `cargo clippy --workspace -- -D warnings` |
| Format | `cargo fmt --all` |
| Run gateway | `cargo run -p clawz-gateway` |
| Add dependency | `cargo add -p clawz-core <crate>` |
| Check MSRV | `cargo +1.87 check --workspace` |
| Audit deps | `cargo audit` |

---

**Last Updated:** 2026-05-24  
**Maintainers:** Enterpryz Ventures  
**License:** Elastic License 2.0 (ELv2)  
**Codebase Size:** ~65K lines of Rust
