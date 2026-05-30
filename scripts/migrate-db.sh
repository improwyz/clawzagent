#!/usr/bin/env bash
# Apply SQL migrations from migrations/*.sql to ClawZ Postgres.
#
# Usage:
#   ./scripts/migrate-db.sh              # all migrations/*.sql in order
#   ./scripts/migrate-db.sh 008_sessions.sql
#   DATABASE_URL=postgresql://... ./scripts/migrate-db.sh
#   COMPOSE="docker compose -p infrastructure" ./scripts/migrate-db.sh
#
# Resolution order:
#   1. DATABASE_URL if psql can connect
#   2. docker compose exec -T db (service name from docker-compose.yml)
#   3. First running container matching *postgres* with database clawz

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

COMPOSE="${COMPOSE:-docker compose}"
PGUSER="${PGUSER:-postgres}"
PGDATABASE="${PGDATABASE:-clawz}"
WAIT_SECS="${CLAWZ_MIGRATE_WAIT_SECS:-60}"

usage() {
  sed -n '2,12p' "$0"
  exit 0
}

log() { echo "[clawz-migrate] $*"; }
err() { echo "[clawz-migrate] ERROR: $*" >&2; }

[[ "${1:-}" == "-h" || "${1:-}" == "--help" ]] && usage

# shellcheck disable=SC1091
[[ -f .env ]] && set -a && source .env && set +a

export COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml}"

wait_for_postgres_compose() {
  log "Waiting for Postgres (compose service db)..."
  local i
  for ((i = 1; i <= WAIT_SECS / 2; i++)); do
    if $COMPOSE exec -T db pg_isready -U "$PGUSER" -d "$PGDATABASE" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

wait_for_postgres_url() {
  log "Waiting for Postgres (DATABASE_URL)..."
  local i
  for ((i = 1; i <= WAIT_SECS / 2; i++)); do
    if psql "$DATABASE_URL" -c 'SELECT 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  return 1
}

find_postgres_container() {
  docker ps --filter 'status=running' --format '{{.Names}}' \
    | grep -E 'postgres|clawz.*db' \
    | head -1
}

detect_mode() {
  if [[ -n "${CLAWZ_MIGRATE_MODE:-}" ]]; then
    echo "$CLAWZ_MIGRATE_MODE"
    return 0
  fi

  if [[ -n "${DATABASE_URL:-}" ]] && command -v psql >/dev/null 2>&1; then
    if psql "$DATABASE_URL" -c 'SELECT 1' >/dev/null 2>&1; then
      echo "url"
      return 0
    fi
    if [[ "${DATABASE_URL}" == *"@db:"* ]] && command -v docker >/dev/null 2>&1; then
      if $COMPOSE exec -T db pg_isready -U "$PGUSER" -d "$PGDATABASE" >/dev/null 2>&1; then
        echo "compose"
        return 0
      fi
    fi
  fi

  if command -v docker >/dev/null 2>&1 && $COMPOSE exec -T db pg_isready -U "$PGUSER" -d "$PGDATABASE" >/dev/null 2>&1; then
    echo "compose"
    return 0
  fi

  if command -v docker >/dev/null 2>&1; then
    local c
    c="$(find_postgres_container || true)"
    if [[ -n "$c" ]]; then
      echo "container:$c"
      return 0
    fi
  fi

  return 1
}

apply_file() {
  local mode="$1"
  local file="$2"
  local base
  base="$(basename "$file")"
  log "Applying ${base}..."

  case "$mode" in
    url)
      psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f "$file"
      ;;
    compose)
      $COMPOSE exec -T db psql -U "$PGUSER" -d "$PGDATABASE" -v ON_ERROR_STOP=1 -f - <"$file"
      ;;
    container:*)
      local c="${mode#container:}"
      docker exec -i "$c" psql -U "$PGUSER" -d "$PGDATABASE" -v ON_ERROR_STOP=1 -f - <"$file"
      ;;
    *)
      err "unknown mode: $mode"
      exit 1
      ;;
  esac
}

main() {
  if ! command -v psql >/dev/null 2>&1; then
    if ! command -v docker >/dev/null 2>&1; then
      err "Need psql or docker to apply migrations."
      exit 1
    fi
  fi

  local mode
  if ! mode="$(detect_mode)"; then
    err "Cannot reach Postgres. Options:"
    err "  - Start stack: docker compose up -d db"
    err "  - Set DATABASE_URL to a reachable host (not @db: from the host unless using compose exec)"
    err "  - Set CLAWZ_MIGRATE_MODE=compose|url|container:<name>"
    exit 1
  fi

  case "$mode" in
    url) wait_for_postgres_url || { err "DATABASE_URL not ready"; exit 1; } ;;
    compose) wait_for_postgres_compose || { err "compose db service not ready"; exit 1; } ;;
    container:*)
      local c="${mode#container:}"
      log "Using container ${c}"
      ;;
  esac

  log "Using migration mode: ${mode}"

  local files=()
  if [[ -n "${1:-}" ]]; then
    local path="$1"
    [[ "$path" != /* ]] && path="${ROOT}/migrations/${path}"
    [[ -f "$path" ]] || { err "File not found: $path"; exit 1; }
    files=("$path")
  else
    shopt -s nullglob
    files=("${ROOT}"/migrations/*.sql)
    shopt -u nullglob
    if [[ ${#files[@]} -eq 0 ]]; then
      err "No migrations in ${ROOT}/migrations/"
      exit 1
    fi
  fi

  local f
  for f in "${files[@]}"; do
    apply_file "$mode" "$f"
  done

  log "Done (${#files[@]} file(s))."
}

main "$@"
