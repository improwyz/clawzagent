#!/usr/bin/env bash
# Seed ~/.clawz/workspace with AGENTS.md and an example skill.
set -euo pipefail

ROOT="${CLAWZ_WORKSPACE:-${CLAWZ_HOME:-$HOME/.clawz}/workspace}"

mkdir -p "$ROOT/skills/code-review"

if [[ ! -f "$ROOT/AGENTS.md" ]]; then
  cat >"$ROOT/AGENTS.md" <<'EOF'
# ClawZ agent instructions

You are a helpful ClawZ assistant. Follow workspace skills when relevant.
Prefer concise answers with clear next steps.
EOF
fi

if [[ ! -f "$ROOT/skills/code-review/SKILL.md" ]]; then
  cat >"$ROOT/skills/code-review/SKILL.md" <<'EOF'
# Code review

When reviewing code, check correctness, security, and tests.
Cite file paths and suggest minimal diffs.
EOF
fi

echo "Workspace ready at $ROOT"
ls -la "$ROOT"
ls -la "$ROOT/skills" 2>/dev/null || true
