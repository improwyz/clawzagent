# ClawZ Installation Guide

This guide covers the interactive TUI installer, shell bootstrap, Docker Compose, source builds, licensing, production hardening, and troubleshooting. For platform overview, see [README.md](README.md).

---

## Recommended: Interactive TUI Installer

The `clawz` binary is a unified installer and operator CLI. Run it with no arguments to launch the interactive TUI:

```bash
./clawz
```

The TUI installer has three screens:

1. **Splash** — branded logo, auto-detects your system (OS, RAM, Docker, ports)
2. **LLM Provider Setup** — select a provider and enter your API key (validated inline)
3. **AI-Guided Chat** — a local setup assistant walks you through the entire deployment conversationally

The setup assistant will:
- Detect your system and recommend Docker (prebuilt) or source build
- Auto-generate all secrets (JWT, worker token, encryption key)
- Execute `docker compose up` or `cargo build` with live progress in the chat
- Configure your agent identity
- Run health checks and show your dashboard URL
- Transition to the live ClawZ agent once the gateway is healthy

### Getting the `clawz` Binary

**Option A — Download prebuilt binary (fastest, no Rust required):**

```bash
# Linux amd64
curl -fsSL https://github.com/improwyz/clawzagent/releases/latest/download/clawz-linux-amd64 -o clawz
chmod +x clawz && ./clawz
```

**Option B — Shell bootstrap (installs Docker/Rust if needed):**

```bash
curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
```

**Option C — Build from source:**

```bash
git clone https://github.com/improwyz/clawzagent.git && cd clawzagent
cargo build -p clawz-cli --release
./target/release/clawz
```

### Headless / CI Mode

For non-interactive environments, use `--headless`:

```bash
./clawz --headless
```

This runs the same setup flow in plain text mode.

---

## Supported LLM Providers

The TUI installer supports 12+ providers out of the box. You need at least one API key to power the setup assistant:

| Provider | Env Variable | Default Endpoint |
|----------|-------------|-----------------|
| Anthropic (Claude) | `ANTHROPIC_API_KEY` | `https://api.anthropic.com/v1` |
| OpenAI | `OPENAI_API_KEY` | `https://api.openai.com/v1` |
| OpenRouter | `OPENROUTER_API_KEY` | `https://openrouter.ai/api/v1` |
| Groq | `GROQ_API_KEY` | `https://api.groq.com/openai/v1` |
| Grok / xAI | `XAI_API_KEY` | `https://api.x.ai/v1` |
| DeepSeek | `DEEPSEEK_API_KEY` | `https://api.deepseek.com/v1` |
| Ollama (local) | — | `http://localhost:11434` |
| Together AI | `TOGETHER_API_KEY` | `https://api.together.xyz/v1` |
| Fireworks AI | `FIREWORKS_API_KEY` | `https://api.fireworks.ai/inference/v1` |
| Google Gemini | `GEMINI_API_KEY` | `https://generativelanguage.googleapis.com/v1beta` |
| AWS Bedrock | `AWS_ACCESS_KEY_ID` | Regional endpoint |
| Custom | `CLAWZ_CUSTOM_LLM_KEY` + `CLAWZ_CUSTOM_LLM_BASE` | Any OpenAI-compatible URL |

All providers with a `_API_BASE` variant (e.g. `OPENAI_API_BASE`) support custom endpoints for proxies and self-hosted models.

---

## HTTPS (Caddy Reverse Proxy)

ClawZ includes **Caddy** in its Docker Compose stack for automatic HTTPS on port 443:

- Set `CLAWZ_DOMAIN` in `.env` to your hostname, public IP, or domain
- **localhost** → Caddy uses a self-signed certificate (zero config)
- **Public domain** → Caddy auto-provisions a Let's Encrypt certificate
- The gateway does not expose port 3000 externally — Caddy is the only entry point

The TUI installer auto-detects your public IP/hostname and sets `CLAWZ_DOMAIN` for you.

---

## Licensing

### 30-Day Free Trial

ClawZ starts with a **30-day free trial** — full functionality, no license key required. The trial is per-machine and begins on first run.

### Activation

Purchase a license key at **[clawz.net](https://clawz.net)**. Keys are available for 1-year terms.

Enter your key during the TUI installer, or later in the web dashboard (Settings → License). Keys are hardware-bound (tied to your machine's fingerprint) and support up to 2 machines per key.

### License Behavior

| State | What happens |
|-------|-------------|
| Trial (days 1–30) | Full access, no restrictions |
| Trial expired | Gateway refuses to start. Run `clawz` to enter a license key. |
| Paid license active | Full access. Re-validated against clawz.net every 7 days. |
| Paid license expired | 7-day grace period with dashboard warnings, then hard stop. |
| Offline > 30 days | Must reconnect to clawz.net to validate, then resumes. |
| Clock manipulation | Detected and blocked (uses monotonic day counter). |

License state is stored at `~/.clawz/license.json`.

---

## Prerequisites

| Component | Docker install | Source install |
|-----------|----------------|----------------|
| **Docker + Compose v2** | Required | Not required |
| **Rust 1.87+** | Not required | Required ([rustup.rs](https://rustup.rs)) |
| **curl** | Required (healthchecks, Caddy) | Required |
| **Git** | Required if cloning | Required |

The TUI installer checks prerequisites automatically and offers to install missing ones.

---

## Operator CLI

The same `clawz` binary serves as both the installer and operator CLI:

| Command | Purpose |
|---------|---------|
| `clawz` | Launch TUI installer (first run) or setup assistant |
| `clawz --headless` | Plain text setup for CI/non-TTY |
| `clawz doctor` | Health check gateway, worker, env, Docker |
| `clawz gateway start/stop/status` | Docker Compose control |
| `clawz agent --message "..."` | Send a message to an agent |
| `clawz cron list/add/run/remove` | Scheduled agent jobs |
| `clawz setup deps` | Install host prerequisites |
| `clawz setup stack [--build]` | Docker Compose up (prebuilt or local build) |

---

## One-click install (recommended)

The installer clones (or uses) the repo, creates `.env` from [.env.example](.env.example), pulls **prebuilt platform images** from GHCR (or builds locally on failure), starts **gateway + worker + Postgres (pgvector)** in **fleet/micro** mode, and waits for health checks.

**Prebuilt images (default):** set a GitHub **PAT** (`read:packages`) and username — not your GitHub password. See [docs/private-registry.md](docs/private-registry.md).

```bash
export GITHUB_TOKEN=ghp_xxxxxxxx
export GITHUB_USER=your_github_username
export DOCKERHUB_USERNAME=your_namespace
export DOCKERHUB_TOKEN=dckr_pat_xxxxxxxx
```

**Maintainers only** (slow local compile): `./install.sh --build`

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
| `--build` | `-Build` | Build gateway/worker images locally instead of pulling from registry |
| `--bootstrap-only` | `-BootstrapOnly` | Install curl, git, Docker (and Node with `--with-web`) only — no stack |
| `--wizard` | `-Wizard` | After install, run `clawz onboard --install-daemon` (or `cargo run -p clawz-cli -- …`) |
| `--registry R` | `-Registry` | Image registry (default: `ghcr.io/improwyz`) |
| `--tag TAG` | `-Tag` | Image tag (default: `latest`) |
| `--source` | `-Source` | Build with `cargo` and run local binaries (no Docker) |
| `--with-web` | `-WithWeb` | Build the React dashboard in `web/` |
| `--dir PATH` | `-InstallDir PATH` | Clone/install location (default: `~/clawz` or `%USERPROFILE%\clawz`) |
| — | `-Prebuilt` | Pull prebuilt GHCR images (default when not using `-Build`) |
| — | `-InstallDocker` | Attempt Docker Desktop install via **winget**, then re-run |

**Auto mode:** When neither `--docker` nor `--source` is set, the script uses Docker if the daemon is running; otherwise it falls back to a source build.

**Examples**

```bash
# Greenfield Linux server (prebuilt + wizard)
export GITHUB_TOKEN=ghp_xxx GITHUB_USER=you
curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash -s -- --docker --wizard

# Clone: deps only, then stack later via CLI
./scripts/install.sh --bootstrap-only
clawz setup stack

# Windows: prebuilt stack + web dashboard
.\scripts\install.ps1 -Docker -Prebuilt -WithWeb
```

### Verify the install

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/v1/system/health
```

Open **http://localhost:3000** in a browser for the gateway API (and dashboard if built).

## Operator CLI (`clawz`)

Build from the repo (or `cargo install --path crates/clawz-cli`):

```bash
cargo build -p clawz-cli --release
export PATH="$PWD/target/release:$PATH"
```

| Command | Purpose |
|---------|---------|
| `clawz onboard` | Interactive first-run wizard (Linux TUI primary); see [install wizard design](docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md) |
| `clawz onboard --install-daemon` | Same, then `clawz setup stack` (deps + Compose up) |
| `clawz setup deps` | Install host prerequisites (curl, git, Docker) via `install-deps.sh` |
| `clawz setup stack` | Prebuilt GHCR pull + `db` → migrate → `worker` + `gateway` |
| `clawz setup stack --build` | Local image build overlay instead of GHCR pull |
| `./scripts/install.sh --wizard` | Install + `clawz onboard --install-daemon` |
| `./scripts/install.sh --bootstrap-only` | Host deps only (no Compose stack) |
| `.\scripts\install.ps1 -Docker -Prebuilt` | Windows: prebuilt pull + migrate + stack |
| `.\scripts\install.ps1 -InstallDocker` | Attempt Docker Desktop install via winget |
| `clawz doctor` | Gateway, worker, env, and Docker checks |
| `clawz gateway status` | Health for gateway + worker |
| `clawz gateway start` / `stop` | Docker Compose in current repo (or `CLAWZ_REPO`) |
| `clawz agent -m "Hello"` | One agent turn via `POST /api/v1/agents/{id}/run` |
| `clawz cron list` | List scheduled jobs |
| `clawz cron add "0 9 * * *" "Daily summary"` | Create a cron job (uses first agent if `--agent-id` omitted) |
| `clawz cron run <job-id>` | Run a job immediately |
| `clawz cron remove <job-id>` | Delete a job |
| `clawz setup` / `clawz tui config` | Interactive config menu |

Set `CLAWZ_API_KEY` or `VALID_API_KEYS` (first key used) and `CLAWZ_GATEWAY_URL` (default `http://127.0.0.1:3000`).

### First-run onboarding wizard

A unified setup flow guides deployment mode, Docker prebuilt vs build, **stack bootstrap**, agent identity, skills, LLM keys (including OAuth), and verification.

| Platform | Primary entry | Stack step |
|----------|---------------|------------|
| **Linux** | `clawz onboard` (TUI) or web `/setup` | `clawz setup stack` / setup API |
| **Windows / macOS** | Web **http://localhost:3000/setup** | Host install command if gateway is in Docker |
| **All** | `./scripts/install.sh --wizard` | Runs onboard + `setup stack` after install |

**Web wizard (`/setup`):**

1. `GET /api/v1/setup/status` — progress + bootstrap token (`X-Clawz-Setup-Token`).
2. Phases: deploy mode → install strategy → **stack** → secrets → LLM OAuth → identity → verify → complete.
3. **Stack step:** `POST /api/v1/setup/stack` with `{ "action": "deps" }` then `{ "action": "up", "install_strategy": "prebuilt" }` when `host_exec_allowed` is true.
4. If the gateway runs inside Compose (`/.dockerenv`), the UI shows a platform-specific `install.sh` / `install.ps1` one-liner instead.

**Setup API (stack):**

```bash
TOKEN="<from GET /setup/status bootstrap_token>"
curl -s http://127.0.0.1:3000/api/v1/setup/stack/status
curl -s -X POST http://127.0.0.1:3000/api/v1/setup/stack \
  -H "X-Clawz-Setup-Token: $TOKEN" -H "Content-Type: application/json" \
  -d '{"action":"deps","dry_run":true,"confirm":"yes-install"}'
```

**Docs**

- Design: [docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md](docs/superpowers/specs/2026-05-30-install-onboarding-wizard-design.md)
- Docker bootstrap: [docs/superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md](docs/superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md)
- Task tracker: [docs/install-onboarding-wizard-tasks.md](docs/install-onboarding-wizard-tasks.md)

## Channels and webhooks

Register a channel via `POST /api/v1/channels` with `channel_type` (e.g. `webhook`, `slack`, `twilio`) and `config.agent_id`. Inbound webhooks:

- Generic: `POST /webhooks/{channel_type}/{channel_id}`
- Twilio SMS/voice: `POST /webhooks/twilio/sms/{id}`, `POST /webhooks/twilio/voice/{id}`

For polling adapters, set `config.poll_interval_secs` (seconds). The gateway supervisor polls enabled channels automatically (disable with `CLAWZ_CHANNEL_SUPERVISOR=0`).

**DM pairing** (optional): set `CLAWZ_CHANNEL_PAIRING=1` or `config.require_pairing: true`, then:

```bash
curl -X POST -H "Authorization: Bearer $CLAWZ_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"channel_id":"<uuid>"}' \
  http://127.0.0.1:3000/api/v1/channels/pairing
# Approve with pairing_code + peer_id:
curl -X POST .../api/v1/channels/pairing/approve -d '{"code":"ABC12345","peer_id":"user-1"}'
```

Allowlist file: `~/.clawz/channel-pairing.json`. Systemd unit templates: `scripts/systemd/`.

## Cron (scheduled agents)

Jobs are stored at `~/.clawz/cron/jobs.json` (override with `CLAWZ_CRON_JOBS_FILE`). The worker scheduler ticks every 60 seconds (`CLAWZ_CRON_TICK_SECS`, default `60`). Disable with `CLAWZ_CRON_SCHEDULER=0`.

```bash
clawz cron add "0 9 * * *" "Summarize overnight email" --agent-id my-agent
clawz cron list
clawz cron run <job-id>
```

REST API (gateway): `GET/POST /api/v1/cron/jobs`, `DELETE /api/v1/cron/jobs/{id}`, `POST /api/v1/cron/jobs/{id}/run`. Cron runs use `cron_mode` (interactive tools disabled).

Dashboard: **http://localhost:3000/cron** (when `web/dist` is built).

## Background ingest (connectors + subconscious)

**Connector sync** (default every 20 minutes, `CLAWZ_CONNECTOR_SYNC_INTERVAL_SECS`):

- Disable with `CLAWZ_CONNECTOR_SYNC=0`
- Config: `~/.clawz/connector-sync.json` (or auto when `GITHUB_TOKEN` / `CLAWZ_CONNECTOR_GITHUB_TOKEN` is set)
- Ingests SaaS objects into agent memory as `sync:*` FTS chunks

**Subconscious reflection** (opt-in):

```bash
export CLAWZ_SUBCONSCIOUS=1
export CLAWZ_SUBCONSCIOUS_AGENT_ID=my-agent   # optional, default "default"
```

Ticks every 30 minutes (`CLAWZ_SUBCONSCIOUS_INTERVAL_SECS`). Manual trigger: `POST /api/v1/background/subconscious`.

## Terminal backends (shell / file tools)

`shell` and `file_ops` run through a pluggable terminal backend:

| Backend | When | Env |
|---------|------|-----|
| `local` | `CLAWZ_MODE=standalone` (default) | `CLAWZ_TERMINAL_BACKEND=local` |
| `docker` | `micro` / `elastic` (default) | `CLAWZ_TERMINAL_BACKEND=docker`, `CLAWZ_TERMINAL_DOCKER_IMAGE` |
| `ssh` | Remote host | `CLAWZ_TERMINAL_BACKEND=ssh`, `CLAWZ_SSH_HOST`, `CLAWZ_SSH_USER` |

Workspace directory: `CLAWZ_TERMINAL_WORKDIR` or `~/.clawz/workspace` (mounted at `/workspace` in Docker).

## Local memory (standalone)

In `CLAWZ_MODE=standalone` (default when `DATABASE_URL` is unset), conversation and agent memory persist to SQLite at `~/.clawz/memory.db`. Override with `CLAWZ_MEMORY_DB` or force SQLite in other modes with `CLAWZ_SQLITE_MEMORY=1`.

## Self-improvement and learning loop

Normal agent turns (not cron/background) attach outcome tracking, identity, skills, and a periodic self-improvement loop:

| Env | Default | Purpose |
|-----|---------|---------|
| `CLAWZ_SELF_IMPROVEMENT` | on | Set `0` to disable learning stack on runtimes |
| `CLAWZ_SELF_IMPROVEMENT_INTERVAL_TURNS` | `5` | Run improvement loop every N pipeline turns |
| `CLAWZ_MEMORY_NUDGE` | on | Post-turn RAG ingest of assistant text (needs embedder) |
| `CLAWZ_OLLAMA_EMBED` | off | Use Ollama for embeddings (`OLLAMA_HOST`, `CLAWZ_EMBED_MODEL`) |

On-disk paths under `~/.clawz/`: `identities/`, `profiles/`, `transcript_fts.db`. Cross-session transcript search is indexed after each chat turn.

Postgres (`DATABASE_URL`) is used when set (micro/elastic deployments).

## Operator workspace (skills)

Seed `AGENTS.md` and an example skill under `~/.clawz/workspace` (or set `CLAWZ_WORKSPACE`):

```bash
./scripts/init-workspace.sh
```

The worker merges `AGENTS.md`, optional `SOUL.md`, and `skills/*/SKILL.md` into the system prompt on each turn. List skills via the gateway:

```bash
curl -s -H "Authorization: Bearer $CLAWZ_API_KEY" http://127.0.0.1:3000/api/v1/skills
```

## First run (after install)

**Routine VPS updates:** do not use `docker compose ... --build` on every `git pull` — that recompiles the full Rust workspace and can take 10–20+ minutes. Use **`./scripts/deploy.sh`** instead (pull prebuilt images or rebuild only what changed). See [docs/deployment-build-strategy.md](docs/deployment-build-strategy.md) for fast vs slow paths.

1. **Health** — confirm the gateway is up:
   ```bash
   curl -sf http://localhost:3000/health
   curl -sf http://localhost:3000/api/v1/system/health
   ```
2. **Web dashboard (optional)** — if you installed with `--with-web` or built `web/dist`, serve the UI on the VPS:
   ```bash
   ./scripts/serve-web-dashboard.sh
   ```
   Open **http://your-server:4173** (replace with your VPS hostname or IP; override port with `CLAWZ_WEB_PORT`). Log in when auth is enabled; with dev `.env`, `CLAWZ_DISABLE_AUTH=1` skips login.
3. **Operator UI** — use the **Monitoring** page in the sidebar for fleet health, metrics, and alerts. Configure API keys and production settings under **Config** after disabling dev auth.

**Gateway TUI:** the in-binary terminal UI is **development-only** and is not wired to production compose or auth. For installs and day-two ops on a VPS, use **`./scripts/install.sh`** (first boot) and the **web dashboard** (ongoing).

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

### Mobile scaffold (experimental)

`crates/clawz-tauri-mobile/` mirrors the desktop split (separate `tauri.conf.json`, shared `web/` build). It is **not** in the workspace `members` list so default `cargo build --workspace` stays desktop/server-only.

```bash
./scripts/build-mobile.sh   # builds web/, then `cargo tauri android/ios build` when SDKs exist
```

Requires [Tauri mobile prerequisites](https://v2.tauri.app/start/prerequisites/) (Android SDK, Xcode for iOS). Voice, CEF, and full mobile UX are deferred — see [agent-runtime-parity-plan.md](docs/agent-runtime-parity-plan.md) Phase DD.

---

## Database migrations

The gateway applies **embedded** core schema on startup (`clawz-core::db::run_migrations`). Additional SQL files under [`migrations/`](migrations/) (sessions, cron jobs, etc.) must be applied with the helper script.

**Docker Compose (default after `./scripts/install.sh`):** migrations run automatically once Postgres is healthy.

**Manual / CI:**

```bash
# All migrations/*.sql in order (auto-detects compose service db or DATABASE_URL)
./scripts/migrate-db.sh

# One file
./scripts/migrate-db.sh 008_sessions.sql

# Another compose project name
COMPOSE="docker compose -p infrastructure" ./scripts/migrate-db.sh

# Bare Postgres on the host
export DATABASE_URL="postgresql://postgres:password@127.0.0.1:5432/clawz"
./scripts/migrate-db.sh
```

Requires `psql` on the host for `DATABASE_URL` mode, or Docker for compose/container mode. Idempotent migrations use `IF NOT EXISTS` where possible.

---

## Environment configuration

Copy [.env.example](.env.example) to `.env` before first run. The installer does this automatically when `.env` is missing and generates random JWT/worker secrets when possible.

Key variables:

| Variable | Purpose |
|----------|---------|
| `CLAWZ_MODE` | `standalone`, `micro`, or `elastic` (Compose defaults to `micro`) |
| `CLAWZ_DOMAIN` | Hostname/IP for Caddy HTTPS (auto-detected by TUI installer) |
| `CLAWZ_PUBLIC_URL` | Public HTTPS base for webhooks — derived from `CLAWZ_DOMAIN` |
| `CLAWZ_REGISTRY` / `CLAWZ_IMAGE_TAG` | Prebuilt image coordinates (`ghcr.io/improwyz`, `latest`) |
| `CLAWZ_AGENT_IMAGE` | Image for spawned agent containers |
| `CLAWZ_DOCKER_NETWORK` | Docker network for fleet agents (`clawz-net`) |
| `CLAWZ_MAX_AGENTS` | Max concurrent agent containers per worker |
| `CLAWZ_DISABLE_AUTH` | Dev only — must be `0` in release builds |
| `CLAWZ_STUB_PROVIDER` | Dev stub LLM — disable when using real providers |
| `VALID_API_KEYS` | Comma-separated API keys (required when auth enabled) |
| `CLAWZ_JWT_SECRET` | JWT signing secret (auto-generated by installer) |
| `CLAWZ_WORKER_TOKEN` | Shared secret for gateway ↔ worker (auto-generated) |
| `CLAWZ_SECRETS_KEY` | Encryption key for secrets at rest (auto-generated) |
| `POSTGRES_PASSWORD` | Database password (used by Compose `db` service) |
| `WORKER_URL` | Worker address (`http://worker:50051` in Compose) |
| `DATABASE_URL` | Postgres connection string |

**LLM provider keys** (set at least one):

| Variable | Provider |
|----------|---------|
| `ANTHROPIC_API_KEY` | Anthropic (Claude) |
| `OPENAI_API_KEY` / `OPENAI_API_BASE` | OpenAI (or compatible proxy) |
| `OPENROUTER_API_KEY` | OpenRouter |
| `GROQ_API_KEY` | Groq |
| `XAI_API_KEY` | Grok / xAI |
| `DEEPSEEK_API_KEY` | DeepSeek |
| `TOGETHER_API_KEY` | Together AI |
| `FIREWORKS_API_KEY` | Fireworks AI |
| `CLAWZ_CUSTOM_LLM_KEY` + `CLAWZ_CUSTOM_LLM_BASE` | Any OpenAI-compatible |

See [README.md — Configuration](README.md#configuration) for TOML config and `CLAWZ__SECTION__KEY` overrides.

---

## Production checklist

Before exposing ClawZ to the internet:

- [ ] Run the TUI installer (`./clawz`) — it generates all secrets automatically
- [ ] Set `CLAWZ_DOMAIN` to your public hostname or IP (Caddy handles TLS)
- [ ] Verify `CLAWZ_DISABLE_AUTH=0` in `.env` (release builds enforce this)
- [ ] Set at least one LLM provider key (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, etc.)
- [ ] Disable `CLAWZ_STUB_PROVIDER` in production
- [ ] Configure real `VALID_API_KEYS` for API access
- [ ] Run `./scripts/migrate-db.sh` after upgrading
- [ ] Use managed Postgres with pgvector; do not expose the Compose `db` port publicly
- [ ] Activate a license key at [clawz.net](https://clawz.net) before the 30-day trial expires
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
- [docs/user-manual.md](docs/user-manual.md) — end-user manual (23 sections)
- [docs/administration-manual.md](docs/administration-manual.md) — operations, security, governance
- [docs/api-reference.md](docs/api-reference.md) — REST API with code samples
- [CONTRIBUTING.md](CONTRIBUTING.md) — building and testing from source
- [TELEPHONY.md](TELEPHONY.md) — Twilio & Google Voice (requires `CLAWZ_PUBLIC_URL`)
- [AGENTS.md](AGENTS.md) — architecture reference for contributors
