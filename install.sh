#!/usr/bin/env bash
# ClawZ install bootstrap (repo root) — safe to pipe from curl; clones repo, runs scripts/install.sh
#
# Curl one-liner (repo file: /install.sh on branch main — "raw/main" is URL syntax, not a folder):
#   curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
#
# Git one-liner (works for private repos if you have git credentials):
#   git clone --depth 1 https://github.com/improwyz/clawz.git ~/clawz && ~/clawz/install.sh
#
# Custom install dir:
#   CLAWZ_INSTALL_DIR=~/my-clawz curl -fsSL https://github.com/improwyz/clawz/raw/main/install.sh | bash
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
