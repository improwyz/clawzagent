# ADR 0004 — Phase 7 performance: scope & deferrals

- **Status:** Accepted
- **Date:** 2026-05-31
- **Phase:** 7 (performance)

## Delivered now

- **API-key record caching.** The auth middleware (`auth/mod.rs`) previously
  re-read `VALID_API_KEYS`, re-split it, and re-hashed (SHA-256) every plain key
  on **every authenticated request**. Records are now parsed once and cached in a
  `LazyLock<RwLock<…>>` keyed by the raw env value, so a runtime change to
  `VALID_API_KEYS` is still picked up (fail-safe on change) but the steady state
  is a single read-lock + `Arc` clone. This removes per-request allocation and
  hashing from the hot auth path. Unit test pins that repeated calls reuse the
  same cached `Arc`.

## Deferred (tracked follow-ups)

- **HashMap-indexing of hot `AppState` registries.** Several registries are
  `RwLock<Vec<_>>` scanned linearly on hot paths (e.g. inbound webhook channel
  lookup; room mutations take a global write lock and `find` over all rooms).
  Indexing by id (`HashMap`) is the right fix but changes the storage type of
  shared mutable state and ripples through every reader/writer, with concurrency
  implications that warrant load-testing not available in this environment. Best
  done as a focused, separately-benchmarked change per registry.
- **In-flight completion de-duplication** (collapsing identical concurrent
  provider calls) — needs careful correctness analysis (only safe for idempotent
  reads) and request-coalescing infrastructure; deferred.
- **Quantified before/after benchmarks.** The plan calls for `cargo bench` / a
  load script on the auth + webhook hot paths. The API-key cache's benefit is
  structural and clear (eliminates per-request parse+SHA-256), but a measured
  delta needs a load harness against a running gateway; deferred to the
  verification follow-up.

## Consequences

- The auth hot path no longer re-parses/re-hashes per request.
- Registry indexing and request coalescing remain explicit, benchmark-gated
  follow-ups rather than rushed changes to shared concurrent state.
