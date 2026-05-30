#!/usr/bin/env bash
# Build the React dashboard and run the ClawZ Tauri desktop shell (dev mode).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/web"
if [[ ! -d node_modules ]]; then
  npm ci
fi
cd "$ROOT/crates/clawz-tauri"
if ! command -v cargo-tauri >/dev/null 2>&1; then
  echo "Install Tauri CLI: cargo install tauri-cli --locked"
  exit 1
fi
cargo tauri dev
