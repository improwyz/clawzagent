# ADR 0003 — Phase 6 API hardening: scope & deferrals

- **Status:** Accepted
- **Date:** 2026-05-31
- **Phase:** 6 (API hardening & rate limiting)

## Context

Phase 6's first cut already shipped (prior commits): a per-node, per-actor
in-memory token-bucket rate limiter (`ratelimit.rs`) with `429` + `Retry-After`
+ `X-RateLimit-*` headers, env-tunable, dev-auth-exempt. That closed the
"no rate limiting" Critical finding. This ADR records what else Phase 6 lands
now and what is deliberately deferred.

## Delivered now

- **Request body-size limit** — `axum::DefaultBodyLimit::max(max_body_bytes())`
  on the gateway router, env-tunable via `CLAWZ_MAX_BODY_BYTES`, defaulting to
  2 MiB (axum's historical default, so behavior is preserved). Bounds memory and
  rejects oversized payloads; affects only inbound request bodies, not streaming
  responses or WS upgrades.
- **Audit-log gaps closed** — the gateway already audits login/register, agent
  create/delete/onboard, policy create, deploy-create, and setup via
  `AppState::append_audit` (in-memory + best-effort Postgres). Two
  privilege-sensitive mutations were unaudited and are now covered:
  - room participant invite / role grant (`routes/rooms.rs`) — ties to the
    Phase 5 owner-role IDOR fix.
  - cloud deployment teardown (`routes/cloud_deploy.rs::destroy_deployment`).

## Deferred (tracked follow-ups)

These are enhancements on top of the working per-node limiter; none re-open a
Critical finding. They are deferred because they cannot be safely completed and
**verified** in the current environment:

- **Distributed Postgres rate counters.** Requires rewiring the stateless
  `from_fn(rate_limit)` middleware to `from_fn_with_state` to reach
  `AppState.db: Option<PgPool>`, a new windowed-counter migration, atomic upsert
  SQL, and fail-open-on-DB-error semantics. The repo uses runtime `sqlx::query`
  (so it would compile without a live DB), but shipping unverified write-path SQL
  into security middleware without an integration DB to test against is not
  acceptable. The windowing math should land with unit tests + a live-DB smoke.
- **Idempotency-key store** for non-idempotent POSTs — same blocker (needs a
  migration + live-DB verification of the key→result TTL store).
- **Per-request timeouts.** A blanket `TimeoutLayer` is unsafe here: the gateway
  carries long-lived WebSocket connections and long-running LLM/streaming
  requests, which a global timeout would kill. This needs per-route-class
  application (strict on CRUD, exempt on stream/WS), i.e. handler-level wiring.
- **Concurrency cap on expensive AI routes** via `tokio::Semaphore` — needs the
  per-route wiring above to target only inference endpoints.

## Consequences

- Inbound payloads are explicitly bounded and operator-tunable.
- All privilege-sensitive gateway mutations now emit audit entries.
- Distributed quota, idempotency, and differentiated timeouts remain explicit,
  tracked items requiring a live Postgres for safe rollout — not silent gaps.
