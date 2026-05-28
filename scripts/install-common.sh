#!/usr/bin/env bash
# Shared helpers for ClawZ install scripts (Linux & macOS).
set -euo pipefail

CLAWZ_REPO_URL="${CLAWZ_REPO_URL:-https://github.com/improwyz/clawz.git}"
CLAWZ_BRANCH="${CLAWZ_BRANCH:-main}"
CLAWZ_INSTALL_DIR="${CLAWZ_INSTALL_DIR:-${HOME}/clawz}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:3000}"
COMPOSE="${COMPOSE:-docker compose}"
CLAWZ_REGISTRY="${CLAWZ_REGISTRY:-ghcr.io/improwyz}"
CLAWZ_IMAGE_TAG="${CLAWZ_IMAGE_TAG:-latest}"
# Default: fleet stack + prebuilt pull. Use COMPOSE_FILE_BUILD for local image build.
export COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml:docker-compose.prebuilt.yml}"
COMPOSE_FILE_BUILD="docker-compose.yml:docker-compose.build.yml"

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
  cat <<EOF

╔══════════════════════════════════════════════════════════════╗
║  ClawZ is running ($mode)                                    ║
╠══════════════════════════════════════════════════════════════╣
║  Gateway API    $GATEWAY_URL
║  Health         $GATEWAY_URL/health
║  OpenAPI        $GATEWAY_URL/api/v1/system/openapi
║  Rooms API      $GATEWAY_URL/api/v1/rooms
║  WebSocket      ws://127.0.0.1:3000/ws/rooms/{room_id}
╠══════════════════════════════════════════════════════════════╣
║  Quick test:
║    curl $GATEWAY_URL/api/v1/system/health
║
║  Stop:    $COMPOSE down
║  Logs:    $COMPOSE logs -f gateway worker
║  Docs:    README.md — "New features setup"
╚══════════════════════════════════════════════════════════════╝

EOF
}

registry_login_hint() {
  warn "If image pull fails, authenticate to your registry:"
  warn "  docker login ghcr.io"
  warn "See docs/private-registry.md"
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
  export CLAWZ_AGENT_IMAGE="${CLAWZ_AGENT_IMAGE:-${CLAWZ_REGISTRY}/clawz-agent:${CLAWZ_IMAGE_TAG}}"

  if [[ "$use_build" == "1" ]]; then
    export COMPOSE_FILE="$COMPOSE_FILE_BUILD"
    log "Building gateway and worker from source..."
    $COMPOSE build gateway worker
  else
    export COMPOSE_FILE="${COMPOSE_FILE:-docker-compose.yml:docker-compose.prebuilt.yml}"
    log "Pulling platform images from ${CLAWZ_REGISTRY} (tag ${CLAWZ_IMAGE_TAG})..."
    if ! $COMPOSE pull gateway worker 2>/dev/null; then
      registry_login_hint
      warn "Pull failed — falling back to local build (use --build to skip this attempt)"
      export COMPOSE_FILE="$COMPOSE_FILE_BUILD"
      CLAWZ_INSTALL_BUILD=1
      $COMPOSE build gateway worker
    fi
  fi

  log "Starting Postgres..."
  $COMPOSE up -d db
  log "Starting worker and gateway (CLAWZ_MODE=micro, fleet orchestration)..."
  $COMPOSE up -d worker gateway
  wait_for_gateway
  if [[ "$use_build" == "1" ]]; then
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
