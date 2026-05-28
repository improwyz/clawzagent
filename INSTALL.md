# ClawZ Installation Guide

This guide covers one-click installs, Docker Compose, source builds, the optional web dashboard, production hardening, and common troubleshooting. For platform overview and feature docs, see [README.md](README.md).

---

## Prerequisites

| Component | Docker install | Source install |
|-----------|----------------|----------------|
| **Git** | Required (remote one-liner clones the repo) | Required |
| **Docker + Compose v2** | Required (`docker compose`) | Optional |
| **Rust 1.87+** | Not required | Required ([rustup.rs](https://rustup.rs)) |
| **Node.js 20+** | Only with `--with-web` | Only with `--with-web` |
| **curl** | Used for health checks | Used for health checks |

Windows users need **PowerShell 5.1+** for the install script.

---

## Repository layout (install scripts)

Public repo: **[github.com/improwyz/clawz](https://github.com/improwyz/clawz)** — default branch **`main`**.

The installer uses **real paths in the cloned tree** (not `curl` to `.../raw/main/...`, which is only a GitHub download URL prefix and is not a folder in the repo):

```text
improwyz/clawz/
├── install.sh                 # wrapper → scripts/install.sh
├── install.ps1                # wrapper → scripts/install.ps1
├── docker-compose.yml
├── Cargo.toml
├── crates/
└── scripts/
    ├── install.sh             # main Linux/macOS installer
    ├── install-common.sh
    ├── install-deps.sh
    └── install.ps1
```

---

## One-click install (recommended)

The installer clones (or uses) the repo, creates `.env` from [.env.example](.env.example), pulls **prebuilt platform images** from GHCR (or builds locally on failure), starts **gateway + worker + Postgres (pgvector)** in **fleet/micro** mode, and waits for health checks.

**Private images:** authenticate before install with a GitHub **PAT** (not your GitHub password). See [docs/private-registry.md](docs/private-registry.md).

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u YOUR_GITHUB_USERNAME --password-stdin
```

**No registry access?** Build locally instead:

```bash
./install.sh --build
```

### Linux / macOS

**Remote one-liner (curl)** — downloads root `install.sh`, then clones the repo and runs `scripts/install.sh`:

```bash
curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
```

Requires a **public** repo (anonymous HTTP). Custom directory: `CLAWZ_INSTALL_DIR=~/my-clawz curl -fsSL ... | bash`

**Remote one-liner (git)** — use when the repo is private or curl returns 404:

```bash
docker login ghcr.io
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz && ~/clawz/scripts/install.sh
```

**From a clone:**

```bash
./install.sh              # wrapper → scripts/install.sh
./scripts/install.sh
```

### Windows (PowerShell)

**Remote one-liner:**

```powershell
git clone --depth 1 https://github.com/improwyz/clawz.git $env:USERPROFILE\clawz; & "$env:USERPROFILE\clawz\scripts\install.ps1"
```

**From a clone:**

```powershell
.\install.ps1
.\scripts\install.ps1
```

### Installer options

| Linux / macOS | Windows (PowerShell) | Description |
|---------------|----------------------|-------------|
| `--docker` | `-Docker` | Force Docker Compose (default when Docker is running) |
| `--build` | — | Build gateway/worker images locally instead of pulling from registry |
| `--registry R` | — | Image registry (default: `ghcr.io/improwyz`) |
| `--tag TAG` | — | Image tag (default: `latest`) |
| `--source` | `-Source` | Build with `cargo` and run local binaries (no Docker) |
| `--with-web` | `-WithWeb` | Build the React dashboard in `web/` |
| `--dir PATH` | `-InstallDir PATH` | Clone/install location (default: `~/clawz` or `%USERPROFILE%\clawz`) |

**Auto mode:** When neither `--docker` nor `--source` is set, the script uses Docker if the daemon is running; otherwise it falls back to a source build.

### Verify the install

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/v1/system/health
```

Open **http://localhost:3000** in a browser for the gateway API (and dashboard if built).

**Fleet smoke test** (agent containers via worker Docker socket):

```bash
./scripts/fleet-smoke.sh
```

---

## Docker Compose (manual)

If you prefer not to use the installer:

```bash
docker login ghcr.io   # private registry
git clone https://github.com/improwyz/clawz.git
cd clawz
cp .env.example .env
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
curl http://localhost:3000/health
```

Local build (no registry):

```bash
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
```

Services started:

| Service | Port | Role |
|---------|------|------|
| `gateway` | 3000 | HTTP API, WebSocket, MCP |
| `worker` | 50051 | Agent execution |
| `db` | 5432 (internal) | Postgres 15 + pgvector |

**Stop / logs:**

```bash
docker compose down
docker compose logs -f gateway worker
```

---

## Build from source (manual)

For contributors or environments without Docker:

```bash
git clone https://github.com/improwyz/clawz.git
cd clawz
cp .env.example .env

export CLAWZ_MODE=standalone
export CLAWZ_DISABLE_AUTH=1
export CLAWZ_STUB_PROVIDER=1
export CLAWZ_JWT_SECRET=dev-secret
export CLAWZ_WORKER_TOKEN=dev-worker-token

cargo build --release -p clawz-gateway -p clawz-worker

./target/release/clawz-worker &
WORKER_URL=http://127.0.0.1:50051 ./target/release/clawz-gateway
```

Or run `./scripts/install.sh --source` to automate the above.

**Stop source install** (if started by the installer):

```bash
kill $(cat .clawz-gateway.pid .clawz-worker.pid)
```

For Postgres-backed persistence without Docker, set `DATABASE_URL` and ensure pgvector is enabled — see [CONTRIBUTING.md](CONTRIBUTING.md) for development workflows.

---

## Web dashboard (optional)

The React dashboard in `web/` is not started automatically unless you build it.

```bash
./scripts/install.sh --with-web    # Linux / macOS
.\scripts\install.ps1 -WithWeb     # Windows
```

Then preview locally:

```bash
cd web && npm run preview   # http://localhost:4173
```

Configure `web/vite.config.ts` to proxy API and WebSocket traffic to `http://localhost:3000`.

**Branding:** Logo assets live in `web/public/branding/` (silver for dark UI, copper for light). See [design-system.md](crates/clawz-tauri/design/design-system.md).

**Mobile / PWA:** `web/public/manifest.webmanifest` enables add-to-home-screen with ClawZ icons.

---

## Desktop app (Tauri)

The native desktop shell lives in `crates/clawz-tauri/`. Bundle icons are in `crates/clawz-tauri/icons/`; UI branding in `crates/clawz-tauri/src-ui/assets/branding/`.

```bash
cd crates/clawz-tauri
cargo tauri dev    # requires Rust + Tauri system deps
```

Regeneration commands are documented in [design-system.md](crates/clawz-tauri/design/design-system.md).

---

## Environment configuration

Copy [.env.example](.env.example) to `.env` before first run. The installer does this automatically when `.env` is missing and generates random JWT/worker secrets when possible.

Key variables:

| Variable | Purpose |
|----------|---------|
| `CLAWZ_MODE` | `standalone`, `micro`, or `elastic` (Compose defaults to `micro`) |
| `CLAWZ_REGISTRY` / `CLAWZ_IMAGE_TAG` | Prebuilt image coordinates (`ghcr.io/improwyz`, `latest`) |
| `CLAWZ_AGENT_IMAGE` | Image for spawned agent containers |
| `CLAWZ_DOCKER_NETWORK` | Docker network for fleet agents (`clawz-net`) |
| `CLAWZ_MAX_AGENTS` | Max concurrent agent containers per worker |
| `CLAWZ_DISABLE_AUTH` | Dev only — set to `0` or unset in production |
| `CLAWZ_STUB_PROVIDER` | Dev stub LLM — disable when using real providers |
| `VALID_API_KEYS` | Comma-separated API keys (required when auth enabled) |
| `CLAWZ_JWT_SECRET` | JWT signing secret |
| `CLAWZ_WORKER_TOKEN` | Shared secret for gateway ↔ worker |
| `WORKER_URL` | Worker address (`http://worker:50051` in Compose) |
| `DATABASE_URL` | Postgres connection string |
| `CLAWZ_PUBLIC_URL` | Public HTTPS base for webhooks (telephony, channels) |

See [README.md — Configuration](README.md#configuration) for TOML config and `CLAWZ__SECTION__KEY` overrides.

**Private registry:** [docs/private-registry.md](docs/private-registry.md)  
**Dependency audit CSV:** [docs/install-dependencies.csv](docs/install-dependencies.csv) (regenerate with `./scripts/export-install-dependencies.sh`)

---

## Production checklist

Before exposing ClawZ to the internet:

- [ ] Copy `.env.example` → `.env` and **replace all placeholder secrets**
- [ ] Set strong `CLAWZ_JWT_SECRET` and `CLAWZ_WORKER_TOKEN` (installer generates these for local dev only)
- [ ] **Disable** `CLAWZ_DISABLE_AUTH` and configure real `VALID_API_KEYS`
- [ ] **Disable** `CLAWZ_STUB_PROVIDER`; set `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, or your provider keys
- [ ] Use managed Postgres with pgvector; do not expose the Compose `db` port publicly
- [ ] Put TLS termination in front of the gateway (reverse proxy or load balancer)
- [ ] Set `CLAWZ_PUBLIC_URL` to your public HTTPS origin (required for [telephony](TELEPHONY.md))
- [ ] Set `CLAWZ_MODE=micro` or `elastic` for multi-container / mesh deployments
- [ ] Configure backups for Postgres and audit logs
- [ ] Review governance and tenant settings in [README.md — New features setup](README.md#new-features-setup)

---

## Create your first agent

After install:

```bash
curl -X POST http://localhost:3000/api/v1/agents \
  -H "Content-Type: application/json" \
  -d '{
    "name": "hello-agent",
    "model": "stub",
    "system_prompt": "You are a helpful assistant."
  }'
```

With auth enabled, add `-H "Authorization: Bearer <api_key>"`.

---

## Troubleshooting

### Gateway never becomes healthy

```bash
docker compose logs gateway worker db
curl -v http://localhost:3000/health
```

Common causes: Docker daemon not running, port 3000 already in use, or database not ready. Wait up to ~3 minutes on first build.

### Docker not available — source fallback

The installer prints a warning and builds with `cargo`. Ensure Rust 1.87+ is installed. Source mode does not start Postgres by default; set `DATABASE_URL` if you need persistence.

### Port conflicts

Change the host mapping in `docker-compose.yml` or stop the process using port 3000:

```bash
ss -tlnp | grep 3000    # Linux
lsof -i :3000           # macOS
```

### `.env` already exists — secrets unchanged

The installer skips overwriting an existing `.env`. Delete or edit it manually, then re-run or adjust secrets by hand.

### Worker connection errors

Ensure `WORKER_URL` matches where the worker listens and `CLAWZ_WORKER_TOKEN` is identical on gateway and worker.

### Web dashboard cannot reach API

Confirm the gateway is on port 3000 and `web/vite.config.ts` proxy targets match. Run `npm run preview` after `npm run build`.

### Windows: script execution policy

If `install.ps1` is blocked:

```powershell
Set-ExecutionPolicy -Scope CurrentUser RemoteSigned
```

Or run: `powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1`

### Still stuck?

- [CONTRIBUTING.md](CONTRIBUTING.md) — development setup and tests
- [README.md](README.md) — configuration reference and deployment modes
- Open an issue with `docker compose logs` or gateway/worker output

---

## Related docs

- [README.md](README.md) — features, architecture, configuration
- [CONTRIBUTING.md](CONTRIBUTING.md) — building and testing from source
- [TELEPHONY.md](TELEPHONY.md) — Twilio & Google Voice (requires `CLAWZ_PUBLIC_URL`)
- [AGENTS.md](AGENTS.md) — architecture reference for contributors
