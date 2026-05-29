#!/usr/bin/env bash
# ClawZ deploy bootstrap (repo root) — runs scripts/deploy.sh
#
# From a clone:
#   ./deploy.sh
#   ./scripts/deploy.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${SCRIPT_DIR}/scripts/deploy.sh" "$@"
