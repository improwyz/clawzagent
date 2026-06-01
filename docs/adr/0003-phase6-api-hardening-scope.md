# ADR 0003 — Phase 6 API hardening: scope & deferrals

- **Status:** Accepted (updated 2026-05-31 for v1.1)
- **Date:** 2026-05-31
- **Phase:** 6 (API hardening & rate limiting)

## Context

Phase 6's first cut shipped earlier: a per-node, per-actor in-memory token-bucket
rate limiter (`ratelimit.rs`) with `429` + `Retry-After` + `X-RateLimit-*`
headers, env-tunable, dev-auth-exempt. That closed the "no rate limiting"
Critical finding.

## Delivered (v1.1)

- **Request body-size limit** — `axum::DefaultBodyLimit::max(max_body_bytes())`
  on the gateway router, env-tunable via `CLAWZ_MAX_BODY_BYTES` (default 2 MiB).
- **Audit-log gaps closed** — room participant invite / role grant and cloud
  deployment teardown now emit `AppState::append_audit` entries.
- **Distributed (cross-fleet) rate counters** — once a Postgres pool is
  registered at bootstrap (`ratelimit::set_distributed_pool`), the limiter also
  enforces a shared fixed-window quota via an atomic upsert into
  `rate_limit_counters` (`ON CONFLICT … DO UPDATE SET hits = hits + 1
  RETURNING hits`). The per-node bucket stays the fast path; the DB check
  **fails open** on error so a database blip can't take the API down. Ceiling is
  env-tunable (`CLAWZ_RATELIMIT_DISTRIBUTED_PER_MIN`, default 10× the per-node
  cap). Verified end-to-end against a live Postgres.
- **Idempotency-key store** — `idempotency.rs` middleware: a mutating request
  (`POST`/`PUT`/`PATCH`) carrying an `Idempotency-Key` header has its first
  successful JSON response stored in `idempotency_keys`; a retry with the same
  key replays the stored response instead of re-running the handler (24h TTL,
  first-write-wins, fails open). Verified end-to-end (handler runs once).

Both DB features are no-ops when `DATABASE_URL` is unset, so single-node /
DB-less deployments are unaffected.

## Deferred (tracked follow-ups)

- **Per-request timeouts.** A blanket `TimeoutLayer` is unsafe: the gateway
  carries long-lived WebSocket connections and long-running LLM/streaming
  requests, which a global timeout would kill. Needs per-route-class application
  (strict on CRUD, exempt on stream/WS).
- **Concurrency cap on expensive AI routes** via `tokio::Semaphore` — needs the
  per-route wiring above to target only inference endpoints.

## Notes

- The distributed-counter and idempotency tables are added to the idempotent
  `MIGRATIONS_SQL` in `clawz-core::db`. Implementing them surfaced a pre-existing
  bug in `run_migrations` (it split the DDL on `;` without stripping `--`
  comments, so a semicolon inside a comment broke a statement); the runner now
  strips line comments before splitting.
