#!/usr/bin/env bash
# ClawZ one-click install — Linux & macOS
#
# From a clone:
#   ./scripts/install.sh
#   ./install.sh                    # root wrapper (same result)
#
# Remote one-liner (uses real repo paths — no /raw/main/ download):
#   git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz && ~/clawz/scripts/install.sh
#
# Options:
#   --docker      Use Docker Compose (default when Docker is available)
#   --source      Build and run from source with cargo (no Docker)
#   --with-web    Build the React dashboard in web/
#   --dir PATH    Install/clone location (default: ~/clawz)
#   --help        Show usage

set -euo pipefail

CLAWZ_REPO_URL="${CLAWZ_REPO_URL:-https://github.com/improwyz/clawz.git}"
CLAWZ_BRANCH="${CLAWZ_BRANCH:-main}"
CLAWZ_INSTALL_DIR="${CLAWZ_INSTALL_DIR:-${HOME}/clawz}"

# Honor --dir before we clone or source helpers.
for ((i = 1; i < $#; i++)); do
  if [[ "${!i}" == "--dir" && $((i + 1)) -le $# ]]; then
    CLAWZ_INSTALL_DIR="${!((i + 1))}"
    break
  fi
done

_resolve_script_dir() {
  local candidate=""
  if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "bash" && "${BASH_SOURCE[0]}" != "-" ]]; then
    candidate="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)" || true
  fi
  if [[ -n "$candidate" && -f "${candidate}/install-common.sh" ]]; then
    echo "$candidate"
    return 0
  fi

  if ! command -v git >/dev/null 2>&1; then
    echo "[clawz] Git is required. Install Git: https://git-scm.com/downloads" >&2
    exit 1
  fi

  local repo_root="${CLAWZ_INSTALL_DIR}"
  if [[ ! -f "${repo_root}/scripts/install-common.sh" ]]; then
    if [[ -d "${repo_root}/.git" ]]; then
      git -C "${repo_root}" fetch --depth 1 origin "${CLAWZ_BRANCH}"
      git -C "${repo_root}" checkout "${CLAWZ_BRANCH}"
      git -C "${repo_root}" pull --ff-only origin "${CLAWZ_BRANCH}" || true
    else
      echo "[clawz] Cloning ${CLAWZ_REPO_URL} into ${repo_root} ..."
      git clone --depth 1 --branch "${CLAWZ_BRANCH}" "${CLAWZ_REPO_URL}" "${repo_root}"
    fi
  fi

  if [[ ! -f "${repo_root}/scripts/install-common.sh" ]]; then
    echo "[clawz] Missing ${repo_root}/scripts/install-common.sh after clone." >&2
    exit 1
  fi
  echo "${repo_root}/scripts"
}

SCRIPT_DIR="$(_resolve_script_dir)"
# shellcheck source=install-common.sh
source "${SCRIPT_DIR}/install-common.sh"

MODE="auto"
WITH_WEB=0

usage() {
  sed -n '2,17p' "$0"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --docker) MODE="docker"; shift ;;
    --source) MODE="source"; shift ;;
    --with-web) WITH_WEB=1; shift ;;
    --dir)
      CLAWZ_INSTALL_DIR="$2"
      shift 2
      ;;
    --help|-h) usage ;;
    *)
      err "Unknown option: $1"
      usage
      ;;
  esac
done

OS="$(detect_os)"
if [[ "$OS" == "unsupported" ]]; then
  err "Unsupported OS. Use scripts/install.ps1 on Windows."
  exit 1
fi

log "ClawZ installer — ${OS}"

ensure_git

if ! ensure_repo_root "${SCRIPT_DIR}/.."; then
  clone_or_update_repo
else
  cd "${SCRIPT_DIR}/.."
fi

ROOT="$(pwd)"
log "Using repository at $ROOT"

if [[ "$MODE" == "auto" ]]; then
  if ensure_docker; then
    MODE="docker"
  else
    warn "Docker unavailable — installing Rust and using source build"
    ensure_rust
    MODE="source"
  fi
fi

case "$MODE" in
  docker) ensure_docker; install_with_docker ;;
  source) ensure_rust; install_from_source ;;
  *) err "Invalid mode: $MODE"; exit 1 ;;
esac

if [[ "$WITH_WEB" -eq 1 ]]; then
  ensure_node
  install_web_dashboard
fi

log "Install complete."
