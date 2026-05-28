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

## GitHub repo layout vs install URLs

Public repo: **[github.com/improwyz/clawz](https://github.com/improwyz/clawz)** — default branch **`main`**.

`/raw/main/` is **not a folder in the repository**. It is GitHub’s URL pattern to download file contents:

```text
https://github.com/{owner}/{repo}/raw/{branch}/{path/in/repo}
```

| Path in repo (on `main`) | Purpose | Raw download URL |
|--------------------------|---------|------------------|
| `scripts/install.sh` | Linux/macOS one-click installer (entry point) | `https://github.com/improwyz/clawz/raw/main/scripts/install.sh` |
| `scripts/install-common.sh` | Shared install helpers (loaded by `install.sh`) | `.../raw/main/scripts/install-common.sh` |
| `scripts/install-deps.sh` | Auto-install Docker, Rust, Git, etc. | `.../raw/main/scripts/install-deps.sh` |
| `scripts/install.ps1` | Windows installer | `.../raw/main/scripts/install.ps1` |
| `install.sh` (repo root) | Optional wrapper → `scripts/install.sh` | `.../raw/main/install.sh` (only if present in repo) |
| `docker-compose.yml` | Docker stack | clone repo; not used in curl one-liner |
| `Cargo.toml`, `crates/` | Source build | clone repo |

**Do not use** `https://github.com/.../main/scripts/...` (missing `/raw/`) — that returns an HTML page, not the script.  
**Do not use** `raw.githubusercontent.com/...` if your environment blocks that host; the `github.com/.../raw/...` URLs serve the same bytes.

The remote one-liner fetches `install.sh` first, then downloads `install-common.sh` and `install-deps.sh` from the same `scripts/` prefix. All three files must exist on `main` for the pipe-to-bash flow to work.

---

## One-click install (recommended)

The installer clones (or uses) the repo, creates `.env` from [.env.example](.env.example), starts **gateway + worker + Postgres (pgvector)**, and waits for health checks.

### Linux / macOS

**Remote one-liner:**

```bash
curl -fsSL https://github.com/improwyz/clawz/raw/main/scripts/install.sh | bash
```

**From a clone:**

```bash
./install.sh              # wrapper → scripts/install.sh
./scripts/install.sh
```

### Windows (PowerShell)

**Remote one-liner** (after `scripts/install.ps1` is on `main`):

```powershell
irm https://github.com/improwyz/clawz/raw/main/scripts/install.ps1 | iex
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

---

## Docker Compose (manual)

If you prefer not to use the installer:

```bash
git clone https://github.com/improwyz/clawz.git
cd clawz
cp .env.example .env
docker compose up -d --build
curl http://localhost:3000/health
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
| `CLAWZ_MODE` | `standalone`, `micro`, or `elastic` |
| `CLAWZ_DISABLE_AUTH` | Dev only — set to `0` or unset in production |
| `CLAWZ_STUB_PROVIDER` | Dev stub LLM — disable when using real providers |
| `VALID_API_KEYS` | Comma-separated API keys (required when auth enabled) |
| `CLAWZ_JWT_SECRET` | JWT signing secret |
| `CLAWZ_WORKER_TOKEN` | Shared secret for gateway ↔ worker |
| `WORKER_URL` | Worker address (`http://worker:50051` in Compose) |
| `DATABASE_URL` | Postgres connection string |
| `CLAWZ_PUBLIC_URL` | Public HTTPS base for webhooks (telephony, channels) |

See [README.md — Configuration](README.md#configuration) for TOML config and `CLAWZ__SECTION__KEY` overrides.

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
