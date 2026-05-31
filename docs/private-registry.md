# Private container registry for ClawZ

ClawZ publishes the same platform images to **two private registries**:

| Priority | Registry | Image path |
|----------|----------|------------|
| **Primary** | GitHub Container Registry (GHCR) | `ghcr.io/improwyz/clawz-{gateway,worker,agent,dashboard}:<tag>` |
| **Fallback** | Docker Hub (private repos) | `docker.io/<namespace>/clawz-{gateway,worker,agent,dashboard}:<tag>` **or** one repo: `docker.io/<namespace>/clawz:gateway-<tag>`, … |

**Default install** pulls from **GHCR** first. If that fails (auth, outage, or missing manifest), the installer automatically retries **Docker Hub** when fallback credentials are configured.

Postgres remains the public image `pgvector/pgvector:pg15`.

---

## One-command install

```bash
# Primary — required for default path
export GITHUB_TOKEN=ghp_xxxxxxxx   # PAT with read:packages
export GITHUB_USER=your_github_username

# Fallback — optional; used only when GHCR pull fails
export DOCKERHUB_USERNAME=your_dockerhub_namespace
export DOCKERHUB_TOKEN=dckr_pat_xxxxxxxx

git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz
cd ~/clawz
./install.sh
```

The installer logs in to GHCR (and Docker Hub if `DOCKERHUB_*` is set), pulls gateway + worker, runs migrations, and starts the stack.

---

## Images

| Image | Purpose |
|-------|---------|
| `ghcr.io/improwyz/clawz-gateway:<tag>` | HTTP API / gateway |
| `ghcr.io/improwyz/clawz-worker:<tag>` | Worker + fleet orchestration |
| `ghcr.io/improwyz/clawz-agent:<tag>` | Per-tenant agent containers |
| `ghcr.io/improwyz/clawz-dashboard:<tag>` | Web UI (`--with-web`) |

Docker Hub mirrors use the same repository names under your namespace, e.g. `docker.io/yourorg/clawz-gateway:<tag>`.

**Tags:** `latest` on release; `main` and `sha-<short>` on every push to `main`.

---

## Authenticate

### GHCR (primary)

GitHub does **not** accept your account password for `docker login`.

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u "$GITHUB_USER" --password-stdin
```

PAT scopes: classic **`read:packages`** or fine-grained **Packages → Read** on `improwyz` packages.

Package access: **Packages** → `clawz-gateway` / `clawz-worker` / etc. → **Manage access**.

### Docker Hub (fallback)

Use an **access token**, not your account password:

```bash
echo "$DOCKERHUB_TOKEN" | docker login -u "$DOCKERHUB_USERNAME" --password-stdin
```

**Warning:** If you `docker push` to a repository that does not exist yet, Docker Hub creates it as **public** by default. Always create private repos first (UI or `./scripts/publish-dockerhub.sh`), and use a token with **Read + Write + Delete** so the publish script can enforce `is_private: true` via the Hub API. Read-only tokens can log in and push but will leak images publicly.

Create **private** repositories on hub.docker.com (or let `publish-dockerhub.sh` create them):

- **Four repos:** `clawz-gateway`, `clawz-worker`, `clawz-agent`, `clawz-dashboard`
- **One repo (monorepo):** `clawz` with component tags `gateway-latest`, `worker-latest`, `agent-latest`, `dashboard-latest`

Monorepo install/publish:

```bash
export CLAWZ_HUB_MONOREPO=clawz
export CLAWZ_REGISTRY_FALLBACK=docker.io/your_namespace
CLAWZ_HUB_MONOREPO=clawz ./scripts/publish-dockerhub.sh
```

Verify privacy before sharing credentials:

```bash
export DOCKERHUB_USERNAME=your_namespace DOCKERHUB_TOKEN=dckr_pat_xxxx
SKIP_BUILD=1 ./scripts/publish-dockerhub.sh   # creates/patches repos only; exits if not private
```

---

## Maintainer CI setup

Workflows push to **both** registries on each build when secrets are present:

- [`.github/workflows/docker-publish-main.yml`](../.github/workflows/docker-publish-main.yml) — `main` branch
- [`.github/workflows/release.yml`](../.github/workflows/release.yml) — `v*` tags

| Secret | Purpose |
|--------|---------|
| `GITHUB_TOKEN` | Provided by Actions for GHCR push |
| `DOCKERHUB_USERNAME` | Docker Hub namespace (optional but enables fallback mirror) |
| `DOCKERHUB_TOKEN` | Hub token with Read/Write for CI push |

Release:

```bash
git tag v1.0.2 && git push clawz v1.0.2
```

### Manual publish to Docker Hub (private)

From a maintainer machine with a valid Hub access token:

```bash
export DOCKERHUB_USERNAME=sajav
export DOCKERHUB_TOKEN=dckr_pat_xxxx
./scripts/publish-dockerhub.sh
```

The script **refuses to push** until all four repositories exist and Hub API reports `is_private: true`. It creates private repos when the token has write scope, patches public repos to private, then builds and pushes with tags `main` and `latest` (override with `CLAWZ_IMAGE_TAG`).

---

## Environment variables

| Variable | Purpose |
|----------|---------|
| `GITHUB_TOKEN` / `GHCR_TOKEN` | PAT for GHCR login (`read:packages`) |
| `GITHUB_USER` / `CLAWZ_REGISTRY_USER` | GitHub username for `docker login ghcr.io` |
| `DOCKERHUB_USERNAME` | Hub namespace for fallback |
| `DOCKERHUB_TOKEN` | Hub access token for fallback |
| `CLAWZ_REGISTRY` | Primary registry path (default `ghcr.io/improwyz`) |
| `CLAWZ_REGISTRY_FALLBACK` | Explicit fallback (default `docker.io/$DOCKERHUB_USERNAME` when set). Use `docker.io/$USER/clawz` to auto-enable monorepo |
| `CLAWZ_HUB_MONOREPO` | Hub repo name for single-repo layout (e.g. `clawz` → `clawz:gateway-latest`) |
| `CLAWZ_IMAGE_TAG` | `latest`, `main`, `v1.0.2`, or `sha-…` |
| `CLAWZ_AGENT_IMAGE` | Set automatically after successful pull |

Force fallback only (skip GHCR attempt):

```bash
export CLAWZ_REGISTRY=docker.io/your_namespace
./install.sh
```

---

## Manual compose

```bash
export CLAWZ_REGISTRY=ghcr.io/improwyz
export CLAWZ_IMAGE_TAG=latest
docker login ghcr.io
cp .env.example .env
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
```

---

## Local build (no registry)

```bash
./install.sh --build
```

---

## Troubleshooting

| Symptom | Action |
|---------|--------|
| GHCR `denied` / `401` | Fix `GITHUB_TOKEN` scopes; grant package Read |
| GHCR `manifest unknown` | Wait for CI or tag a release; or rely on Hub fallback |
| Fallback not attempted | Set `DOCKERHUB_USERNAME` + `DOCKERHUB_TOKEN` |
| Images appeared on public Hub repos | Token lacked repo write scope; `docker push` auto-created **public** repos. Delete them, create **private** repos (UI or new token + `SKIP_BUILD=1 ./scripts/publish-dockerhub.sh`), then publish again |
| Hub `pull access denied` | `docker login`; private repo + collaborator access |
| Both fail | `./install.sh --build` |
