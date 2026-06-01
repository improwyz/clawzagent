# ADR 0002 — Phase 4 dedup: type layering & god-module splits

- **Status:** Accepted
- **Date:** 2026-05-31
- **Phase:** 4 (structural dedup) of the audit & refactor program

## Context

The audit flagged several "duplicated" items in Phase 4:

1. `CircuitBreaker` — canonical lock-free copy in `clawz-core` + a private mutex
   copy in the worker provider router.
2. retry/backoff — `retry_with_backoff` (channels) vs `execute_with_retry` (router).
3. `ProviderConfig` / `MeshConfig` / `AuditEntry` — "defined twice with divergent
   fields" across core / worker / gateway.
4. God modules — `clawz-core/src/db.rs` (~2071 LOC),
   `clawz-gateway/src/routes/agents.rs` (~1091), and
   `clawz-worker/src/governance/guardrails.rs` (~1079).

## Decisions

### 1 & 2 — Genuine duplication: consolidated

- The router's private `CircuitBreaker`/`CircuitState` is **deleted**; the router
  now uses `clawz_core::CircuitBreaker` via a `breaker_for()` helper. Behavior
  note: a single failed probe in HalfOpen now re-opens the circuit (standard
  semantics) vs. the old breaker's "reset counter, allow `threshold` fresh
  failures." Pinned by `clawz_core::circuit_breaker` tests; single-commit revert.
- `retry_with_backoff` is **hoisted** into `clawz_core::retry` and re-exported
  from `clawz-worker` channels::plugin (path-compatible). The router's
  `execute_with_retry` is **kept** — it is a distinct policy (jittered backoff,
  circuit-breaker integration, no-retry-on-auth), not a copy of the generic
  helper, and collapsing it would silently change behavior.

### 3 — Not duplication: distinct layers, documented (not merged)

On inspection these are **different types that happen to share a name**, serving
different layers. Merging them would be artificial and, for `AuditEntry`,
actively unsafe:

| Type | core / gateway | worker |
|---|---|---|
| `ProviderConfig` | static/declarative: `api_key_env` (env-var *name*), `default_model` | resolved HTTP adapter: literal/`${VAR}` key, `endpoint`, `auth_type`, `fallback_models`, `extras` |
| `MeshConfig` | overlay/control-plane: `network_name`, `listen_port`, `management_url`, `auth_key_env` | runtime transport: `bootstrap_peers`, heartbeat intervals, route-cache TTL, path counts |
| `AuditEntry` | resource-centric API row: `actor`, `resource_type/id`, `action` | agent-centric **SHA-256 hash-chained** record: `prev_hash`/`current_hash`, `AuditResult` |

The `AuditEntry` shapes feed a tamper-evidence hash chain, so extracting a shared
embedded struct could alter canonicalization and break verification for zero
behavioral gain.

**Decision:** keep the types separate and add cross-referencing "layering note"
doc comments to each so future contributors don't mistake them for duplicates
and try to merge them. No struct changes.

### 4 — God-module splits: deferred (tracked)

The `db.rs` / `routes/agents.rs` / `guardrails.rs` splits are pure mechanical
moves (re-export from a `mod.rs` to preserve public paths) with **no behavioral
change and no runtime benefit** — they are maintainability-only. Given they (a)
touch 4,200+ LOC, (b) risk subtle private-visibility / intra-module reference
breakage, and (c) each iteration costs a multi-minute crate rebuild in this
environment, they are deferred to dedicated follow-up commits rather than rushed
alongside the higher-value security/performance phases. They block nothing.

## Consequences

- The two real logic duplications are gone (one breaker, one shared retry helper).
- The "duplicate type" confusion is resolved by documentation, not a risky merge.
- God-module decomposition remains an explicit, tracked follow-up.
