# ClawZ Deployment & Build Strategy

**Document version:** 1.1  
**Date:** 2026-05-28  
**Audience:** Operators, maintainers, and contributors deploying ClawZ from GitHub  
**Repository:** [github.com/improwyz/clawz](https://github.com/improwyz/clawz)

---

## Executive summary

ClawZ is a **Rust workspace (~65K LOC)** with a **3-tier runtime** (gateway → worker → core), optional **React dashboard** (`web/`), and **Docker Compose micro/fleet** as the default production shape on a VPS. Today, the slowest path—and the one many operators accidentally use—is **recompiling the full Rust workspace inside Docker on every `git pull`**, which takes **10–20+ minutes** per deploy.

The platform already supports a **fast path** (pull prebuilt images from GHCR in under a minute), but that path is **misaligned with `main` branch development**: images are published only on **version tags** (`v*`), not on every merge to `main`. Dashboard-only fixes therefore still tempt operators into `--build`, which is the wrong tool for frequent UI rollouts.

**Recommended direction (phased):**

| Phase | Focus | New-server time | Typical update time |
|-------|--------|-----------------|---------------------|
| **0** (now) | Documented runbooks + `scripts/deploy.sh` | ~3–5 min (pull + compose) | **30s–2 min** (pull or web-only rebuild) |
| **1** | CI publishes `main` images + web image; cargo-chef Dockerfiles | ~3–5 min | **1–3 min** (pull changed services) |
| **2** | Gateway static dashboard; digest-pinned releases; rollback | ~2–4 min | **30s–90s** (single compose pull) |
| **3** (optional) | Slim gateway crate boundary; remote builders | Same | Faster cold builds for maintainers |

This document analyzes the current system, compares options, and provides implementation-ready plans for Phases 0–2.

---

## Table of contents

1. [Platform analysis](#1-platform-analysis)  
2. [Current build & deploy topology](#2-current-build--deploy-topology)  
3. [Pain points & root causes](#3-pain-points--root-causes)  
4. [Use cases & requirements](#4-use-cases--requirements)  
5. [Deployable units & change matrix](#5-deployable-units--change-matrix)  
6. [Options analysis](#6-options-analysis)  
7. [Recommended strategy](#7-recommended-strategy)  
8. [Phase 0: Immediate improvements](#8-phase-0-immediate-improvements)  
9. [Phase 1: CI/CD & registry](#9-phase-1-cicd--registry)  
10. [Phase 2: Docker, web packaging & operations](#10-phase-2-docker-web-packaging--operations)  
11. [Phase 3: Architecture (longer term)](#11-phase-3-architecture-longer-term)  
12. [Runbooks](#12-runbooks)  
13. [Rollback & release channels](#13-rollback--release-channels)  
14. [Security & compliance notes](#14-security--compliance-notes)  
15. [Implementation backlog](#15-implementation-backlog)  
16. [Decision tree](#16-decision-tree)  
17. [Appendix](#17-appendix)  
18. [TUI, onboarding & first-run UX](#18-tui-onboarding--first-run-ux)

---

## 1. Platform analysis

### 1.1 Architecture (runtime)

ClawZ follows a **gateway / worker / core** split documented in [ARCHITECTURE.md](ARCHITECTURE.md) and [AGENTS.md](../AGENTS.md):

```
┌─────────────────────────────────────────────────────────────┐
│  Clients: Web dashboard, REST, WebSocket, MCP, Tauri, CLI   │
└────────────────────────────┬────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────┐
│  clawz-gateway (Axum) — :3000                               │
│  Auth, routes, scheduling, deploy adapters, connectors, WS  │
└────────────────────────────┬────────────────────────────────┘
                             │ HTTP/gRPC (WORKER_URL)
┌────────────────────────────▼────────────────────────────────┐
│  clawz-worker — :50051                                      │
│  Runtime pipeline, governance, tools, providers, fleet/Docker│
└────────────────────────────┬────────────────────────────────┘
                             │
┌────────────────────────────▼────────────────────────────────┐
│  clawz-core + Postgres (pgvector) + optional mesh/elastic   │
└─────────────────────────────────────────────────────────────┘
```

**Deployment modes** (`CLAWZ_MODE`):

| Mode | Typical use | Compose stack |
|------|-------------|---------------|
| `standalone` | Dev, single binary | Source or minimal Docker |
| `micro` | **Default VPS** | gateway + worker + db |
| `elastic` | Multi-node mesh | Same + discovery/TLS |

### 1.2 Workspace crates (build relevance)

| Crate | Role in deploy | Build in production? |
|-------|----------------|----------------------|
| `clawz-gateway` | **Always** (API :3000) | Yes |
| `clawz-worker` | **Always** (control :50051) | Yes |
| `clawz-core` | Library | Transitive |
| `clawz-platform` | Gateway persistence | Transitive |
| `clawz-services` | Shared services | Transitive |
| `clawz-runtime` | Agent runtime lib | Transitive |
| `clawz-agent` | Per-tenant containers | Image `clawz-agent` (worker spawns) |
| `clawz-tauri` | Desktop shell | **Exclude** from server CI (`CARGO_WORKSPACE_EXCLUDE`) |
| `clawz-embedded` | Firmware/embedded | Not in default VPS stack |
| `web/` | React dashboard | Separate npm build (not in gateway image today) |

**Important coupling:** `clawz-gateway` **depends on `clawz-worker` as a path crate** (governance, identity store, docker tool catalog, orchestration specs). Building the gateway **compiles a large fraction of the worker crate** even when only gateway code changed. This is a major contributor to compile time and should be addressed in Phase 3.

### 1.3 External dependencies (runtime)

| Dependency | Image / service | Notes |
|------------|-----------------|-------|
| PostgreSQL 15 + pgvector | `pgvector/pgvector:pg15` | Public; data persists in `postgres-data` volume |
| Docker socket | Mounted on worker | Fleet/agent containers |
| GHCR | `ghcr.io/improwyz/clawz-{gateway,worker,agent}` | Private; PAT `read:packages` |
| LLM providers | Env / `providers` API | Optional stub: `CLAWZ_STUB_PROVIDER=1` |

### 1.4 What operators actually deploy

On a typical VPS (e.g. production dashboard + API):

1. **Platform:** `docker compose` → gateway, worker, db  
2. **Dashboard:** `web/dist` served via `vite preview` (port 4173) or reverse proxy  
3. **Secrets:** `.env` (JWT, worker token, API keys, `DATABASE_URL`)

The gateway **does not** currently serve `web/dist` as static files ([server.rs](../crates/clawz-gateway/src/server.rs) has no `ServeDir`). The dashboard is always a **second process** or external CDN/nginx.

---

## 2. Current build & deploy topology

### 2.1 Artifacts

| Artifact | How produced | Size / time (typical) |
|----------|--------------|------------------------|
| `clawz-gateway` binary | `cargo build --release -p clawz-gateway` | Large; first build 15–30 min cold |
| `clawz-worker` binary | `cargo build --release -p clawz-worker` | Similar |
| Docker `clawz-gateway` | [Dockerfile.gateway](../Dockerfile.gateway) | Full workspace compile in container |
| Docker `clawz-worker` | [Dockerfile.worker](../Dockerfile.worker) | Full workspace compile |
| Docker `clawz-agent` | [Dockerfile.agent](../Dockerfile.agent) | Worker crate + agent bin |
| `web/dist` | `cd web && npm run build` | ~1–3 min |
| GHCR images | [.github/workflows/release.yml](../.github/workflows/release.yml) on tag `v*` | Built in CI with GHA cache |

### 2.2 Install paths (today)

| Path | Entry | Duration (first / update) |
|------|-------|---------------------------|
| **Prebuilt (default)** | `./scripts/install.sh` + GHCR login | **~3–5 min** / **~1 min** pull |
| **Local Docker build** | `./install.sh --build` or `compose.build.yml` | **10–20+ min** / **10–20+ min** (poor cache) |
| **Source** | `./install.sh --source` | **15–30 min** / **2–5 min** incremental on host |
| **Manual VPS habit** | `docker compose up -d --build gateway` + `npm ci && npm run build` | **Worst case** every pull |

### 2.3 Dockerfiles (critical flaw)

Both gateway and worker Dockerfiles use the same anti-pattern:

```dockerfile
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --release -p clawz-gateway   # or worker
```

**Any** change under `crates/` invalidates the Docker layer and forces a **full dependency + workspace rebuild**. CI mitigates this with `cache-from: type=gha` ([ci.yml](../.github/workflows/ci.yml), [release.yml](../.github/workflows/release.yml)); **VPS builds do not**.

### 2.4 CI vs release gap

| Workflow | Trigger | Publishes images? |
|----------|---------|------------------|
| **CI** | `push` to `main`, PRs | Builds Docker locally in CI only (`load: true`), **does not push** |
| **Release** | Tag `v*` or `workflow_dispatch` | Pushes `latest` + semver to GHCR |

**Consequence:** Operators on `main` who `git pull` but use `CLAWZ_IMAGE_TAG=latest` from GHCR may **not** get their latest commits until maintainers tag a release. This pushes them toward `--build`, which is slow.

### 2.5 Web dashboard deploy

- Built with Vite 6 + React 19 ([web/package.json](../web/package.json))  
- [scripts/serve-web-dashboard.sh](../scripts/serve-web-dashboard.sh) runs `vite preview` with proxy to gateway  
- `web/dist` is gitignored; must be rebuilt on server unless served from a **web image** or static mount  

---

## 3. Pain points & root causes

| Symptom | Root cause |
|---------|------------|
| 10–20 min every deploy | VPS `docker compose --build` with no cargo layer cache |
| Rebuild after one-line Rust fix | Dockerfile copies all sources before `cargo build` |
| Gateway build compiles “everything” | Path dep `clawz-gateway` → `clawz-worker` (+ heavy deps: sqlx, bollard, etc.) |
| `main` pull doesn’t update running code | Prebuilt images only on `v*` tags |
| Dashboard + API both “feel” slow | Unconditional `npm ci` + full Docker rebuild |
| Two processes to manage | Gateway doesn’t embed `web/dist`; preview is separate |
| compose-smoke always builds | [compose-smoke.sh](../scripts/compose-smoke.sh) sets `docker-compose.build.yml` |

---

## 4. Use cases & requirements

### 4.1 Primary use cases

| ID | Scenario | Success criteria |
|----|----------|------------------|
| **U1** | **New server** from GitHub | Single documented command; <5 min to healthy gateway |
| **U2** | **Dashboard bugfix** | Update UI without recompiling Rust; <3 min |
| **U3** | **Gateway API fix** | Roll out gateway only; <5 min; worker unchanged if possible |
| **U4** | **Worker/runtime fix** | Roll out worker; gateway compatible |
| **U5** | **Pinned production** | Reproducible version; easy rollback |
| **U6** | **Bleeding edge** | Test `main` without 20 min local compile |
| **U7** | **Air-gapped / no GHCR** | Documented slow path with caching improvements |

### 4.2 Non-goals (for this plan)

- Replacing Docker with Kubernetes (optional future; compose remains default)  
- Building `clawz-tauri` in server CI  
- Multi-region active-active (elastic mode is documented but not required for VPS plan)

### 4.3 Constraints

- **ELv2 license** — distribution via GHCR/binaries is fine; respect license on forks  
- **Private GHCR** — requires PAT; install already enforces this  
- **Worker needs Docker socket** for fleet agents  
- **Postgres volume** must survive updates  

---

## 5. Deployable units & change matrix

Use this to **avoid rebuilding everything**:

| Changed paths | Redeploy | Do not rebuild |
|---------------|----------|----------------|
| `web/**` only | `web` (npm build + preview/restart) | gateway, worker |
| `crates/clawz-gateway/**` | `gateway` image/binary | worker (if ABI/API unchanged)* |
| `crates/clawz-worker/**`, `clawz-core/**` | `worker` (+ often gateway due to path dep) | web |
| `docker-compose.yml`, `.env` | `compose up -d` (recreate) | — |
| DB migrations (future) | Documented migration step | — |

\*Today gateway links worker crate; worker-only deploy is still possible if gateway API unchanged, but gateway image may need rebuild for consistency.

---

## 6. Options analysis

### Option A — Prebuilt GHCR only (status quo enhanced)

**Description:** Default install pulls `ghcr.io/improwyz/clawz-{gateway,worker}:${TAG}`; never `--build` on VPS.

| Pros | Cons |
|------|------|
| Fastest operator experience | `latest` stale vs `main` until tagged |
| No Rust on server | Requires registry auth |
| Matches [private-registry.md](private-registry.md) | |

**Best for:** U1, U5  
**Needs:** CI publishing strategy (Option B) for U3/U6  

---

### Option B — CI images on every `main` push

**Description:** Add workflow (or extend CI) to push:

- `ghcr.io/improwyz/clawz-gateway:main` (or `sha-<short>`)  
- `ghcr.io/improwyz/clawz-worker:main`  
- Keep `latest` for **latest tag release** only (or move `latest` → `main` with docs)

| Pros | Cons |
|------|------|
| `git pull` + `pull` = current `main` | Registry storage/retention policy |
| No VPS compile | Slightly more CI minutes |
| Enables [U6] | Must document tag semantics |

**Best for:** U2–U6  
**Effort:** Medium (workflow + docs)  

---

### Option C — cargo-chef + BuildKit cache in Dockerfiles

**Description:** Refactor Dockerfiles:

1. `cargo chef prepare` / `cargo chef cook --release` for dependency layer  
2. `RUN --mount=type=cache,target=/usr/local/cargo/registry`  
3. Final `cargo build` only for app crate  

| Pros | Cons |
|------|------|
| `--build` on VPS drops to **2–5 min** for code-only changes | Still slower than pull |
| Helps maintainers & air-gap | Initial implementation effort |
| Aligns VPS with CI caching | |

**Best for:** U7, maintainers, `--build` fallback  
**Effort:** Medium  

---

### Option D — Host `cargo` + binary bind-mount / copy

**Description:** Install Rust on VPS once; `cargo build --release -p clawz-gateway`; mount binary into minimal runtime container or systemd unit.

| Pros | Cons |
|------|------|
| Incremental builds **fast** on repeat | Rust toolchain on production host |
| No Docker build cache needed | Operator skill required |
| Good for single-tenant VPS | Two deployment styles to support |

**Best for:** Power users, dev/staging VPS  
**Effort:** Low (document + optional script)  

---

### Option E — Web as Docker image or gateway static files

**Description (E1 – web image):** `Dockerfile.web` → nginx or `vite preview` in container; compose service `dashboard:4173`.

**Description (E2 – gateway static):** Build `web/dist` in CI; `COPY` into gateway image; Axum `ServeDir` at `/` or `/dashboard`.

| Pros | Cons |
|------|------|
| Single `compose pull` updates UI | E2 rebuilds gateway when web changes |
| No separate Node on server (E2) | E1 adds third image |
| E2: one port, simpler TLS | SPA routing needs fallback to `index.html` |

**Best for:** U1, U2, U5  
**Effort:** E1 medium; E2 medium-high  

**Recommendation:** **E1 for Phase 1** (clean separation); **E2 for Phase 2** (simplest operator UX).

---

### Option F — Unified `scripts/deploy.sh`

**Description:** One script: detect changed paths (`git diff`), choose pull/build targets, health-check, optional rollback hint.

| Pros | Cons |
|------|------|
| Eliminates operator guesswork | Must maintain path→service map |
| Enforces fast path | |

**Best for:** All use cases  
**Effort:** Low–medium  

---

### Option G — Release channels

| Channel | Image tag | Source |
|---------|-----------|--------|
| **stable** | `v1.0.0`, `latest` | Git tag |
| **rc** | `v1.1.0-rc.1` | Pre-release tag |
| **nightly** | `main`, `sha-abc1234` | `main` branch CI |

| Pros | Cons |
|------|------|
| Clear rollback (pin `v1.0.0`) | More tags to manage |
| Production uses stable | |

**Best for:** U5, U6  

---

### Option H — Remote builders (BuildCloud, self-hosted runner)

**Description:** VPS triggers build on GitHub Actions / BuildKit remote; pulls result.

| Pros | Cons |
|------|------|
| Zero compile on VPS | Complexity, secrets, cost |

**Best for:** Large teams; defer unless GHCR push insufficient  

---

### Comparison summary

| Option | New server | Update (typical) | Ops complexity | Implement effort |
|--------|------------|------------------|----------------|----------------|
| A Prebuilt | ★★★★★ | ★★★★ (if tag fresh) | Low | None |
| B CI `main` images | ★★★★★ | ★★★★★ | Low | Medium |
| C cargo-chef | ★★ | ★★★ | Medium | Medium |
| D Host cargo | ★★★ | ★★★★ | Medium | Low |
| E Web image/static | ★★★★ | ★★★★★ (web) | Low–medium | Medium |
| F deploy.sh | ★★★★★ | ★★★★★ | Low | Low |
| G Channels | ★★★★ | ★★★★★ | Low | Medium |
| H Remote builder | ★★★★ | ★★★★ | High | High |

**Recommended bundle:** **A + B + E1 + F + G** (short term), add **C** for `--build` fallback, **E2** when ready for single-port UX.

---

## 7. Recommended strategy

### Design principles

1. **Never compile Rust on the VPS by default** — pull immutable images.  
2. **Separate artifacts** — gateway, worker, web are independent versioned units.  
3. **CI owns builds** — VPS only pulls, recreates containers, runs migrations.  
4. **Pin production, float staging** — `CLAWZ_IMAGE_TAG=v1.0.0` in prod; `main` on staging.  
5. **Change detection** — deploy script rebuilds/pulls only what changed.  

### Target state (12-month)

```
GitHub (main / tags)
       │
       ▼
┌──────────────────┐     ┌─────────────────┐     ┌──────────────────┐
│  CI: build push  │────►│ GHCR            │────►│ VPS: compose pull │
│  gateway/worker  │     │ :main :v* :sha  │     │ + health + rollback│
│  web dashboard   │     └─────────────────┘     └──────────────────┘
└──────────────────┘
```

---

## 8. Phase 0: Immediate improvements

**Goal:** Faster deploys **without** waiting for CI/Dockerfile refactors.  
**Timeline:** 1–2 days  

### 8.1 Operator documentation

Add to [INSTALL.md](../INSTALL.md) / [private-registry.md](private-registry.md):

- **“Never run `docker compose --build` on a VPS for routine updates.”**  
- Pin tags: `export CLAWZ_IMAGE_TAG=v1.0.0`  
- Dashboard-only workflow (no Docker)  

### 8.2 `scripts/deploy.sh` (recommended implementation)

Behavior:

```bash
./scripts/deploy.sh              # auto: git pull, detect changes, pull/up services
./scripts/deploy.sh --web-only   # npm run build + restart dashboard
./scripts/deploy.sh --pull       # force image pull (gateway + worker)
./scripts/deploy.sh --tag v1.0.0 # pin before pull
./scripts/deploy.sh --build      # explicit slow path (warn + confirm)
```

**Change detection (git):**

```text
web/                          → web-only
crates/clawz-gateway/         → gateway
crates/clawz-worker/          → worker
crates/clawz-core/            → gateway + worker
Cargo.lock                    → gateway + worker (pull, not local build)
```

Use `docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml`.

### 8.3 VPS `.env` template

```bash
CLAWZ_IMAGE_TAG=v1.0.0          # production pin
CLAWZ_REGISTRY=ghcr.io/improwyz
GITHUB_TOKEN=...                 # read:packages
GITHUB_USER=...
CLAWZ_WEB_PORT=4173
```

### 8.4 Fast commands (interim)

**New server:**

```bash
export GITHUB_TOKEN=ghp_...
export GITHUB_USER=your_user
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz
cd ~/clawz && ./scripts/install.sh --with-web
./scripts/serve-web-dashboard.sh
```

**Dashboard-only update:**

```bash
cd ~/clawz && git pull
cd web && npm run build    # skip npm ci if package-lock unchanged
# restart vite preview / systemd unit
```

**Platform update (prebuilt):**

```bash
cd ~/clawz && git pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull gateway worker
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d gateway worker
```

---

## 9. Phase 1: CI/CD & registry

**Goal:** `main` branch deploys in **under 3 minutes** on VPS.  
**Timeline:** 1–2 weeks  

### 9.1 New workflow: `docker-publish-main.yml`

On push to `main` (after CI green):

- Build & push `clawz-gateway:main`, `clawz-worker:main`, `clawz-agent:main`  
- Also push `clawz-gateway:sha-<7char>` for exact rollback  
- Use existing Buildx + `cache-from/to: type=gha`  

**Do not** move production `latest` to every `main` commit without docs—prefer explicit tags:

| Tag | Meaning |
|-----|---------|
| `latest` | Latest **semver release** (`v*`) |
| `main` | Tip of `main` branch |
| `v1.0.0` | Immutable release |

### 9.2 Web image

`Dockerfile.web`:

```dockerfile
FROM node:22-bookworm AS build
WORKDIR /app
COPY web/package*.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM nginx:alpine
COPY --from=build /app/dist /usr/share/nginx/html
COPY deploy/nginx-dashboard.conf /etc/nginx/conf.d/default.conf
```

`docker-compose.prebuilt.yml` addition:

```yaml
dashboard:
  image: ${CLAWZ_REGISTRY}/clawz-dashboard:${CLAWZ_IMAGE_TAG:-main}
  ports:
    - "4173:80"
  depends_on:
    - gateway
```

Nginx proxies `/api` and `/ws` to `gateway:3000` (removes Node from VPS).

### 9.3 Update install script

- Default `CLAWZ_IMAGE_TAG` for **staging**: `main`  
- Document production pin to `v*`  
- `pull_prebuilt_images` includes `dashboard` when `--with-web`  

### 9.4 Release workflow alignment

Keep [release.yml](../.github/workflows/release.yml) for tagged releases; add post-release step to update `latest` only from tags.

---

## 10. Phase 2: Docker, web packaging & operations

**Goal:** Reliable fast builds when compile is unavoidable; simpler single-endpoint UX.  
**Timeline:** 2–4 weeks  

### 10.1 cargo-chef Dockerfiles (gateway & worker)

Sketch:

```dockerfile
FROM rust:1.88-bookworm AS chef
RUN cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo chef cook --release --recipe-path recipe.json -p clawz-gateway

COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p clawz-gateway
```

Apply to `Dockerfile.gateway`, `Dockerfile.worker`, `Dockerfile.agent`.

### 10.2 Optional: gateway serves dashboard (E2)

- CI job builds `web/dist`, passes as build context to gateway image  
- Add `tower_http::services::ServeDir` + SPA fallback in [server.rs](../crates/clawz-gateway/src/server.rs)  
- Operators expose only **port 3000** behind TLS  

**Trade-off:** Gateway image rebuilds on every UI change—acceptable if CI builds are fast and web changes are bundled in same pull.

### 10.3 `docker-compose.prod.yml` overlay

Production-oriented overlay:

- No published db port  
- Resource limits  
- `restart: unless-stopped`  
- Healthchecks on gateway/worker  
- Optional Caddy/Traefik labels  

### 10.4 Database migrations

Introduce `sqlx migrate` or versioned SQL in repo; `deploy.sh` runs migrations before `up -d gateway`.

---

## 11. Phase 3: Architecture (longer term)

**Goal:** Shorter compile graph and cleaner deploy units.  

### 11.1 Split `clawz-gateway` from `clawz-worker` crate dependency

Extract shared types into `clawz-core` or new `clawz-contracts`:

- Approval workflow interfaces  
- Identity store trait (gateway holds `Arc<dyn …>`)  
- Tool catalog metadata (JSON or thin module in core)  

**Impact:** Gateway release builds may drop **30–50%** compile time.

### 11.2 Slim gateway feature flags

```toml
[features]
default = ["dashboard-api", "connectors"]
connectors = [...]  # optional for minimal edge deploy
```

### 11.3 Binary releases alongside Docker

CI already uploads `clawz-{amd64,arm64}` on tag ([release.yml](../.github/workflows/release.yml)). Document systemd units for bare-metal without Docker.

---

## 12. Runbooks

### 12.1 New server bootstrap (recommended)

**Prerequisites:** Ubuntu 22.04+, Docker 24+, Compose v2, 4GB+ RAM, ports 3000 (API), 4173 (UI optional).

```bash
# 1. Auth for private images
export GITHUB_TOKEN=ghp_xxxx   # read:packages
export GITHUB_USER=your_github_username

# 2. Clone & install (prebuilt)
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz
cd ~/clawz
cp .env.example .env
# Edit .env: secrets, CLAWZ_PUBLIC_URL, disable CLAWZ_DISABLE_AUTH in prod

# 3. Pin version (production)
export CLAWZ_IMAGE_TAG=v1.0.0   # or 'main' for staging

./scripts/install.sh --with-web

# 4. Verify
curl -sf http://127.0.0.1:3000/health
curl -sf http://127.0.0.1:3000/api/v1/system/health

# 5. Dashboard (until web image in compose)
./scripts/serve-web-dashboard.sh
```

**TLS:** Put Caddy/nginx in front of 3000/4173; terminate TLS; proxy WebSocket.

### 12.2 Routine update (production, pinned)

```bash
cd ~/clawz
git fetch --tags
git checkout v1.0.1          # or stay on branch + set CLAWZ_IMAGE_TAG
export CLAWZ_IMAGE_TAG=v1.0.1

docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d

# If only web changed in this release:
cd web && npm run build && systemctl restart clawz-dashboard  # or serve script
```

### 12.3 Staging update (track `main`)

```bash
cd ~/clawz && git pull origin main
export CLAWZ_IMAGE_TAG=main
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull gateway worker dashboard
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
```

### 12.4 Hotfix (dashboard only)

```bash
cd ~/clawz && git pull
cd web && npm run build
# restart preview/systemd — no docker
```

### 12.5 Maintainer slow path (no registry)

```bash
export DOCKER_BUILDKIT=1
./scripts/install.sh --build
```

Prefer fixing registry access over defaulting to this path.

---

## 13. Rollback & release channels

### 13.1 Image rollback

```bash
export CLAWZ_IMAGE_TAG=v1.0.0   # previous known-good
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
```

Or pin by digest:

```bash
export CLAWZ_IMAGE_TAG=sha256:abc123...
```

### 13.2 Git rollback

```bash
git checkout v1.0.0
# match CLAWZ_IMAGE_TAG to same tag
```

### 13.3 Database caution

Rolling back **binaries** without reversing **migrations** can break schema compatibility. Phase 2 should add forward-only migrations with compatibility notes in release changelog.

### 13.4 Channel policy (recommended)

| Environment | Git | Image tag |
|-------------|-----|-----------|
| Production | Tag `v*` | Same `v*` or digest |
| Staging | `main` | `main` |
| Dev laptop | branch | local build or `main` |

---

## 14. Security & compliance notes

- **Never** commit `GITHUB_TOKEN`, JWT secrets, or provider keys; use `.env` + restricted permissions.  
- PAT scope: **`read:packages` only** on production VPS (not `write`).  
- Rotate `CLAWZ_JWT_SECRET`, `CLAWZ_WORKER_TOKEN`, API keys on compromise.  
- Production: set `CLAWZ_DISABLE_AUTH=0`, configure `VALID_API_KEYS`, TLS at edge.  
- Worker Docker socket is **high privilege** — isolate VPS, firewall, non-root where possible.  
- GHCR image signing (cosign) — optional Phase 2 enhancement for supply chain.  

---

## 15. Implementation backlog

| ID | Task | Phase | Effort | Impact |
|----|------|-------|--------|--------|
| P0-1 | Add `scripts/deploy.sh` with change detection | 0 | S | ★★★★★ |
| P0-2 | INSTALL.md “fast vs slow deploy” section | 0 | S | ★★★★ |
| P0-3 | Link this doc from README / INSTALL | 0 | S | ★★★ |
| P1-1 | Workflow: push `main` images to GHCR | 1 | M | ★★★★★ |
| P1-2 | `Dockerfile.web` + compose `dashboard` service | 1 | M | ★★★★ |
| P1-3 | `install.sh` pull dashboard when `--with-web` | 1 | S | ★★★★ |
| P1-4 | Document tag channels (`latest` vs `main` vs `v*`) | 1 | S | ★★★★ |
| P2-1 | cargo-chef Dockerfiles (gateway/worker/agent) | 2 | M | ★★★★ |
| P2-2 | `docker-compose.prod.yml` overlay | 2 | S | ★★★ |
| P2-3 | Gateway `ServeDir` for embedded dashboard (optional) | 2 | M | ★★★ |
| P2-4 | sqlx migrations in deploy path | 2 | M | ★★★★ |
| P3-1 | Extract gateway↔worker shared crate boundary | 3 | L | ★★★★ |
| P3-2 | cosign image signing | 3 | M | ★★ |

**Effort key:** S = hours, M = days, L = weeks  

---

## 16. Decision tree

```text
Need to deploy ClawZ?
│
├─ New server?
│   └─ Yes → ./scripts/install.sh (prebuilt) + GHCR auth
│            └─ Production? → CLAWZ_IMAGE_TAG=v* (not main)
│
├─ What changed?
│   ├─ web/ only → npm run build + restart dashboard (NO docker build)
│   ├─ gateway Rust → pull :main or :v* gateway image only
│   ├─ worker Rust → pull worker image only
│   └─ Cargo.lock / core → pull gateway + worker
│
├─ Image not on GHCR yet?
│   └─ Fix auth OR wait for CI tag OR ./install.sh --build (last resort)
│
└─ Production rollback?
    └─ CLAWZ_IMAGE_TAG=<previous v*> + compose pull + up -d
```

---

## 17. Appendix

### 17.1 Key files reference

| File | Purpose |
|------|---------|
| [scripts/install.sh](../scripts/install.sh) | One-click install |
| [scripts/install-common.sh](../scripts/install-common.sh) | Prebuilt pull, compose modes |
| [docker-compose.yml](../docker-compose.yml) | Base stack |
| [docker-compose.prebuilt.yml](../docker-compose.prebuilt.yml) | GHCR images |
| [docker-compose.build.yml](../docker-compose.build.yml) | Local compile overlay |
| [Dockerfile.gateway](../Dockerfile.gateway) | Gateway image (needs chef) |
| [docs/private-registry.md](private-registry.md) | GHCR auth |
| [.github/workflows/release.yml](../.github/workflows/release.yml) | Tagged releases |
| [.github/workflows/ci.yml](../.github/workflows/ci.yml) | CI (no push today) |
| [scripts/serve-web-dashboard.sh](../scripts/serve-web-dashboard.sh) | Headless dashboard |

### 17.2 Environment variables (deploy-related)

| Variable | Default | Notes |
|----------|---------|-------|
| `CLAWZ_REGISTRY` | `ghcr.io/improwyz` | Image prefix |
| `CLAWZ_IMAGE_TAG` | `latest` | Pin `v*` in prod; use `main` for staging |
| `CLAWZ_AGENT_IMAGE` | `{registry}/clawz-agent:{tag}` | Worker spawns agents |
| `GITHUB_TOKEN` | — | `read:packages` for pull |
| `CLAWZ_MODE` | `micro` in compose | standalone / elastic also supported |
| `CLAWZ_WEB_PORT` | `4173` | Dashboard preview |

### 17.3 Expected timings (targets after Phase 1)

| Operation | Today (worst) | Target |
|-----------|---------------|--------|
| New server (prebuilt + web image) | 3–20 min | **3–5 min** |
| Pull gateway+worker update | 1–20 min | **1–2 min** |
| Dashboard-only | 2–5 min | **1–2 min** |
| Local `--build` after chef | 10–20 min | **3–8 min** (code change) |

### 17.4 Related documents

- [INSTALL.md](../INSTALL.md) — installation guide  
- [ARCHITECTURE.md](ARCHITECTURE.md) — system design  
- [private-registry.md](private-registry.md) — GHCR authentication  
- [AGENTS.md](../AGENTS.md) — development reference  

---

## 18. TUI, onboarding & first-run UX

This section maps **how operators and users get started today**, how that differs from the **gateway TUI module**, and how onboarding should fit the deployment strategy in [§7](#7-recommended-strategy).

### 18.1 Four separate “onboarding” surfaces (today)

ClawZ does **not** have one unified onboarding flow. There are four independent mechanisms:

| Surface | Location | Wired to production? | What it does |
|---------|----------|----------------------|--------------|
| **Install script** | [scripts/install.sh](../scripts/install.sh), [install-common.sh](../scripts/install-common.sh) | **Yes** — primary VPS path | Clone/pull, `.env`, GHCR login, `compose up`, health wait |
| **Gateway TUI** | [crates/clawz-gateway/src/tui/mod.rs](../crates/clawz-gateway/src/tui/mod.rs) | **No** — not invoked from any binary | Stdin prompts → prints `export` lines; config sub-menus are stubs |
| **Web dashboard** | [web/src/pages/Login.tsx](../web/src/pages/Login.tsx) | **Yes** (when `web/` is built) | Login, register, API key; gates the React app |
| **HTTP APIs** | Gateway routes | **Yes** | Agent goal onboarding, mobile pairing, auth |

**Implication for deploy:** Fast VPS rollout uses **`install.sh` + web Login**, not the TUI. The TUI does not affect build time unless someone mistakenly tries to compile/run it as a separate product.

### 18.2 Gateway TUI — current implementation status

Module: `clawz_gateway::tui` ([tui/mod.rs](../crates/clawz-gateway/src/tui/mod.rs)).

| Entry point | Purpose | Production-ready? |
|-------------|---------|-------------------|
| `run_onboarding()` | Port, JWT, provider keys, log level, data dir → prints env exports | **Dev helper only** — env names partially wrong (`JWT_SECRET` vs `CLAWZ_JWT_SECRET`) |
| `run_dashboard()` | ASCII stats panel | **Static mock data** — does not call `/dashboard/metrics` |
| `run_config()` | Providers / channels / governance / deploy menus | **Stubs** — no persistence, no API calls |

**Critical gap:** [`clawz-gateway` binary](../crates/clawz-gateway/src/bin/clawz-gateway.rs) has **no subcommands** (`--tui`, `setup`, `config`). Nothing in the repo calls `run_onboarding`, `run_config`, or `run_dashboard` except unit tests. ROADMAP lists “TUI: agent monitoring, policy inspection” as future work.

The module’s own docs state it is **for local development**, not production servers (stdin/stdout, no `ratatui`).

### 18.3 What actually onboards a new server (operator)

**Recommended operator journey** (aligns with [§12.1](#121-new-server-bootstrap-recommended)):

```text
1. Prerequisites     Docker, Compose, GHCR PAT (read:packages)
2. install.sh        Creates .env, pulls images, starts db/worker/gateway
3. Health checks     curl /health, /api/v1/system/health
4. Web (optional)    install.sh --with-web OR serve-web-dashboard.sh
5. First login       Dashboard → register/login OR API key (Config tab)
6. Platform config   Config / Monitoring pages (or REST) — providers, governance
7. First agent       Agents UI or POST /api/v1/agents or POST /agents/onboard
```

**Not in this path:** running a TUI inside the gateway container (distroless image has no shell/interactive stdin).

### 18.4 API-level onboarding (runtime / product)

These work over HTTP and **do** belong in production:

| Endpoint | Role |
|----------|------|
| `POST /api/v1/system/auth/register` | Dashboard user registration |
| `POST /api/v1/system/auth/login` | JWT session for web |
| `GET /api/v1/system/auth/status` | Auth gate for UI |
| `POST /api/v1/system/pairing` | Short-lived code (legacy mobile); 5 min TTL |
| `POST /api/v1/agents/onboard` | Goal text → parsed agent + audit + identity hook |
| `GET /api/v1/dashboard/overview` | Post-install “what’s running” + API catalog (Monitoring page) |

Agent onboard uses `GoalParser` in the worker crate; it is **product onboarding** (first agent), not **server provisioning**.

### 18.5 Tauri desktop (related, separate artifact)

`clawz-tauri` is a **desktop shell** (system tray, gateway URL, permissions) — not the gateway TUI. It has its own build chain (`cargo tauri dev/build`) and is **excluded from server CI**. Desktop onboarding is not coupled to Docker/VPS deploy; document separately if shipping desktop + server together.

### 18.6 Gaps vs operator expectations

| Expectation | Reality |
|-------------|---------|
| “`clawz-gateway setup` on the VPS” | **Does not exist** |
| TUI writes `.env` or updates Postgres | **No** — only prints exports |
| TUI shows live metrics after deploy | **No** — mock strings; web Monitoring page is the real view |
| Same flow for Docker and bare metal | Install script covers Docker; TUI targets bare `clawz-gateway` hints |
| Onboarding includes GHCR login | **install.sh** only |

### 18.7 Recommendations (fit with deploy phases)

#### Phase 0 — Document and redirect (no new compile)

1. **INSTALL.md** — Add “First run” subsection: install.sh → health → web login → Config/Monitoring (explicitly: **do not use TUI on production VPS**).
2. **install.sh success banner** — Print dashboard URL, default auth note (`CLAWZ_DISABLE_AUTH`), link to Monitoring.
3. **Deprecate or label TUI** in AGENTS.md as “dev-only, unwired” until CLI lands.

#### Phase 1 — Operator CLI (replaces TUI for servers)

Add a **`clawz` or `clawz-gateway` subcommand** (does not require full gateway compile on target if shipped as small binary):

```bash
clawz setup          # interactive: GHCR, .env, CLAWZ_IMAGE_TAG, CLAWZ_PUBLIC_URL
clawz doctor         # curl health, compose status, image tag vs git rev
clawz onboard-agent  # thin wrapper → POST /agents/onboard
```

Implementation options:

| Option | Build impact | Notes |
|--------|--------------|-------|
| **A. Subcommands on gateway binary** | None extra if already deploying gateway | `clawz-gateway setup` before `serve` |
| **B. Separate `clawz-cli` crate** | Small crate, fast compile | Best for VPS without Rust toolchain |
| **C. Extend `scripts/deploy.sh`** | Bash only | Fastest; not a true TUI |

**Recommendation:** **C for Phase 0**, **B for Phase 1** (typed, testable, can call APIs).

#### Phase 2 — TUI revival (optional, local / SSH only)

If a real TUI is still desired for power users:

- Use **ratatui** + **reqwest** against live gateway (`CLAWZ_GATEWAY_URL`).
- Screens: health, agent list, pending approvals, tail audit — mirror Monitoring page data.
- **`run_onboarding`** should write `.env` from `.env.example` with `openssl rand` secrets (match install.sh), not outdated `JWT_SECRET` / port `8080` defaults.
- **Never** run inside distroless container; document `docker compose exec` only if using a debug image variant.

#### Phase 3 — Unified first-run in web (primary UX)

For most users, **web beats TUI**:

1. Post-install redirect to `/login` then **Setup wizard** route (steps: verify health → set API keys → add provider → create first agent via `/agents/onboard`).
2. Drive from `GET /dashboard/overview` for progress checklist.
3. Mobile: keep `POST /system/pairing` or OAuth; link from docs.

This reuses the dashboard you already deploy and avoids SSH.

### 18.8 Onboarding vs build-time (summary)

| Activity | Affects Docker build? | Right tool |
|----------|----------------------|------------|
| Provision VPS + stack | No (pull images) | `install.sh` |
| Register dashboard user | No | Web Login / API |
| Configure providers/channels | No | Config page / REST |
| Create first agent | No | Agents / `POST /agents/onboard` |
| Interactive env wizard | No | Future `clawz setup` or fix TUI + CLI |
| Compile gateway/worker | **Yes** — avoid for routine onboarding | Prebuilt images |

**Onboarding should happen after pull/up, not during `docker compose --build`.**

### 18.9 Backlog additions (TUI / onboarding)

| ID | Task | Phase | Effort |
|----|------|-------|--------|
| O-1 | INSTALL “First run” + install banner URLs | 0 | S |
| O-2 | `deploy.sh doctor` (health + compose + tag check) | 0 | S |
| O-3 | Wire `clawz-gateway setup\|tui\|config` subcommands → `tui::*` | 1 | M |
| O-4 | Fix TUI env names to match `CLAWZ_*` / compose | 1 | S |
| O-5 | TUI dashboard polls `/dashboard/overview` + `/dashboard/metrics` | 2 | M |
| O-6 | Web “Setup wizard” route (post-login) | 2 | M |
| O-7 | Optional `clawz-cli` crate for VPS without Rust | 2 | M |

---

**Maintainers:** Enterpryz Ventures  
**Next review:** After Phase 1 workflow ships; update tag policy and measured deploy times on reference VPS.
