# Private container registry for ClawZ

Platform images are published to **GitHub Container Registry (GHCR)** and should be **private** for production. Credentials are never stored in this repository.

## Images

| Image | Purpose |
|-------|---------|
| `ghcr.io/improwyz/clawz-gateway:<tag>` | HTTP API / gateway |
| `ghcr.io/improwyz/clawz-worker:<tag>` | Worker control plane + fleet orchestration |
| `ghcr.io/improwyz/clawz-agent:<tag>` | Per-tenant agent runtime (spawned by worker) |
| `pgvector/pgvector:pg15` | Postgres (public upstream) |

## Authenticate before install

GitHub **does not** accept your account password for `docker login`. Use a **Personal Access Token (PAT)** or the GitHub CLI.

**PAT (recommended):**

1. GitHub → **Settings** → **Developer settings** → **Personal access tokens**
2. Create a token with **`read:packages`** (classic) or **Packages: Read** (fine-grained)
3. Log in:

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u YOUR_GITHUB_USERNAME --password-stdin
```

Use your GitHub **username** (not email). Paste the **token** as the password.

**GitHub CLI:**

```bash
gh auth login
gh auth token | docker login ghcr.io -u YOUR_GITHUB_USERNAME --password-stdin
```

If pull still fails (no package access or images not published yet), install without the registry:

```bash
./install.sh --build
```

## Install with prebuilt images

Only attempted when you are logged into `ghcr.io` or set `CLAWZ_PREBUILT=1`.

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u YOUR_GITHUB_USERNAME --password-stdin
export CLAWZ_PREBUILT=1
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz
cd ~/clawz
cp .env.example .env
./install.sh
```

Without login, `./install.sh` **builds from source automatically** (no registry needed).

Force local build (no GHCR login required):

```bash
./install.sh --build
```

This builds `gateway` and `worker` from `Dockerfile.gateway` / `Dockerfile.worker` on your machine (first run can take 10–20 minutes).

Custom registry/tag:

```bash
CLAWZ_REGISTRY=ghcr.io/myorg CLAWZ_IMAGE_TAG=v1.0.0 ./install.sh
```

## Private agent image pulls (fleet mode)

The worker spawns `clawz-agent` containers via the host Docker API. The worker container must be able to pull your private agent image.

Options:

1. **Mount Docker credentials** (development only):

   ```yaml
   worker:
     volumes:
       - /var/run/docker.sock:/var/run/docker.sock
       - ${HOME}/.docker/config.json:/root/.docker/config.json:ro
   ```

2. **Robot account + `docker login` on the host** before starting compose (worker uses host daemon via socket).

3. **Public agent image** for demos only (not recommended for production).

## Digest pinning

Prefer immutable deploys:

```bash
export CLAWZ_IMAGE_TAG=sha256:abc123...
```

Or pin in compose:

```yaml
image: ghcr.io/improwyz/clawz-gateway@sha256:...
```

## Package visibility

In GitHub: **Packages** → select package → **Package settings** → change visibility to **Private**.
