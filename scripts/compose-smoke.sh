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
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"

cleanup() {
  $COMPOSE down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

$COMPOSE build gateway worker
$COMPOSE up -d db
$COMPOSE up -d worker gateway

echo "Waiting for gateway health..."
for i in $(seq 1 60); do
  if curl -sf "$GATEWAY_URL/health" | grep -q OK; then
    break
  fi
  sleep 2
  if [ "$i" -eq 60 ]; then
    echo "gateway did not become healthy"
    $COMPOSE logs gateway
    exit 1
  fi
done

curl -sf "$GATEWAY_URL/api/v1/system/health" >/dev/null
curl -sf "$GATEWAY_URL/api/v1/cloud/providers" | grep -q fly_io

echo "compose smoke OK"
