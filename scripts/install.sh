#!/usr/bin/env bash
# ClawZ one-click install — Linux & macOS
#
# From a clone:
#   ./scripts/install.sh
#   ./scripts/install.sh --source
#   ./scripts/install.sh --with-web
#
# Remote one-liner:
#   curl -fsSL https://raw.githubusercontent.com/improwyz/clawz/main/scripts/install.sh | bash
#
# Options:
#   --docker      Use Docker Compose (default when Docker is available)
#   --source      Build and run from source with cargo (no Docker)
#   --with-web    Build the React dashboard in web/
#   --dir PATH    Install/clone location (default: ~/clawz)
#   --help        Show usage

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
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

if ! ensure_repo_root "$SCRIPT_DIR/.."; then
  clone_or_update_repo
else
  cd "$SCRIPT_DIR/.."
fi

ROOT="$(pwd)"
log "Using repository at $ROOT"

if [[ "$MODE" == "auto" ]]; then
  if command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    MODE="docker"
  else
    MODE="source"
    warn "Docker not available — falling back to source install"
  fi
fi

case "$MODE" in
  docker) install_with_docker ;;
  source) install_from_source ;;
  *) err "Invalid mode: $MODE"; exit 1 ;;
esac

if [[ "$WITH_WEB" -eq 1 ]]; then
  install_web_dashboard
fi

log "Install complete."
