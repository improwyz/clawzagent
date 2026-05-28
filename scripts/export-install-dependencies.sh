#!/usr/bin/env bash
# Export union of Rust crates used by gateway + worker Docker builds.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OUT="${1:-docs/install-dependencies.csv}"

mkdir -p "$(dirname "$OUT")"

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

{
  cargo tree -p clawz-gateway -e normal --prefix none 2>/dev/null | sed 's/ (.*)//' | sed 's/^/gateway,/' || true
  cargo tree -p clawz-worker -e normal --prefix none 2>/dev/null | sed 's/ (.*)//' | sed 's/^/worker,/' || true
} > "$TMP"

{
  echo "crate,version,package"
  sort -u "$TMP" | while IFS= read -r line; do
  pkg="${line%%,*}"
  rest="${line#*,}"
  if [[ "$rest" == *"@"* ]]; then
    name="${rest%%@*}"
    ver="${rest#*@}"
    echo "$name,$ver,$pkg"
  else
    echo "$rest,,,$pkg"
  fi
  done
} > "$OUT"

echo "Wrote $(($(wc -l < "$OUT") - 1)) crates to $OUT"
