#!/bin/sh
# ClawZ single-line install script
# Usage: curl -fsSL https://install.clawz.net | sh
# or: curl -fsSL https://releases.clawz.net/latest/install.sh | sh

set -e

ARCH="$(uname -m)"
OS="$(uname -s)"
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
RELEASE_BASE="${RELEASE_BASE:-https://releases.clawz.net/latest}"

echo "Installing ClawZ agent runtime for ${OS}/${ARCH}..."

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64)   ASSET="clawz-linux-amd64.tar.gz";;
      aarch64)  ASSET="clawz-linux-arm64.tar.gz";;
      riscv64)  ASSET="clawz-linux-riscv64.tar.gz";;
      *) echo "Unsupported architecture: $ARCH"; exit 1;;
    esac;;
  Darwin)
    case "$ARCH" in
      x86_64)   ASSET="clawz-macos-x86_64.tar.gz";;
      arm64)    ASSET="clawz-macos-arm64.tar.gz";;
      *) echo "Unsupported architecture: $ARCH"; exit 1;;
    esac;;
  *)
    echo "Unsupported OS: $OS"; exit 1;;
esac

# Download and verify
TMPDIR="$(mktemp -d)"
curl -fsSL "${RELEASE_BASE}/${ASSET}" -o "${TMPDIR}/clawz.tar.gz"
curl -fsSL "${RELEASE_BASE}/${ASSET}.sha256" -o "${TMPDIR}/clawz.tar.gz.sha256"
sha256sum -c "${TMPDIR}/clawz.tar.gz.sha256"

# Extract
tar xzf "${TMPDIR}/clawz.tar.gz" -C "${INSTALL_DIR}"
chmod +x "${INSTALL_DIR}/clawz-agent"

# Cleanup
rm -rf "${TMPDIR}"

echo "ClawZ installed to ${INSTALL_DIR}/clawz-agent"
echo "Run 'clawz-agent --version' to verify"