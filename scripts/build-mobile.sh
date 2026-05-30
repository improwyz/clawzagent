#!/usr/bin/env bash
# ClawZ mobile build stubs (requires Tauri CLI + platform SDKs).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/crates/clawz-tauri-mobile"

echo "Building web assets..."
npm run build --prefix "$ROOT/web"

if command -v cargo-tauri >/dev/null 2>&1; then
  echo "Android (debug)..."
  cargo tauri android build --debug || echo "Android build skipped (SDK not configured)"
  echo "iOS (debug)..."
  cargo tauri ios build --debug || echo "iOS build skipped (Xcode not configured)"
else
  echo "Install tauri-cli: cargo install tauri-cli"
  exit 1
fi
