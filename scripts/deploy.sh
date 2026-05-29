#!/usr/bin/env bash
# ClawZ deployment — git pull, change detection, prebuilt images or local build.
#
#   ./scripts/deploy.sh              # pull (optional skip), detect changes, deploy
#   ./scripts/deploy.sh --web-only   # npm build web/; restart serve-web-dashboard.sh
#   ./scripts/deploy.sh --pull       # force pull gateway+worker (prebuilt)
#   ./scripts/deploy.sh --tag v1.0.0 # set CLAWZ_IMAGE_TAG before pull
#   ./scripts/deploy.sh --doctor     # health checks only
#   ./scripts/deploy.sh --build      # local compose build (confirm with 'yes')
#   ./scripts/deploy.sh --no-pull    # skip git pull
#   ./scripts/deploy.sh --help

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=install-common.sh
source "${SCRIPT_DIR}/install-common.sh"

DO_GIT_PULL=1
FORCE_PULL=0
WEB_ONLY=0
DOCTOR=0
DO_BUILD=0

usage() {
  sed -n '2,12p' "$0"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-pull) DO_GIT_PULL=0; shift ;;
    --pull) FORCE_PULL=1; shift ;;
    --web-only) WEB_ONLY=1; shift ;;
    --tag)
      CLAWZ_IMAGE_TAG="$2"
      shift 2
      ;;
    --doctor) DOCTOR=1; shift ;;
    --build) DO_BUILD=1; shift ;;
    --help|-h) usage ;;
    *)
      err "Unknown option: $1"
      usage
      ;;
  esac
done

cd "$ROOT"

load_env_file() {
  # shellcheck disable=SC1091
  if [[ -f .env ]]; then
    set -a
    # shellcheck disable=SC1091
    source .env
    set +a
  fi
  export CLAWZ_REGISTRY="${CLAWZ_REGISTRY:-ghcr.io/improwyz}"
  export CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-latest}"
  export CLAWZ_AGENT_IMAGE="${CLAWZ_AGENT_IMAGE:-${CLAWZ_REGISTRY}/clawz-agent:${CLAWZ_IMAGE_TAG}}"
}

require_docker() {
  ensure_docker || exit 1
  if ! $COMPOSE version >/dev/null 2>&1; then
    err "Docker Compose v2 not found (try: ${COMPOSE} version)"
    exit 1
  fi
}

compose_prebuilt() {
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args prebuilt) "$@"
}

compose_build_mode() {
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args build) "$@"
}

git_pull_repo() {
  if [[ ! -d .git ]]; then
    warn "Not a git repository — skipping git pull"
    return 0
  fi
  log "Pulling latest changes..."
  git fetch origin 2>/dev/null || git fetch 2>/dev/null || true
  local branch
  branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)"
  if git rev-parse --abbrev-ref "@{u}" >/dev/null 2>&1; then
    git pull --ff-only 2>/dev/null || git pull --ff-only origin "$branch" 2>/dev/null || {
      warn "git pull --ff-only failed — continuing with local tree"
    }
  else
    git pull --ff-only origin "$branch" 2>/dev/null || {
      warn "git pull failed — continuing with local tree"
    }
  fi
}

changed_files() {
  if [[ ! -d .git ]]; then
    return 0
  fi
  local range=""
  if git rev-parse --abbrev-ref "@{u}" >/dev/null 2>&1; then
    range="@{u}..HEAD"
  elif git rev-parse HEAD~1 >/dev/null 2>&1; then
    range="HEAD~1..HEAD"
  else
    return 0
  fi
  git diff --name-only "$range" 2>/dev/null || true
}

analyze_changes() {
  local files="$1"
  DEPLOY_ACTION="none"
  DEPLOY_PULL_GATEWAY=0
  DEPLOY_PULL_WORKER=0
  DEPLOY_WEB=0
  DEPLOY_COMPOSE_RECREATE=0

  if [[ -z "$files" ]]; then
    log "No changed files detected in comparison range — nothing to deploy"
    return 0
  fi

  local line doc_only=1
  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    case "$line" in
      docs/*|*.md) ;;
      *)
        doc_only=0
        ;;
    esac
  done <<< "$files"

  if [[ "$doc_only" -eq 1 ]]; then
    local non_doc=0
    while IFS= read -r line; do
      [[ -z "$line" ]] && continue
      case "$line" in
        docs/*|README.md|AGENTS.md|CLAWZ.md|INSTALL.md|CONTRIBUTING.md) ;;
        *) non_doc=1; break ;;
      esac
    done <<< "$files"
    if [[ "$non_doc" -eq 0 ]]; then
      DEPLOY_ACTION="docs_only"
      return 0
    fi
  fi

  while IFS= read -r line; do
    [[ -z "$line" ]] && continue
    case "$line" in
      web/*)
        DEPLOY_WEB=1
        ;;
      crates/clawz-gateway/*)
        DEPLOY_PULL_GATEWAY=1
        ;;
      crates/clawz-worker/*|crates/clawz-core/*|Cargo.lock)
        DEPLOY_PULL_GATEWAY=1
        DEPLOY_PULL_WORKER=1
        ;;
      docker-compose*.yml|.env.example)
        DEPLOY_COMPOSE_RECREATE=1
        ;;
    esac
  done <<< "$files"

  if [[ "$DEPLOY_COMPOSE_RECREATE" -eq 1 ]]; then
    DEPLOY_ACTION="compose_recreate"
    DEPLOY_PULL_GATEWAY=1
    DEPLOY_PULL_WORKER=1
  elif [[ "$DEPLOY_PULL_WORKER" -eq 1 ]]; then
    DEPLOY_ACTION="gateway_worker"
  elif [[ "$DEPLOY_PULL_GATEWAY" -eq 1 ]]; then
    DEPLOY_ACTION="gateway_only"
  elif [[ "$DEPLOY_WEB" -eq 1 ]]; then
    DEPLOY_ACTION="web_only"
  else
    DEPLOY_ACTION="none"
  fi
}

deploy_web_only() {
  if [[ ! -d web ]]; then
    err "web/ directory not found"
    exit 1
  fi
  require_command node "Install Node.js: https://nodejs.org/"
  require_command npm "npm ships with Node.js"
  log "Building web dashboard..."
  (
    cd web
    if [[ ! -d node_modules ]]; then
      log "node_modules missing — running npm ci"
      npm ci
    fi
    npm run build
  )
  log "Web build complete (web/dist)."
  warn "Restart the dashboard if it is already running:"
  warn "  ./scripts/serve-web-dashboard.sh"
}

pull_services() {
  local pull_gateway="${1:-1}"
  local pull_worker="${2:-1}"
  if [[ "$pull_gateway" -eq 1 && "$pull_worker" -eq 1 ]]; then
    pull_prebuilt_images
    return 0
  fi
  local pull_log
  pull_log="$(mktemp)"
  trap 'rm -f "$pull_log"' RETURN
  if [[ "$pull_gateway" -eq 1 ]]; then
    log "Pulling gateway image (${CLAWZ_IMAGE_TAG})..."
    # shellcheck disable=SC2086
    $COMPOSE $(compose_args prebuilt) pull gateway 2>"$pull_log" || {
      err "Gateway image pull failed"
      sed 's/^/[clawz] /' "$pull_log" >&2 || true
      exit 1
    }
  fi
  if [[ "$pull_worker" -eq 1 ]]; then
    log "Pulling worker image (${CLAWZ_IMAGE_TAG})..."
    # shellcheck disable=SC2086
    $COMPOSE $(compose_args prebuilt) pull worker 2>"$pull_log" || {
      err "Worker image pull failed"
      sed 's/^/[clawz] /' "$pull_log" >&2 || true
      exit 1
    }
  fi
}

up_gateway_worker() {
  local up_gateway="${1:-1}"
  local up_worker="${2:-1}"
  log "Ensuring Postgres is up..."
  compose_prebuilt up -d db
  if [[ "$up_worker" -eq 1 ]]; then
    log "Starting worker..."
    compose_prebuilt up -d worker
  fi
  if [[ "$up_gateway" -eq 1 ]]; then
    log "Starting gateway..."
    compose_prebuilt up -d gateway
  fi
}

deploy_force_pull() {
  ensure_registry_auth
  pull_prebuilt_images
  up_gateway_worker 1 1
  wait_for_gateway
  log "Pull deploy complete (gateway + worker, tag=${CLAWZ_IMAGE_TAG})."
}

deploy_gateway_only() {
  ensure_registry_auth
  pull_services 1 "${DEPLOY_PULL_WORKER}"
  up_gateway_worker 1 "${DEPLOY_PULL_WORKER}"
  wait_for_gateway
  log "Gateway deploy complete (tag=${CLAWZ_IMAGE_TAG})."
}

deploy_gateway_worker() {
  ensure_registry_auth
  pull_services 1 1
  up_gateway_worker 1 1
  wait_for_gateway
  log "Gateway + worker deploy complete (tag=${CLAWZ_IMAGE_TAG})."
}

deploy_compose_recreate() {
  ensure_registry_auth
  pull_prebuilt_images
  log "Recreating compose stack (config/env changed)..."
  compose_prebuilt up -d --force-recreate
  wait_for_gateway
  log "Compose recreate complete."
}

confirm_build() {
  warn "Local build typically takes 10–20 minutes on first run."
  printf '\033[1;33m[clawz]\033[0m Type yes to continue: '
  local answer=""
  read -r answer
  if [[ "$answer" != "yes" ]]; then
    err "Build cancelled."
    exit 1
  fi
}

deploy_build() {
  confirm_build
  log "Building gateway and worker from source..."
  compose_build_mode build gateway worker
  log "Starting stack..."
  compose_build_mode up -d db worker gateway
  wait_for_gateway
  log "Build deploy complete."
}

image_digest() {
  local image_ref="$1"
  docker image inspect --format '{{index .RepoDigests 0}}' "$image_ref" 2>/dev/null \
    || docker image inspect --format '{{.Id}}' "$image_ref" 2>/dev/null \
    || echo "unknown"
}

run_doctor() {
  require_docker
  load_env_file

  log "=== ClawZ deploy doctor ==="
  echo ""
  log "CLAWZ_IMAGE_TAG=${CLAWZ_IMAGE_TAG}"
  log "CLAWZ_REGISTRY=${CLAWZ_REGISTRY}"
  if [[ -d .git ]]; then
    log "git rev: $(git rev-parse --short HEAD 2>/dev/null || echo unknown) ($(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo detached))"
  else
    warn "Not a git repository"
  fi
  echo ""
  log "Compose status:"
  compose_prebuilt ps || true
  echo ""
  local gw_image="${CLAWZ_REGISTRY}/clawz-gateway:${CLAWZ_IMAGE_TAG}"
  local wk_image="${CLAWZ_REGISTRY}/clawz-worker:${CLAWZ_IMAGE_TAG}"
  log "Image digests (if present locally):"
  log "  gateway: $(image_digest "$gw_image")"
  log "  worker:  $(image_digest "$wk_image")"
  echo ""
  log "HTTP health:"
  if curl -sf "${GATEWAY_URL}/health" 2>/dev/null; then
    echo ""
  else
    err "  GET ${GATEWAY_URL}/health — failed"
  fi
  if curl -sf "${GATEWAY_URL}/api/v1/system/health" 2>/dev/null; then
    echo ""
  else
    err "  GET ${GATEWAY_URL}/api/v1/system/health — failed"
  fi
}

run_auto_deploy() {
  local files
  files="$(changed_files)"
  if [[ -n "$files" ]]; then
    log "Changed files:"
    echo "$files" | sed 's/^/  /'
  fi
  analyze_changes "$files"

  case "$DEPLOY_ACTION" in
    docs_only)
      log "Docs-only changes — no deploy needed."
      ;;
    web_only)
      deploy_web_only
      ;;
    gateway_only)
      deploy_gateway_only
      ;;
    gateway_worker)
      deploy_gateway_worker
      ;;
    compose_recreate)
      deploy_compose_recreate
      ;;
    none)
      log "No deployable paths in change set."
      ;;
  esac

  if [[ "$DEPLOY_WEB" -eq 1 && "$DEPLOY_ACTION" != "web_only" && "$DEPLOY_ACTION" != "docs_only" && "$DEPLOY_ACTION" != "none" ]]; then
    log "Also rebuilding web (web/ changed alongside backend)..."
    deploy_web_only
  fi
}

# --- main ---

if [[ "$DOCTOR" -eq 1 ]]; then
  run_doctor
  exit 0
fi

if [[ "$WEB_ONLY" -eq 1 ]]; then
  deploy_web_only
  exit 0
fi

require_docker
load_env_file

if [[ "$DO_BUILD" -eq 1 ]]; then
  deploy_build
  exit 0
fi

if [[ "$FORCE_PULL" -eq 1 ]]; then
  ensure_registry_auth
  deploy_force_pull
  exit 0
fi

if [[ "$DO_GIT_PULL" -eq 1 ]]; then
  git_pull_repo
fi

run_auto_deploy
