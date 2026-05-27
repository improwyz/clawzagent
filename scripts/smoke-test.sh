#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

export CLAWZ_DISABLE_AUTH=1
export CLAWZ_STUB_PROVIDER=1
export CLAWZ_JWT_SECRET=test-smoke-secret
export RUST_LOG=info

cleanup() {
  if [[ -n "${GATEWAY_PID:-}" ]]; then kill "$GATEWAY_PID" 2>/dev/null || true; fi
  if [[ -n "${WORKER_PID:-}" ]]; then kill "$WORKER_PID" 2>/dev/null || true; fi
}
trap cleanup EXIT

cargo build -p clawz-worker -p clawz-gateway --quiet

./target/debug/clawz-worker &
WORKER_PID=$!
sleep 2

WORKER_URL=http://127.0.0.1:50051 ./target/debug/clawz-gateway &
GATEWAY_PID=$!
sleep 2

echo "Creating agent..."
AGENT_JSON=$(curl -sf -X POST http://127.0.0.1:3000/api/v1/agents \
  -H 'Content-Type: application/json' \
  -d '{"name":"smoke","model":"claude-sonnet-4-5"}')
AGENT_ID=$(echo "$AGENT_JSON" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")

echo "Running agent turn..."
RUN_JSON=$(curl -sf -X POST "http://127.0.0.1:3000/api/v1/agents/${AGENT_ID}/run" \
  -H 'Content-Type: application/json' \
  -d '{"message":"ping"}')
echo "$RUN_JSON" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('content'), d"

echo "Checking dashboard metrics..."
curl -sf http://127.0.0.1:3000/api/v1/dashboard/metrics >/dev/null

echo "Smoke test passed."
