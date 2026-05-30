# ClawZ Installation Guide (docs copy)

> **Canonical guide:** [INSTALL.md](../INSTALL.md) at the repository root is the primary, maintained installation document. This file summarizes the same procedures for readers browsing `docs/`.

---

## Quick links

| Document | Contents |
|----------|----------|
| [../INSTALL.md](../INSTALL.md) | Full install guide (prerequisites, options, wizard, troubleshooting) |
| [../README.md](../README.md) | Quick start, features, configuration |
| [private-registry.md](private-registry.md) | GHCR login and `GITHUB_TOKEN` |
| [deployment-build-strategy.md](deployment-build-strategy.md) | Updates via `scripts/deploy.sh` (avoid rebuild every pull) |
| [install-onboarding-wizard-tasks.md](install-onboarding-wizard-tasks.md) | Wizard implementation tracker |
| [superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md](superpowers/specs/2026-05-30-docker-bootstrap-compose-deploy-plan.md) | Host bootstrap + Compose architecture |

---

## One-click install

**Linux / macOS**

```bash
export GITHUB_TOKEN=ghp_xxx GITHUB_USER=your_github_username
curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
# or
git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz && ~/clawz/scripts/install.sh
```

**Windows (PowerShell)**

```powershell
git clone --depth 1 https://github.com/improwyz/clawz.git $env:USERPROFILE\clawz
& "$env:USERPROFILE\clawz\scripts\install.ps1" -Docker -Prebuilt
```

---

## Common install paths

| Goal | Command |
|------|---------|
| Full Docker stack (prebuilt) | `./scripts/install.sh` or `.\scripts\install.ps1 -Docker -Prebuilt` |
| Install + wizard | `./scripts/install.sh --wizard` or `.\scripts\install.ps1 -Docker -Prebuilt -Wizard` |
| Host dependencies only | `./scripts/install.sh --bootstrap-only` |
| Local Docker build | `./scripts/install.sh --build` |
| Cargo / no Docker | `./scripts/install.sh --source` |
| CLI stack (from clone) | `clawz setup stack` |
| Web first-run | `http://localhost:3000/setup` |

Default Docker sequence: pull or build images → `db` → `scripts/migrate-db.sh` → `worker` + `gateway` → health check.

---

## Operator CLI

```bash
cargo build -p clawz-cli --release
export PATH="$PWD/target/release:$PATH"

clawz setup deps              # curl, git, Docker (host)
clawz setup stack             # Compose micro stack
clawz onboard                 # TUI wizard
clawz onboard --install-daemon
clawz doctor
```

---

## Verify

```bash
curl http://localhost:3000/health
curl http://localhost:3000/api/v1/system/health
```

For production hardening, wizard API details, and troubleshooting, see **[../INSTALL.md](../INSTALL.md)**.
