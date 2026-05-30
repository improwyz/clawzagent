# Private container registry for ClawZ

Platform images are published to **GitHub Container Registry (GHCR)** at `ghcr.io/improwyz`.  
**Default install pulls prebuilt images** — no local Rust compile unless you pass `./install.sh --build`.

## One-command install (prebuilt)

```bash
export GITHUB_TOKEN=ghp_xxxxxxxx   # PAT with read:packages — NOT your GitHub password
export GITHUB_USER=your_github_username
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz
cd ~/clawz
./install.sh
```

The installer logs in to `ghcr.io` from `GITHUB_TOKEN` / `CLAWZ_REGISTRY_TOKEN`, then pulls gateway + worker, starts Postgres, runs migrations, and brings up worker + gateway.

**Related install docs:** [INSTALL.md](../INSTALL.md) (full procedures), [README.md](../README.md) (quick start), `clawz setup stack` / web `/setup` for post-install onboarding.

## Images

| Image | Purpose |
|-------|---------|
| `ghcr.io/improwyz/clawz-gateway:<tag>` | HTTP API / gateway |
| `ghcr.io/improwyz/clawz-worker:<tag>` | Worker control plane + fleet orchestration |
| `ghcr.io/improwyz/clawz-agent:<tag>` | Per-tenant agent runtime (spawned by worker) |
| `pgvector/pgvector:pg15` | Postgres (public upstream) |

Default tag: `latest` (set `CLAWZ_IMAGE_TAG=v1.0.0` to pin a release).

## Authenticate

GitHub **does not** accept your account password for `docker login`.

**PAT (recommended):**

1. GitHub → **Settings** → **Developer settings** → **Personal access tokens**
2. Classic token: scope **`read:packages`**  
   Fine-grained: **Packages → Read** on `improwyz` packages
3. Login:

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u "$GITHUB_USER" --password-stdin
```

**GitHub CLI:**

```bash
gh auth login -s read:packages
gh auth token | docker login ghcr.io -u "$(gh api user -q .login)" --password-stdin
```

## Package access

Private packages require **Read** permission for your user or team:

**Packages** → `clawz-gateway` / `clawz-worker` / `clawz-agent` → **Package settings** → **Manage access**

## Images must be published

Pull fails with `manifest unknown` / `not found` when CI has not pushed images yet.

**Maintainers:** push a version tag or run **Actions → Release → Run workflow**:

```bash
git tag v1.0.0 && git push origin v1.0.0
```

Workflow: [`.github/workflows/release.yml`](../.github/workflows/release.yml) builds amd64 + arm64 and tags `latest` + `v*`.

## Manual compose (prebuilt)

```bash
docker login ghcr.io   # or use GITHUB_TOKEN as above
cp .env.example .env
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml pull
docker compose -f docker-compose.yml -f docker-compose.prebuilt.yml up -d
```

## Local build (maintainers only)

Avoids registry; compiles Rust on the host (10–20 minutes):

```bash
./install.sh --build
```

## Environment variables

| Variable | Purpose |
|----------|---------|
| `GITHUB_TOKEN` / `CLAWZ_REGISTRY_TOKEN` | PAT for `docker login` (install reads automatically) |
| `GITHUB_USER` / `CLAWZ_REGISTRY_USER` | GitHub username for login |
| `CLAWZ_REGISTRY` | Default `ghcr.io/improwyz` |
| `CLAWZ_IMAGE_TAG` | Default `latest` |
| `CLAWZ_AGENT_IMAGE` | Agent container image for fleet spawn |

## Digest pinning

```bash
export CLAWZ_IMAGE_TAG=v1.0.0
# or
export CLAWZ_IMAGE_TAG=sha256:...
```
