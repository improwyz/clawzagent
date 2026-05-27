#!/usr/bin/env bash
# Convenience wrapper — run from repo root.
exec "$(cd "$(dirname "$0")" && pwd)/scripts/install.sh" "$@"
