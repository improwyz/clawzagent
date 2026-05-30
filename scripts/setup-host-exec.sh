#!/usr/bin/env bash
# Allowlisted host operations for clawz-setup (deps + stack).
# Invoked by HostScriptRunner — do not add arbitrary shell passthrough.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${CLAWZ_REPO_ROOT:-${CLAWZ_INSTALL_DIR:-${PWD}}}"

if [[ ! -f "${REPO_ROOT}/docker-compose.yml" ]]; then
  echo "[clawz] repo root missing docker-compose.yml: ${REPO_ROOT}" >&2
  exit 1
fi
cd "${REPO_ROOT}"

# shellcheck source=install-deps.sh
source "${SCRIPT_DIR}/install-deps.sh"

cmd="${1:?usage: setup-host-exec.sh <command> [args...]}"
shift || true

case "${cmd}" in
  ensure_curl) ensure_curl ;;
  ensure_git) ensure_git ;;
  ensure_docker) ensure_docker ;;
  ensure_rust) ensure_rust ;;
  ensure_node) ensure_node ;;
  stack_up)
    build="${1:-0}"
    with_web="${2:-0}"
  # shellcheck source=install-common.sh
    source "${SCRIPT_DIR}/install-common.sh"
    export WITH_WEB="${with_web}"
    export CLAWZ_INSTALL_BUILD="${build}"
    install_with_docker
    ;;
  stack_down)
  # shellcheck source=install-common.sh
    source "${SCRIPT_DIR}/install-common.sh"
    # shellcheck disable=SC2086
    $COMPOSE $(compose_args base) down "$@"
    ;;
  migrate_db)
    COMPOSE="${COMPOSE:-docker compose}"
    COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml}"
    export COMPOSE COMPOSE_FILE
    exec "${SCRIPT_DIR}/migrate-db.sh"
    ;;
  *)
    echo "[clawz] unknown setup-host-exec command: ${cmd}" >&2
    exit 1
    ;;
esac
