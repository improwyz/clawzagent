#!/usr/bin/env bash
# ClawZ install entrypoint (repo root) — clones/updates repo, runs scripts/install.sh
#
# Remote one-liner:
#   git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz && ~/clawz/install.sh
#
# From an existing clone:
#   ./install.sh

set -euo pipefail

CLAWZ_REPO_URL="${CLAWZ_REPO_URL:-https://github.com/improwyz/clawz.git}"
CLAWZ_BRANCH="${CLAWZ_BRANCH:-main}"
CLAWZ_INSTALL_DIR="${CLAWZ_INSTALL_DIR:-${HOME}/clawz}"

for ((i = 1; i < $#; i++)); do
  if [[ "${!i}" == "--dir" && $((i + 1)) -le $# ]]; then
    CLAWZ_INSTALL_DIR="${!((i + 1))}"
    break
  fi
done

if ! command -v git >/dev/null 2>&1; then
  echo "[clawz] Git is required. Install Git: https://git-scm.com/downloads" >&2
  exit 1
fi

if [[ ! -f "${CLAWZ_INSTALL_DIR}/scripts/install.sh" ]]; then
  if [[ -d "${CLAWZ_INSTALL_DIR}/.git" ]]; then
    git -C "${CLAWZ_INSTALL_DIR}" fetch --depth 1 origin "${CLAWZ_BRANCH}"
    git -C "${CLAWZ_INSTALL_DIR}" checkout "${CLAWZ_BRANCH}"
    git -C "${CLAWZ_INSTALL_DIR}" pull --ff-only origin "${CLAWZ_BRANCH}" || true
  else
    echo "[clawz] Cloning ${CLAWZ_REPO_URL} into ${CLAWZ_INSTALL_DIR} ..."
    git clone --depth 1 --branch "${CLAWZ_BRANCH}" "${CLAWZ_REPO_URL}" "${CLAWZ_INSTALL_DIR}"
  fi
fi

exec "${CLAWZ_INSTALL_DIR}/scripts/install.sh" "$@"
