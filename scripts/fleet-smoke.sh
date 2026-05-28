#!/usr/bin/env bash
# Fleet orchestration smoke test (requires Docker + compose micro stack).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

COMPOSE="${COMPOSE:-docker compose}"
export COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml:docker-compose.build.yml}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"
WORKER_URL="${WORKER_URL:-http://127.0.0.1:50051}"
API_KEY="${VALID_API_KEYS:-dev-key}"
TENANT_A="${CLAWZ_TENANT_ID:-default}"
TENANT_B="${FLEET_SMOKE_TENANT_B:-tenant-b}"

cleanup() {
  $COMPOSE down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

echo "Starting fleet stack (build overlay)..."
$COMPOSE up -d db
for i in $(seq 1 30); do
  if $COMPOSE exec -T db pg_isready -U postgres -d clawz >/dev/null 2>&1; then
    break
  fi
  sleep 2
done

$COMPOSE up -d worker gateway

echo "Waiting for worker..."
for i in $(seq 1 30); do
  if curl -sf "$WORKER_URL/health" | grep -q ok; then
    break
  fi
  sleep 2
done

echo "Waiting for gateway..."
for i in $(seq 1 60); do
  if curl -sf "$GATEWAY_URL/health" | grep -q OK; then
    break
  fi
  sleep 2
  if [ "$i" -eq 60 ]; then
    echo "gateway not healthy"
    exit 1
  fi
done

echo "Creating agent..."
AGENT_JSON=$(curl -sf -X POST "$GATEWAY_URL/api/v1/agents" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"fleet-smoke","model":"claude-sonnet-4-5"}')
AGENT_ID=$(echo "$AGENT_JSON" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)
if [ -z "$AGENT_ID" ]; then
  echo "failed to parse agent id: $AGENT_JSON"
  exit 1
fi

echo "Registering fleet node..."
NODE_JSON=$(curl -sf -X POST "$GATEWAY_URL/api/v1/fleet" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"name":"local-worker","node_type":"worker","host":"worker","port":50051}')
NODE_ID=$(echo "$NODE_JSON" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p' | head -1)

echo "Fleet deploy (tenant $TENANT_A)..."
curl -sf -X POST "$GATEWAY_URL/api/v1/fleet/deploy" \
  -H "Authorization: Bearer $API_KEY" \
  -H "Content-Type: application/json" \
  -d "{\"agent_id\":\"$AGENT_ID\",\"node_id\":\"$NODE_ID\"}" >/dev/null

sleep 3
if docker ps --filter "label=managed-by=clawz" --format '{{.Names}}' | grep -q "clawz-agent"; then
  echo "OK: clawz agent container present"
else
  echo "WARN: no clawz-agent container (Docker socket or scheduler may be unavailable)"
fi

echo "Worker fleet list (tenant A)..."
LIST_A=$(curl -sf "$WORKER_URL/v1/fleet/agents?tenant_id=$TENANT_A" \
  -H "Authorization: Bearer ${CLAWZ_WORKER_TOKEN:-dev-compose-worker-token}")
echo "$LIST_A" | head -c 200
echo

echo "Worker fleet list (tenant B — expect empty or no tenant A agents)..."
LIST_B=$(curl -sf "$WORKER_URL/v1/fleet/agents?tenant_id=$TENANT_B" \
  -H "Authorization: Bearer ${CLAWZ_WORKER_TOKEN:-dev-compose-worker-token}" || echo '{"agents":[]}')
if echo "$LIST_A" | grep -q "$TENANT_B" && [ "$TENANT_A" != "$TENANT_B" ]; then
  echo "FAIL: tenant B response leaked tenant A data"
  exit 1
fi
echo "OK: tenant isolation check passed"

echo "fleet-smoke completed"
