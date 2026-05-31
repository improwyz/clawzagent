#!/usr/bin/env bash
# Shared helpers for ClawZ install scripts (Linux & macOS).
set -euo pipefail

SCRIPT_DIR="${SCRIPT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"

CLAWZ_REPO_URL="${CLAWZ_REPO_URL:-https://github.com/improwyz/clawz.git}"
CLAWZ_BRANCH="${CLAWZ_BRANCH:-main}"
CLAWZ_INSTALL_DIR="${CLAWZ_INSTALL_DIR:-${HOME}/clawz}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"
COMPOSE="${COMPOSE:-docker compose}"
CLAWZ_REGISTRY="${CLAWZ_REGISTRY:-ghcr.io/improwyz}"
CLAWZ_REGISTRY_FALLBACK="${CLAWZ_REGISTRY_FALLBACK:-}"
CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-latest}"
COMPOSE_BASE="docker-compose.yml"
COMPOSE_PREBUILT="docker-compose.prebuilt.yml"
COMPOSE_BUILD="docker-compose.build.yml"

compose_args() {
  local mode="${1:-base}"
  case "$mode" in
    prebuilt) printf '%s' "-f ${COMPOSE_BASE} -f ${COMPOSE_PREBUILT}" ;;
    build) printf '%s' "-f ${COMPOSE_BASE} -f ${COMPOSE_BUILD}" ;;
    base|*) printf '%s' "-f ${COMPOSE_BASE}" ;;
  esac
}

_SCRIPT_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd)"
# shellcheck source=install-deps.sh
source "${_SCRIPT_LIB_DIR}/install-deps.sh"

log() { printf '\033[1;34m[clawz]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[clawz]\033[0m %s\n' "$*"; }
err() { printf '\033[1;31m[clawz]\033[0m %s\n' "$*" >&2; }

detect_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    *) echo "unsupported" ;;
  esac
}

require_command() {
  local cmd="$1"
  local hint="$2"
  if ! command -v "$cmd" >/dev/null 2>&1; then
    err "Missing required command: $cmd"
    [[ -n "$hint" ]] && err "$hint"
    exit 1
  fi
}

ensure_repo_root() {
  if [[ -f "${1}/Cargo.toml" && -f "${1}/docker-compose.yml" ]]; then
    cd "$1"
    return 0
  fi
  return 1
}

clone_or_update_repo() {
  if [[ -d "$CLAWZ_INSTALL_DIR/.git" ]]; then
    log "Updating existing install at $CLAWZ_INSTALL_DIR"
    git -C "$CLAWZ_INSTALL_DIR" fetch --depth 1 origin "$CLAWZ_BRANCH"
    git -C "$CLAWZ_INSTALL_DIR" checkout "$CLAWZ_BRANCH"
    git -C "$CLAWZ_INSTALL_DIR" pull --ff-only origin "$CLAWZ_BRANCH" || true
  else
    log "Cloning ClawZ into $CLAWZ_INSTALL_DIR"
    ensure_git
    git clone --depth 1 --branch "$CLAWZ_BRANCH" "$CLAWZ_REPO_URL" "$CLAWZ_INSTALL_DIR"
  fi
  cd "$CLAWZ_INSTALL_DIR"
}

write_env_file() {
  if [[ -f .env ]]; then
    log ".env already exists — leaving unchanged"
    return
  fi
  if [[ -f .env.example ]]; then
    cp .env.example .env
    # Generate random secrets for local dev
    if command -v openssl >/dev/null 2>&1; then
      jwt_secret="$(openssl rand -hex 32)"
      worker_token="$(openssl rand -hex 24)"
      sed -i.bak "s/change-me-in-production/${jwt_secret}/" .env
      sed -i.bak "s/change-me-worker-token/${worker_token}/" .env
      rm -f .env.bak
    fi
    log "Created .env from .env.example"
  else
    warn ".env.example not found — using Docker Compose defaults only"
  fi
}

wait_for_gateway() {
  log "Waiting for gateway at $GATEWAY_URL ..."
  for i in $(seq 1 90); do
    if curl -sf "$GATEWAY_URL/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  err "Gateway did not become healthy in time"
  $COMPOSE logs gateway || true
  exit 1
}

print_success() {
  local mode="$1"
  local web_port="${CLAWZ_WEB_PORT:-4173}"
  local web_host
  web_host="$(printf '%s' "${GATEWAY_URL}" | sed -E 's#https?://([^:/]+).*#\1#')"
  if [[ "$web_host" == "127.0.0.1" || "$web_host" == "localhost" ]]; then
    if command -v hostname >/dev/null 2>&1; then
      web_host="$(hostname -f 2>/dev/null || hostname 2>/dev/null || echo "$web_host")"
    fi
  fi
  local dashboard_url="http://${web_host}:${web_port}"
  local auth_line=""
  if [[ "${CLAWZ_DISABLE_AUTH:-}" == "1" ]] \
    || { [[ -f .env ]] && grep -qE '^CLAWZ_DISABLE_AUTH=1' .env; }; then
    auth_line=$'║  Dev auth:     CLAWZ_DISABLE_AUTH=1 (login bypassed)\n'
  fi
  cat <<EOF

╔══════════════════════════════════════════════════════════════╗
║  ClawZ is running ($mode)                                    ║
╠══════════════════════════════════════════════════════════════╣
║  Gateway API    $GATEWAY_URL
║  Health         $GATEWAY_URL/health
║  OpenAPI        $GATEWAY_URL/api/v1/system/openapi
║  Rooms API      $GATEWAY_URL/api/v1/rooms
║  WebSocket      ws://127.0.0.1:3000/ws/rooms/{room_id}
║  Dashboard      $dashboard_url  (./scripts/serve-web-dashboard.sh)
${auth_line}╠══════════════════════════════════════════════════════════════╣
║  Quick test:
║    curl $GATEWAY_URL/api/v1/system/health
║  Updates:  ./scripts/deploy.sh
║  Doctor:    ./scripts/deploy.sh --doctor
║
║  Stop:    $COMPOSE down
║  Logs:    $COMPOSE logs -f gateway worker
║  Docs:    INSTALL.md — "First run"; docs/deployment-build-strategy.md
╚══════════════════════════════════════════════════════════════╝

EOF
}

ghcr_logged_in() {
  [[ -f "${HOME}/.docker/config.json" ]] && grep -q '"ghcr.io"' "${HOME}/.docker/config.json" 2>/dev/null
}

docker_hub_logged_in() {
  [[ -f "${HOME}/.docker/config.json" ]] \
    && grep -qE 'index\.docker\.io|https://index\.docker\.io/v1/' "${HOME}/.docker/config.json" 2>/dev/null
}

registry_github_user() {
  if [[ -n "${CLAWZ_REGISTRY_USER:-}" ]]; then
    echo "$CLAWZ_REGISTRY_USER"
    return 0
  fi
  if [[ -n "${GITHUB_USER:-}" ]]; then
    echo "$GITHUB_USER"
    return 0
  fi
  if command -v gh >/dev/null 2>&1; then
    gh api user -q .login 2>/dev/null && return 0
  fi
  return 1
}

registry_dockerhub_user() {
  if [[ -n "${DOCKERHUB_USERNAME:-}" ]]; then
    echo "$DOCKERHUB_USERNAME"
    return 0
  fi
  if [[ "${CLAWZ_REGISTRY_FALLBACK:-}" =~ ^docker\.io/([^/]+)$ ]]; then
    echo "${BASH_REMATCH[1]}"
    return 0
  fi
  return 1
}

clawz_registry_fallback() {
  if [[ -n "${CLAWZ_REGISTRY_FALLBACK:-}" ]]; then
    echo "$CLAWZ_REGISTRY_FALLBACK"
    return 0
  fi
  local hub_user
  hub_user="$(registry_dockerhub_user)" || return 1
  echo "docker.io/${hub_user}"
}

login_ghcr() {
  if ghcr_logged_in; then
    log "Using existing docker login for ghcr.io (primary)"
    return 0
  fi
  local token="${GITHUB_TOKEN:-${GHCR_TOKEN:-${CLAWZ_REGISTRY_TOKEN:-}}}"
  if [[ -z "$token" ]]; then
    return 1
  fi
  local user
  user="$(registry_github_user)" || return 1
  log "Logging in to ghcr.io as ${user}..."
  echo "$token" | docker login ghcr.io -u "$user" --password-stdin >/dev/null
}

login_dockerhub() {
  if docker_hub_logged_in; then
    log "Using existing docker login for Docker Hub (fallback)"
    return 0
  fi
  local token="${DOCKERHUB_TOKEN:-}"
  if [[ -z "$token" ]]; then
    return 1
  fi
  local user
  user="$(registry_dockerhub_user)" || return 1
  log "Logging in to Docker Hub as ${user}..."
  echo "$token" | docker login -u "$user" --password-stdin >/dev/null
}

registry_login_hint() {
  err "Prebuilt install pulls from GHCR first, then Docker Hub if configured."
  err ""
  err "Primary (GHCR):"
  err "  export GITHUB_TOKEN=ghp_xxxx   # read:packages"
  err "  export GITHUB_USER=your_github_username"
  err ""
  err "Fallback (private Docker Hub — optional):"
  err "  export DOCKERHUB_USERNAME=your_namespace"
  err "  export DOCKERHUB_TOKEN=dckr_pat_xxxx"
  err "  export CLAWZ_REGISTRY_FALLBACK=docker.io/\${DOCKERHUB_USERNAME}"
  err ""
  err "Or: ./install.sh --build  (local compile, no registry)"
  err "Docs: docs/private-registry.md"
}

ensure_registry_auth() {
  if ! login_ghcr; then
    warn "GHCR login skipped (set GITHUB_TOKEN + GITHUB_USER for primary registry)."
  fi
  if clawz_registry_fallback >/dev/null 2>&1 || [[ -n "${DOCKERHUB_TOKEN:-}" ]]; then
    login_dockerhub || warn "Docker Hub fallback login skipped (set DOCKERHUB_TOKEN + DOCKERHUB_USERNAME)."
  fi
  if ! ghcr_logged_in && ! docker_hub_logged_in; then
    registry_login_hint
    exit 1
  fi
}

compose_has_service() {
  local svc="$1"
  shift
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args prebuilt) "$@" config --services 2>/dev/null | grep -qxF "$svc"
}

_pull_log_report() {
  local pull_log="$1"
  err "Prebuilt image pull failed."
  sed 's/^/[clawz] /' "$pull_log" >&2 || true
  err ""
  if grep -qiE 'unauthorized|denied|403|401' "$pull_log" 2>/dev/null; then
    err "Auth issue: check GHCR (read:packages) or Docker Hub token access."
  elif grep -qiE 'not found|manifest unknown|404' "$pull_log" 2>/dev/null; then
    err "Images may not be published yet. Ask maintainers to run the Release workflow or push tag v*."
    err "Until then, only ./install.sh --build will work (local compile, 10–20 min)."
  fi
}

_compose_pull_prebuilt() {
  local pull_log="$1"
  local pull_services="$2"
  shift 2
  local compose_profile_args=("$@")
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args prebuilt) "${compose_profile_args[@]}" pull $pull_services 2>"$pull_log"
}

pull_prebuilt_images() {
  local pull_log pull_services="gateway worker" compose_profile_args=()
  local primary_registry="${CLAWZ_REGISTRY:-ghcr.io/improwyz}"
  pull_log="$(mktemp)"
  trap 'rm -f "$pull_log"' RETURN

  if [[ "${WITH_WEB:-0}" == "1" ]] && compose_has_service dashboard --profile web; then
    pull_services="gateway worker dashboard"
    compose_profile_args=(--profile web)
  fi

  export CLAWZ_REGISTRY="$primary_registry"
  log "Pulling from primary registry ${CLAWZ_REGISTRY} (tag ${CLAWZ_IMAGE_TAG})..."
  if _compose_pull_prebuilt "$pull_log" "$pull_services" "${compose_profile_args[@]}"; then
    export CLAWZ_AGENT_IMAGE="${CLAWZ_REGISTRY}/clawz-agent:${CLAWZ_IMAGE_TAG}"
    return 0
  fi

  local fallback_registry
  fallback_registry="$(clawz_registry_fallback)" || true
  if [[ -z "$fallback_registry" || "$fallback_registry" == "$primary_registry" ]]; then
    _pull_log_report "$pull_log"
    registry_login_hint
    exit 1
  fi

  warn "Primary registry (${primary_registry}) failed — trying fallback (${fallback_registry})..."
  login_dockerhub || true
  export CLAWZ_REGISTRY="$fallback_registry"
  : >"$pull_log"
  log "Pulling from fallback registry ${CLAWZ_REGISTRY} (tag ${CLAWZ_IMAGE_TAG})..."
  if _compose_pull_prebuilt "$pull_log" "$pull_services" "${compose_profile_args[@]}"; then
    export CLAWZ_AGENT_IMAGE="${CLAWZ_REGISTRY}/clawz-agent:${CLAWZ_IMAGE_TAG}"
    return 0
  fi

  _pull_log_report "$pull_log"
  registry_login_hint
  exit 1
}

# Shared Compose up sequence (used by install.sh and clawz-setup StackRunner).
clawz_wait_for_gateway() {
  wait_for_gateway
}

clawz_compose_up() {
  local use_build="${1:-${CLAWZ_INSTALL_BUILD:-0}}"
  CLAWZ_INSTALL_BUILD="$use_build"
  install_with_docker
}

install_with_docker() {
  local use_build="${CLAWZ_INSTALL_BUILD:-0}"
  ensure_docker || exit 1
  if ! $COMPOSE version >/dev/null 2>&1; then
    err "Docker Compose v2 not found (try: ${COMPOSE} version)"
    exit 1
  fi

  write_env_file
  # shellcheck disable=SC1091
  [[ -f .env ]] && set -a && source .env && set +a

  export CLAWZ_REGISTRY="${CLAWZ_REGISTRY:-ghcr.io/improwyz}"
  export CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-latest}"
  if [[ -z "${CLAWZ_REGISTRY_FALLBACK:-}" && -n "${DOCKERHUB_USERNAME:-}" ]]; then
    export CLAWZ_REGISTRY_FALLBACK="docker.io/${DOCKERHUB_USERNAME}"
  fi
  export CLAWZ_AGENT_IMAGE="${CLAWZ_AGENT_IMAGE:-${CLAWZ_REGISTRY}/clawz-agent:${CLAWZ_IMAGE_TAG}}"

  local compose_mode="prebuilt"
  if [[ "$use_build" == "1" ]]; then
    log "Building gateway and worker from source (--build; first run may take 10–20 minutes)..."
    # shellcheck disable=SC2086
    $COMPOSE $(compose_args base) build gateway worker
    compose_mode="base"
  else
    ensure_registry_auth
    pull_prebuilt_images
    compose_mode="prebuilt"
  fi

  log "Starting Postgres..."
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args "$compose_mode") up -d db
  if [[ -x "${SCRIPT_DIR}/migrate-db.sh" ]]; then
    log "Applying SQL migrations..."
    COMPOSE="$COMPOSE" COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml}" \
      "${SCRIPT_DIR}/migrate-db.sh" || {
      err "Database migrations failed (see above)."
      exit 1
    }
  fi
  log "Starting worker and gateway (CLAWZ_MODE=micro, fleet orchestration)..."
  # shellcheck disable=SC2086
  $COMPOSE $(compose_args "$compose_mode") up -d worker gateway
  wait_for_gateway
  if [[ "$use_build" == "1" || "${CLAWZ_INSTALL_BUILD:-0}" == "1" ]]; then
    print_success "Docker Compose (built from source)"
  else
    print_success "Docker Compose (prebuilt images)"
  fi
}

install_from_source() {
  ensure_rust || exit 1
  load_cargo_env
  require_command curl "curl is required for smoke checks"

  write_env_file
  log "Building gateway and worker (release)..."
  cargo build --release -p clawz-gateway -p clawz-worker

  log "Starting worker..."
  CLAWZ_DISABLE_AUTH=1 \
  CLAWZ_STUB_PROVIDER=1 \
  CLAWZ_LISTEN_ADDR=127.0.0.1:50051 \
  ./target/release/clawz-worker &
  echo $! > .clawz-worker.pid

  log "Starting gateway..."
  CLAWZ_DISABLE_AUTH=1 \
  CLAWZ_STUB_PROVIDER=1 \
  CLAWZ_JWT_SECRET=local-dev-secret \
  CLAWZ_WORKER_TOKEN=local-dev-worker \
  WORKER_URL=http://127.0.0.1:50051 \
  ./target/release/clawz-gateway &
  echo $! > .clawz-gateway.pid

  wait_for_gateway
  print_success "source (local binaries)"
  warn "Stop with: kill \$(cat .clawz-gateway.pid .clawz-worker.pid)"
}

install_web_dashboard() {
  if [[ ! -d web ]]; then
    warn "web/ directory not found — skipping dashboard"
    return
  fi
  ensure_node || exit 1
  log "Installing web dashboard dependencies..."
  (cd web && npm ci)
  log "Building web dashboard..."
  (cd web && npm run build)
  log "Dashboard built to web/dist — serve with: cd web && npm run preview"
}
