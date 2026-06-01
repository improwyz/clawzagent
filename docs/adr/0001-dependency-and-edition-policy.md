# ADR 0001 — Workspace dependency centralization & edition policy

- **Status:** Accepted
- **Date:** 2026-05-31
- **Phase:** 3 (low-risk cleanup) of the audit & refactor program

## Context

The workspace carried two sources of truth for several shared crates:

- `[workspace.dependencies]` in the root `Cargo.toml` pinned `axum = "0.7"`,
  `tower = "0.4"`, `tower-http = "0.5"`, but **no crate referenced these via
  `workspace = true`** — they were stale, misleading config.
- `clawz-gateway` and `clawz-worker` independently pinned `axum = "0.8"`,
  `tower = "0.5"`, `tower-http = "0.6"` inline, so the real versions diverged
  from the workspace table by a full minor release.

Editions are also mixed: `clawz-gateway`, `clawz-worker`, and `clawz-services`
use edition 2024 / `rust-version = "1.87"`; the remaining non-embedded crates
(`cli`, `tui`, `tauri`, `runtime`, `platform`, `setup`) and the embedded crates
use edition 2021 / `1.75`.

## Decision

### HTTP-stack dependencies (`axum`, `tower`, `tower-http`)

Centralize the **real** versions in `[workspace.dependencies]`:

```toml
axum       = { version = "0.8", features = ["macros", "json"] }
tower      = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace"] }
```

Crates reference them with `{ workspace = true }` and add only the extra
features they need (Cargo unions feature sets):

- gateway: `axum` +`ws`, `tower-http` +`fs`, dev `tower` +`util`
- worker: base features only

This is behavior-preserving: same resolved versions and feature unions as
before, verified by an unchanged `Cargo.lock` and a green `cargo build`.

### `reqwest` — deliberately **not** centralized

`clawz-core` uses `reqwest = { workspace = true }` with default features
(native-tls via `hyper-tls`), while `gateway`/`worker`/`cli`/`setup` pin
`reqwest` inline with `rustls-tls` + `default-features = false`. Unifying these
would change the TLS backend for at least one crate — a behavior change outside
the "no behavior change in Phase 3" rule. Tracked as a follow-up: standardize
on `rustls-tls` workspace-wide in a dedicated, separately-verified commit.

### Editions

Keep the current split for now. Edition 2021 → 2024 migration of the remaining
non-embedded crates is **not** mechanical (it can surface new edition lints and
`unsafe extern` / prelude changes that require code edits) and carries no
runtime benefit, so it is deferred to its own isolated commit rather than bundled
into low-risk cleanup. Embedded crates (`clawz-embedded`, firmware) stay on their
own edition/toolchain regardless.

### Dead-code removal — deferred

The plan gates removal of `#[allow(dead_code)]` sites (48 across the workspace)
on `cargo +nightly udeps` / grep confirmation that each is truly unreachable.
Neither a nightly toolchain nor `cargo-udeps` is installed in this environment,
so removal cannot be verified safely and is deferred to avoid deleting
feature-gated or reflectively-used code. Re-attempt once the tooling is present.

## Consequences

- One source of truth for the HTTP stack; version drift can no longer hide in a
  stale workspace table.
- `reqwest` TLS unification and the edition migration remain explicit, tracked
  follow-ups rather than silent risks.
