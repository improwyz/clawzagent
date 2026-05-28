#!/usr/bin/env bash
# Bootstrap system dependencies for ClawZ install (git, curl, Docker, Rust, optional Node).
# Sourced by install-common.sh — do not run directly.
set -euo pipefail

CLAWZ_MIN_RUST="${CLAWZ_MIN_RUST:-1.87.0}"

have_command() {
  command -v "$1" >/dev/null 2>&1
}

maybe_sudo() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif have_command sudo; then
    sudo "$@"
  else
    err "Root privileges required to run: $*"
    err "Re-run as root or install the dependency manually."
    return 1
  fi
}

load_cargo_env() {
  if [[ -f "${HOME}/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "${HOME}/.cargo/env"
  fi
  export PATH="${HOME}/.cargo/bin:${PATH}"
}

version_ge() {
  # True when $1 >= $2 (semver-ish).
  printf '%s\n%s\n' "$2" "$1" | sort -C -V 2>/dev/null || {
    local a="${1%%.*}" b="${2%%.*}"
    [[ "${a:-0}" -ge "${b:-0}" ]]
  }
}

rust_version_ok() {
  local v
  v="$(rustc --version 2>/dev/null | awk '{print $2}' | tr -d '\r')"
  [[ -n "$v" ]] && version_ge "$v" "$CLAWZ_MIN_RUST"
}

docker_ready() {
  if have_command docker && docker info >/dev/null 2>&1; then
    return 0
  fi
  if have_command sudo && sudo docker info >/dev/null 2>&1; then
    return 0
  fi
  return 1
}

configure_docker_cmd() {
  if docker info >/dev/null 2>&1; then
    export CLAWZ_DOCKER=(docker)
  elif have_command sudo && sudo docker info >/dev/null 2>&1; then
    export CLAWZ_DOCKER=(sudo docker)
  else
    return 1
  fi
  COMPOSE="${CLAWZ_DOCKER[*]} compose"
  export COMPOSE
  return 0
}

detect_linux_distro() {
  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    echo "${ID:-unknown}"
  else
    echo "unknown"
  fi
}

detect_pkg_manager() {
  if have_command apt-get; then echo apt; return; fi
  if have_command dnf; then echo dnf; return; fi
  if have_command yum; then echo yum; return; fi
  if have_command pacman; then echo pacman; return; fi
  if have_command apk; then echo apk; return; fi
  if have_command brew; then echo brew; return; fi
  echo none
}

pkg_install() {
  local pkgs=("$@")
  local pm
  pm="$(detect_pkg_manager)"
  case "$pm" in
    apt)
      maybe_sudo apt-get update -qq
      maybe_sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${pkgs[@]}"
      ;;
    dnf) maybe_sudo dnf install -y "${pkgs[@]}" ;;
    yum) maybe_sudo yum install -y "${pkgs[@]}" ;;
    pacman) maybe_sudo pacman -Sy --noconfirm "${pkgs[@]}" ;;
    apk) maybe_sudo apk add --no-cache "${pkgs[@]}" ;;
    brew) brew install "${pkgs[@]}" ;;
    *)
      err "No supported package manager found to install: ${pkgs[*]}"
      return 1
      ;;
  esac
}

ensure_curl() {
  if have_command curl; then
    return 0
  fi
  log "Installing curl..."
  pkg_install curl ca-certificates || pkg_install curl
}

ensure_git() {
  if have_command git; then
    return 0
  fi
  log "Installing Git..."
  case "$(detect_os)" in
    macos) pkg_install git ;;
    linux)
      case "$(detect_linux_distro)" in
        alpine) pkg_install git ;;
        *) pkg_install git ;;
      esac
      ;;
    *) err "Install Git manually: https://git-scm.com/downloads"; return 1 ;;
  esac
}

start_docker_daemon_linux() {
  if have_command systemctl; then
    maybe_sudo systemctl enable --now docker 2>/dev/null || true
    maybe_sudo systemctl start docker 2>/dev/null || true
  fi
  if have_command service; then
    maybe_sudo service docker start 2>/dev/null || true
  fi
}

install_docker_linux() {
  local distro
  distro="$(detect_linux_distro)"
  log "Installing Docker (${distro})..."

  case "$distro" in
    alpine)
      pkg_install docker docker-cli-compose || pkg_install docker
      ;;
    arch|manjaro)
      pkg_install docker docker-compose
      ;;
    ubuntu|debian|linuxmint|pop)
      ensure_curl
      curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
      maybe_sudo sh /tmp/get-docker.sh
      rm -f /tmp/get-docker.sh
      ;;
    fedora|rhel|centos|rocky|almalinux|amzn)
      if have_command dnf; then
        maybe_sudo dnf -y install dnf-plugins-core
        maybe_sudo dnf config-manager --add-repo https://download.docker.com/linux/centos/docker-ce.repo 2>/dev/null \
          || maybe_sudo dnf -y install moby-engine moby-cli docker-compose-plugin 2>/dev/null \
          || true
        maybe_sudo dnf -y install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin \
          || maybe_sudo dnf -y install docker docker-compose-plugin
      else
        ensure_curl
        curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
        maybe_sudo sh /tmp/get-docker.sh
        rm -f /tmp/get-docker.sh
      fi
      ;;
    *)
      ensure_curl
      curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
      maybe_sudo sh /tmp/get-docker.sh
      rm -f /tmp/get-docker.sh
      ;;
  esac

  start_docker_daemon_linux

  if have_command docker && ! docker info >/dev/null 2>&1; then
    if [[ -n "${USER:-}" ]] && getent group docker >/dev/null 2>&1; then
      maybe_sudo usermod -aG docker "$USER" 2>/dev/null || true
      warn "Added $USER to the docker group — you may need to log out/in for passwordless docker."
      warn "Using sudo for Docker commands in this session."
    fi
  fi
}

install_docker_macos() {
  if have_command docker && docker info >/dev/null 2>&1; then
    return 0
  fi
  if have_command brew; then
    log "Installing Docker Desktop via Homebrew..."
    if ! brew list --cask docker &>/dev/null 2>&1; then
      brew install --cask docker
    fi
    open -a Docker 2>/dev/null || true
    log "Waiting for Docker Desktop to start (up to 3 minutes)..."
    for _ in $(seq 1 90); do
      if docker info >/dev/null 2>&1; then
        return 0
      fi
      sleep 2
    done
  fi
  err "Install Docker Desktop for Mac: https://docs.docker.com/desktop/install/mac-install/"
  return 1
}

ensure_docker() {
  if docker_ready; then
    configure_docker_cmd
    return 0
  fi

  log "Docker not found or daemon not running — installing and starting Docker..."
  case "$(detect_os)" in
    linux) install_docker_linux ;;
    macos) install_docker_macos ;;
    *) err "Unsupported OS for automatic Docker install"; return 1 ;;
  esac

  if docker_ready; then
    configure_docker_cmd
    log "Docker is ready."
    return 0
  fi

  err "Docker installation did not complete successfully."
  return 1
}

ensure_rust() {
  load_cargo_env
  if have_command cargo && rust_version_ok; then
    log "Rust $(rustc --version | awk '{print $2}') already installed."
    return 0
  fi

  if have_command cargo && ! rust_version_ok; then
    log "Upgrading Rust toolchain (need >= ${CLAWZ_MIN_RUST})..."
    if have_command rustup; then
      rustup update stable
      rustup default stable
    fi
    load_cargo_env
    if rust_version_ok; then
      return 0
    fi
  fi

  log "Installing Rust via rustup (stable, >= ${CLAWZ_MIN_RUST})..."
  ensure_curl
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  load_cargo_env

  if ! have_command rustup; then
    err "rustup install failed"
    return 1
  fi

  rustup default stable
  rustup update stable 2>/dev/null || true

  if ! rust_version_ok; then
    warn "Installed Rust $(rustc --version 2>/dev/null || echo unknown) — build may fail if below ${CLAWZ_MIN_RUST}"
  else
    log "Rust $(rustc --version | awk '{print $2}') ready."
  fi
  return 0
}

ensure_node() {
  if have_command node && have_command npm; then
    local major
    major="$(node -p "process.versions.node.split('.')[0]" 2>/dev/null || echo 0)"
    if [[ "$major" -ge 20 ]]; then
      return 0
    fi
    warn "Node.js $(node --version 2>/dev/null || echo unknown) is older than 20 — upgrading if possible."
  fi

  log "Installing Node.js 20+..."
  case "$(detect_os)" in
    macos)
      if have_command brew; then
        brew install node@20 2>/dev/null || brew install node
        if [[ -d "$(brew --prefix node@20 2>/dev/null)/bin" ]]; then
          export PATH="$(brew --prefix node@20)/bin:${PATH}"
        fi
        return 0
      fi
      ;;
    linux)
      case "$(detect_pkg_manager)" in
        apt)
          ensure_curl
          curl -fsSL https://deb.nodesource.com/setup_20.x | maybe_sudo bash -
          maybe_sudo apt-get install -y -qq nodejs
          return 0
          ;;
        dnf|yum)
          pkg_install nodejs npm || pkg_install nodejs
          return 0
          ;;
        pacman)
          pkg_install nodejs npm
          return 0
          ;;
        apk)
          pkg_install nodejs npm
          return 0
          ;;
      esac
      ;;
  esac

  err "Install Node.js 20+ manually: https://nodejs.org/"
  return 1
}
