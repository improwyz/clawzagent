#!/usr/bin/env bash
# Smoke-test gateway + worker via docker compose (requires Docker).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not installed — skipping compose smoke"
  exit 0
fi

COMPOSE="${COMPOSE:-docker compose}"
export COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml:docker-compose.build.yml}"
export CLAWZ_MODE="${CLAWZ_MODE:-micro}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"

# Ensure .env exists (compose requires it via env_file)
if [[ ! -f .env ]] && [[ -f .env.example ]]; then
  cp .env.example .env
fi

cleanup() {
  $COMPOSE down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

$COMPOSE build gateway worker
$COMPOSE up -d db

echo "Waiting for Postgres..."
for i in $(seq 1 30); do
  if $COMPOSE exec -T db pg_isready -U postgres -d clawz >/dev/null 2>&1; then
    break
  fi
  sleep 2
  if [ "$i" -eq 30 ]; then
    echo "postgres did not become ready"
    $COMPOSE logs db
    exit 1
  fi
done

if [[ -x "${ROOT}/scripts/migrate-db.sh" ]]; then
  echo "Applying SQL migrations..."
  COMPOSE="$COMPOSE" COMPOSE_FILE="${COMPOSE_FILE}" "${ROOT}/scripts/migrate-db.sh"
fi

$COMPOSE up -d worker
echo "Waiting for worker control API..."
for i in $(seq 1 30); do
  if curl -sf "http://127.0.0.1:50051/health" | grep -q '"ok"'; then
    break
  fi
  sleep 2
  if [ "$i" -eq 30 ]; then
    echo "worker did not become reachable (continuing — gateway may still start)"
    $COMPOSE logs worker
    break
  fi
done

$COMPOSE up -d gateway

echo "Waiting for gateway health..."
for i in $(seq 1 90); do
  if curl -sf "$GATEWAY_URL/health" | grep -q OK; then
    break
  fi
  sleep 2
  if [ "$i" -eq 90 ]; then
    echo "gateway did not become healthy"
    $COMPOSE logs gateway worker db
    exit 1
  fi
done

API_KEY="${VALID_API_KEYS:-dev-key}"
AUTH=(-H "X-API-Key: $API_KEY")

curl -sf "$GATEWAY_URL/api/v1/system/health" >/dev/null
curl -sf "${AUTH[@]}" "$GATEWAY_URL/api/v1/cloud/providers" | grep -q fly_io \
  || {
    echo "cloud providers check failed"
    curl -sv "${AUTH[@]}" "$GATEWAY_URL/api/v1/cloud/providers" 2>&1 | head -40
    $COMPOSE logs gateway
    exit 1
  }

echo "compose smoke OK"
