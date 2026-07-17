#!/usr/bin/env sh
set -eu

REPOSITORY="Dwsy/pi-grok-build"
VERSION="${GROK_PI_VERSION:-latest}"
INSTALL_DIR="${GROK_PI_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
  printf '%s\n' "error: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin)
    case "$(uname -m)" in
      arm64) asset="grok-pi-macos-aarch64.tar.gz" ;;
      *) fail "macOS $(uname -m) is unsupported; only Apple Silicon is released" ;;
    esac
    ;;
  Linux)
    case "$(uname -m)" in
      x86_64) asset="grok-pi-linux-x86_64.tar.gz" ;;
      *) fail "Linux $(uname -m) is unsupported; only x86_64 is released" ;;
    esac
    ;;
  *)
    fail "$(uname -s) is unsupported; use install.ps1 on Windows x64"
    ;;
esac

case "$VERSION" in
  latest) url="https://github.com/$REPOSITORY/releases/latest/download/$asset" ;;
  v*) url="https://github.com/$REPOSITORY/releases/download/$VERSION/$asset" ;;
  *) fail "GROK_PI_VERSION must be 'latest' or a v-prefixed release tag" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

mkdir -p "$INSTALL_DIR"
printf '%s\n' "Installing $asset to $INSTALL_DIR..."
curl --fail --location --silent --show-error "$url" | tar -xz -C "$INSTALL_DIR" grok-pi
chmod +x "$INSTALL_DIR/grok-pi"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf '%s\n' "Add $INSTALL_DIR to PATH, then open a new terminal." ;;
esac

printf '%s\n' "Installed $INSTALL_DIR/grok-pi"
printf '%s\n' "Install Pi with: npm install --global @earendil-works/pi-coding-agent"
printf '%s\n' "Run with: grok-pi --pi-bin pi --pi-cwd /path/to/project -- --no-session"
