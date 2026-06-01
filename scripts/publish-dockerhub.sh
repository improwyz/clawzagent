#!/usr/bin/env bash
# Build and push ClawZ platform images to private Docker Hub.
#
# Layouts:
#   multirepo (default) — docker.io/user/clawz-gateway:tag, clawz-worker, …
#   monorepo            — docker.io/user/clawz:gateway-tag, worker-tag, …
#                         set CLAWZ_HUB_MONOREPO=clawz (repo name, default "clawz")
#
# IMPORTANT: `docker push` to a missing repo creates a PUBLIC repository on Docker Hub.
# This script must create or patch repos to private via the Hub API before any push.
#
# Usage:
#   export DOCKERHUB_USERNAME=your_namespace
#   export DOCKERHUB_TOKEN=dckr_pat_xxxx   # Read + Write + Delete
#   ./scripts/publish-dockerhub.sh
#
# Single private repo (e.g. sajanv/clawz):
#   CLAWZ_HUB_MONOREPO=clawz ./scripts/publish-dockerhub.sh
#
# Optional:
#   CLAWZ_IMAGE_TAG=main PLATFORMS=linux/amd64 ./scripts/publish-dockerhub.sh
#   SKIP_BUILD=1 ./scripts/publish-dockerhub.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$ROOT"

DOCKERHUB_USERNAME="${DOCKERHUB_USERNAME:?Set DOCKERHUB_USERNAME (Hub namespace)}"
DOCKERHUB_TOKEN="${DOCKERHUB_TOKEN:?Set DOCKERHUB_TOKEN (Hub access token)}"
CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-main}"
PLATFORMS="${PLATFORMS:-linux/amd64,linux/arm64}"
CLAWZ_HUB_MONOREPO="${CLAWZ_HUB_MONOREPO:-}"

if [[ -n "$CLAWZ_HUB_MONOREPO" ]]; then
  CLAWZ_HUB_REPOS=("$CLAWZ_HUB_MONOREPO")
  CLAWZ_PUBLISH_COMPONENTS=(gateway worker agent dashboard)
else
  CLAWZ_HUB_REPOS=(clawz-gateway clawz-worker clawz-agent clawz-dashboard)
  CLAWZ_PUBLISH_COMPONENTS=()
fi

log() { printf '\033[1;34m[clawz]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[clawz]\033[0m %s\n' "$*"; }
err() { printf '\033[1;31m[clawz]\033[0m %s\n' "$*" >&2; }

hub_token_scope_help() {
  err ""
  err "Docker Hub token cannot manage repositories (HTTP 403)."
  err "A push without private repos first will create PUBLIC repositories."
  err ""
  err "Fix one of:"
  err "  1. Create a new access token with Read + Write + Delete:"
  err "     https://hub.docker.com/settings/security"
  err "  2. Manually create private repo(s) on hub.docker.com, then re-run:"
  for r in "${CLAWZ_HUB_REPOS[@]}"; do
    err "       ${DOCKERHUB_USERNAME}/${r}"
  done
  err ""
  err "Docs: docs/private-registry.md"
}

hub_jwt() {
  curl -sf -H "Content-Type: application/json" \
    -d "{\"username\":\"${DOCKERHUB_USERNAME}\",\"password\":\"${DOCKERHUB_TOKEN}\"}" \
    https://hub.docker.com/v2/users/login/ \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])"
}

repo_get_json() {
  local name="$1"
  local jwt="$2"
  local out="$3"
  curl -sS -o "$out" -w "%{http_code}" \
    -H "Authorization: Bearer ${jwt}" \
    "https://hub.docker.com/v2/repositories/${DOCKERHUB_USERNAME}/${name}/"
}

repo_is_private() {
  local name="$1"
  local jwt="$2"
  local body code
  body="$(mktemp)"
  code="$(repo_get_json "$name" "$jwt" "$body")"
  if [[ "$code" != "200" ]]; then
    rm -f "$body"
    return 2
  fi
  if python3 -c 'import json,sys; sys.exit(0 if json.load(open(sys.argv[1])).get("is_private") else 1)' "$body"; then
    rm -f "$body"
    return 0
  fi
  rm -f "$body"
  return 1
}

patch_repo_private() {
  local name="$1"
  local jwt="$2"
  local code body
  body="$(mktemp)"
  code="$(curl -sS -o "$body" -w "%{http_code}" -X PATCH \
    -H "Authorization: Bearer ${jwt}" \
    -H "Content-Type: application/json" \
    -d '{"is_private":true}' \
    "https://hub.docker.com/v2/repositories/${DOCKERHUB_USERNAME}/${name}/")"
  rm -f "$body"
  [[ "$code" == "200" ]]
}

ensure_private_repo() {
  local name="$1"
  local jwt="$2"
  local code body

  if repo_is_private "$name" "$jwt"; then
    log "Repository ${DOCKERHUB_USERNAME}/${name} is private"
    return 0
  fi

  body="$(mktemp)"
  code="$(repo_get_json "$name" "$jwt" "$body")"
  if [[ "$code" == "200" ]]; then
    rm -f "$body"
    warn "Repository ${DOCKERHUB_USERNAME}/${name} exists but is public — setting private"
    if ! patch_repo_private "$name" "$jwt"; then
      err "Could not set ${DOCKERHUB_USERNAME}/${name} to private (check token permissions)"
      hub_token_scope_help
      exit 1
    fi
    log "Repository ${DOCKERHUB_USERNAME}/${name} is now private"
    return 0
  fi
  rm -f "$body"

  if [[ "$code" != "404" ]]; then
    err "Unexpected Hub API response for ${name} (HTTP ${code})"
    exit 1
  fi

  body="$(mktemp)"
  code="$(curl -sS -o "$body" -w "%{http_code}" -X POST \
    -H "Authorization: Bearer ${jwt}" \
    -H "Content-Type: application/json" \
    -d "{\"namespace\":\"${DOCKERHUB_USERNAME}\",\"name\":\"${name}\",\"is_private\":true}" \
    https://hub.docker.com/v2/repositories/)"
  case "$code" in
    201)
      log "Created private repository ${DOCKERHUB_USERNAME}/${name}"
      ;;
    409)
      if ! patch_repo_private "$name" "$jwt"; then
        err "Repository ${name} exists but could not enforce private"
        hub_token_scope_help
        exit 1
      fi
      log "Repository ${DOCKERHUB_USERNAME}/${name} exists — enforced private"
      ;;
    403)
      err "Cannot create private repository ${DOCKERHUB_USERNAME}/${name} (HTTP 403)"
      cat "$body" >&2
      rm -f "$body"
      hub_token_scope_help
      exit 1
      ;;
    *)
      err "Failed to create ${name} (HTTP ${code})"
      cat "$body" >&2
      rm -f "$body"
      exit 1
      ;;
  esac
  rm -f "$body"

  if ! repo_is_private "$name" "$jwt"; then
    err "Repository ${DOCKERHUB_USERNAME}/${name} is not private after setup"
    exit 1
  fi
}

assert_all_repos_private() {
  local jwt="$1"
  local name
  for name in "${CLAWZ_HUB_REPOS[@]}"; do
    if ! repo_is_private "$name" "$jwt"; then
      err "Refusing to push: ${DOCKERHUB_USERNAME}/${name} is missing or not private"
      hub_token_scope_help
      exit 1
    fi
  done
}

log "Logging in to Docker Hub as ${DOCKERHUB_USERNAME}..."
echo "$DOCKERHUB_TOKEN" | docker login -u "$DOCKERHUB_USERNAME" --password-stdin >/dev/null

if [[ -n "$CLAWZ_HUB_MONOREPO" ]]; then
  log "Monorepo layout: docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}:<component>-<tag>"
else
  log "Multirepo layout: docker.io/${DOCKERHUB_USERNAME}/clawz-<component>:<tag>"
fi

log "Ensuring private repositories (required before push)..."
JWT="$(hub_jwt)"
for repo in "${CLAWZ_HUB_REPOS[@]}"; do
  ensure_private_repo "$repo" "$JWT"
done
assert_all_repos_private "$JWT"

if [[ "${SKIP_BUILD:-0}" == "1" ]]; then
  log "SKIP_BUILD=1 — private repositories ready; skipping image build."
  exit 0
fi

if ! docker buildx version >/dev/null 2>&1; then
  err "docker buildx is required"
  exit 1
fi

if ! docker buildx inspect clawz-publish >/dev/null 2>&1; then
  docker buildx create --name clawz-publish --driver docker-container --use >/dev/null
  docker buildx inspect --bootstrap >/dev/null
else
  docker buildx use clawz-publish >/dev/null
fi

build_push_multirepo() {
  local image="$1"
  local dockerfile="$2"
  local ref="docker.io/${DOCKERHUB_USERNAME}/${image}"
  log "Building and pushing ${ref}:${CLAWZ_IMAGE_TAG} (${PLATFORMS})..."
  docker buildx build \
    --platform "$PLATFORMS" \
    --file "$dockerfile" \
    --tag "${ref}:${CLAWZ_IMAGE_TAG}" \
    --tag "${ref}:latest" \
    --push \
    .
  if ! repo_is_private "$image" "$JWT"; then
    err "After push, ${DOCKERHUB_USERNAME}/${image} is not private"
    exit 1
  fi
}

build_push_monorepo() {
  local component="$1"
  local dockerfile="$2"
  local ref="docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}"
  log "Building and pushing ${ref}:${component}-${CLAWZ_IMAGE_TAG} (${PLATFORMS})..."
  docker buildx build \
    --platform "$PLATFORMS" \
    --file "$dockerfile" \
    --tag "${ref}:${component}-${CLAWZ_IMAGE_TAG}" \
    --tag "${ref}:${component}-latest" \
    --push \
    .
}

if [[ -n "$CLAWZ_HUB_MONOREPO" ]]; then
  build_push_monorepo gateway Dockerfile.gateway
  build_push_monorepo worker Dockerfile.worker
  build_push_monorepo agent Dockerfile.agent
  build_push_monorepo dashboard Dockerfile.web
  if ! repo_is_private "$CLAWZ_HUB_MONOREPO" "$JWT"; then
    err "After push, ${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO} is not private"
    exit 1
  fi
  log "Done. Private monorepo (component tags):"
  log "  docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}:gateway-${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}:worker-${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}:agent-${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/${CLAWZ_HUB_MONOREPO}:dashboard-${CLAWZ_IMAGE_TAG}"
else
  build_push_multirepo clawz-gateway Dockerfile.gateway
  build_push_multirepo clawz-worker Dockerfile.worker
  build_push_multirepo clawz-agent Dockerfile.agent
  build_push_multirepo clawz-dashboard Dockerfile.web
  log "Done. Private images:"
  log "  docker.io/${DOCKERHUB_USERNAME}/clawz-gateway:${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/clawz-worker:${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/clawz-agent:${CLAWZ_IMAGE_TAG}"
  log "  docker.io/${DOCKERHUB_USERNAME}/clawz-dashboard:${CLAWZ_IMAGE_TAG}"
fi

log ""
log "Install fallback (monorepo):"
log "  export DOCKERHUB_USERNAME=${DOCKERHUB_USERNAME}"
log "  export DOCKERHUB_TOKEN=..."
log "  export CLAWZ_HUB_MONOREPO=${CLAWZ_HUB_MONOREPO:-clawz}"
log "  export CLAWZ_REGISTRY_FALLBACK=docker.io/${DOCKERHUB_USERNAME}"
