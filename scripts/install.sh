#!/usr/bin/env bash
# ClawZ one-click install — Linux & macOS
#
# From a clone:
#   ./scripts/install.sh
#   ./scripts/install.sh --source
#   ./scripts/install.sh --with-web
#
# Remote one-liner:
#   curl -fsSL https://github.com/improwyz/clawz/raw/main/scripts/install.sh | bash
#
# Options:
#   --docker      Use Docker Compose (default when Docker is available)
#   --source      Build and run from source with cargo (no Docker)
#   --with-web    Build the React dashboard in web/
#   --dir PATH    Install/clone location (default: ~/clawz)
#   --help        Show usage

set -euo pipefail

CLAWZ_RAW_BASE="${CLAWZ_RAW_BASE:-https://github.com/improwyz/clawz/raw/main/scripts}"

_resolve_script_dir() {
  local candidate=""
  if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" != "bash" && "${BASH_SOURCE[0]}" != "-" ]]; then
    candidate="$(cd "$(dirname "${BASH_SOURCE[0]}")" 2>/dev/null && pwd)" || true
  fi
  if [[ -n "$candidate" && -f "${candidate}/install-common.sh" ]]; then
    echo "$candidate"
    return 0
  fi
  command -v curl >/dev/null 2>&1 || {
    echo "curl is required for the remote installer. Install curl and retry." >&2
    exit 1
  }
  local tmp
  tmp="$(mktemp -d)"
  for f in install-common.sh install-deps.sh; do
    curl -fsSL "${CLAWZ_RAW_BASE}/${f}" -o "${tmp}/${f}"
  done
  echo "$tmp"
}

SCRIPT_DIR="$(_resolve_script_dir)"
# shellcheck source=install-common.sh
source "${SCRIPT_DIR}/install-common.sh"

MODE="auto"
WITH_WEB=0

usage() {
  sed -n '2,18p' "$0"
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

ensure_curl
ensure_git

if ! ensure_repo_root "$SCRIPT_DIR/.."; then
  clone_or_update_repo
else
  cd "$SCRIPT_DIR/.."
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
