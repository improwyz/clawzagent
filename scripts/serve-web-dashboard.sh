#!/usr/bin/env bash
# Serve the React dashboard on a headless server (avoids wrong /usr/bin/vite).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEB="${ROOT}/web"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"
HOST="${CLAWZ_WEB_HOST:-0.0.0.0}"
PORT="${CLAWZ_WEB_PORT:-4173}"

require_command() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "[clawz] Missing: $1" >&2
    exit 1
  }
}

require_command node
require_command npm

if command -v vite >/dev/null 2>&1 && ! vite --version 2>/dev/null | grep -qE '^vite/v'; then
  echo "[clawz] WARNING: $(command -v vite) is not Node Vite (often a Qt app from apt)." >&2
  echo "[clawz] This script uses web/node_modules/.bin/vite only." >&2
fi

cd "$WEB"
if [[ ! -d node_modules/vite ]]; then
  echo "[clawz] Installing web dependencies..."
  npm ci
fi

if [[ ! -d dist ]]; then
  echo "[clawz] Building dashboard..."
  npm run build
fi

echo "[clawz] Gateway API expected at ${GATEWAY_URL} (vite proxies /api and /ws)"
echo "[clawz] Dashboard: http://${HOST}:${PORT}"
exec ./node_modules/.bin/vite preview --host "$HOST" --port "$PORT"
