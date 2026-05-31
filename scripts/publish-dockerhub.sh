#!/usr/bin/env bash
# Build and push ClawZ platform images to private Docker Hub repositories.
#
# Prerequisites:
#   - Docker Buildx, logged in or credentials below
#   - Private repos created (this script creates them if missing)
#
# Usage:
#   export DOCKERHUB_USERNAME=sajav
#   export DOCKERHUB_TOKEN=dckr_pat_xxxx   # Hub access token (Read + Write)
#   ./scripts/publish-dockerhub.sh
#
# Optional:
#   CLAWZ_IMAGE_TAG=main ./scripts/publish-dockerhub.sh
#   PLATFORMS=linux/amd64 ./scripts/publish-dockerhub.sh   # faster single-arch
#   SKIP_BUILD=1 ./scripts/publish-dockerhub.sh            # repos only

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$ROOT"

DOCKERHUB_USERNAME="${DOCKERHUB_USERNAME:?Set DOCKERHUB_USERNAME (Hub namespace)}"
DOCKERHUB_TOKEN="${DOCKERHUB_TOKEN:?Set DOCKERHUB_TOKEN (Hub access token)}"
CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-main}"
PLATFORMS="${PLATFORMS:-linux/amd64,linux/arm64}"

log() { printf '\033[1;34m[clawz]\033[0m %s\n' "$*"; }
err() { printf '\033[1;31m[clawz]\033[0m %s\n' "$*" >&2; }

hub_jwt() {
  curl -sf -H "Content-Type: application/json" \
    -d "{\"username\":\"${DOCKERHUB_USERNAME}\",\"password\":\"${DOCKERHUB_TOKEN}\"}" \
    https://hub.docker.com/v2/users/login/ \
    | python3 -c "import sys,json; print(json.load(sys.stdin)['token'])"
}

ensure_private_repo() {
  local name="$1"
  local jwt="$2"
  local code body
  body="$(mktemp)"
  code="$(curl -s -o "$body" -w "%{http_code}" -X POST \
    -H "Authorization: Bearer ${jwt}" \
    -H "Content-Type: application/json" \
    -d "{\"namespace\":\"${DOCKERHUB_USERNAME}\",\"name\":\"${name}\",\"is_private\":true}" \
    https://hub.docker.com/v2/repositories/)"
  case "$code" in
    201) log "Created private repository ${DOCKERHUB_USERNAME}/${name}" ;;
    409)
      log "Repository ${DOCKERHUB_USERNAME}/${name} exists — enforcing private"
      curl -sf -X PATCH \
        -H "Authorization: Bearer ${jwt}" \
        -H "Content-Type: application/json" \
        -d '{"is_private":true}' \
        "https://hub.docker.com/v2/repositories/${DOCKERHUB_USERNAME}/${name}/" >/dev/null \
        || warn "Could not PATCH is_private (check Hub permissions)"
      ;;
    *)
      err "Failed to create ${name} (HTTP ${code})"
      cat "$body" >&2
      rm -f "$body"
      exit 1
      ;;
  esac
  rm -f "$body"
}

warn() { printf '\033[1;33m[clawz]\033[0m %s\n' "$*"; }

log "Logging in to Docker Hub as ${DOCKERHUB_USERNAME}..."
echo "$DOCKERHUB_TOKEN" | docker login -u "$DOCKERHUB_USERNAME" --password-stdin >/dev/null

log "Ensuring private repositories..."
JWT="$(hub_jwt)"
for repo in clawz-gateway clawz-worker clawz-agent clawz-dashboard; do
  ensure_private_repo "$repo" "$JWT"
done

if [[ "${SKIP_BUILD:-0}" == "1" ]]; then
  log "SKIP_BUILD=1 — repositories ready; skipping image build."
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

build_push() {
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
}

build_push clawz-gateway Dockerfile.gateway
build_push clawz-worker Dockerfile.worker
build_push clawz-agent Dockerfile.agent
build_push clawz-dashboard Dockerfile.web

log "Done. Private images:"
log "  docker.io/${DOCKERHUB_USERNAME}/clawz-gateway:${CLAWZ_IMAGE_TAG}"
log "  docker.io/${DOCKERHUB_USERNAME}/clawz-worker:${CLAWZ_IMAGE_TAG}"
log "  docker.io/${DOCKERHUB_USERNAME}/clawz-agent:${CLAWZ_IMAGE_TAG}"
log "  docker.io/${DOCKERHUB_USERNAME}/clawz-dashboard:${CLAWZ_IMAGE_TAG}"
log ""
log "Install fallback:"
log "  export DOCKERHUB_USERNAME=${DOCKERHUB_USERNAME}"
log "  export DOCKERHUB_TOKEN=... CLAWZ_REGISTRY_FALLBACK=docker.io/${DOCKERHUB_USERNAME}"
