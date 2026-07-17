#!/usr/bin/env sh
# One-line install:
#   curl -fsSL https://github.com/Dwsy/pi-grok-build/releases/latest/download/install.sh | sh
# Pin a version:
#   curl -fsSL https://github.com/Dwsy/pi-grok-build/releases/download/v0.0.1/install.sh | GROK_PI_VERSION=v0.0.1 sh
# Optional env:
#   GROK_PI_VERSION=v0.0.1
#   GROK_PI_INSTALL_DIR=$HOME/.local/bin
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
      *) fail "macOS $(uname -m) is unsupported; only Apple Silicon (arm64) is released" ;;
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
  *) fail "GROK_PI_VERSION must be 'latest' or a v-prefixed release tag (e.g. v0.0.1)" ;;
esac

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t grok-pi-install)"
cleanup() {
  rm -rf "$tmpdir"
}
trap cleanup EXIT INT HUP TERM

printf '%s\n' "Downloading $asset ($VERSION)..."
curl --fail --location --progress-bar --show-error "$url" -o "$tmpdir/$asset"
tar -xzf "$tmpdir/$asset" -C "$tmpdir"

if [ ! -f "$tmpdir/grok-pi" ]; then
  fail "archive did not contain grok-pi"
fi

mkdir -p "$INSTALL_DIR"
# Prefer install(1) when available so the binary is replaced atomically.
if command -v install >/dev/null 2>&1; then
  install -m 755 "$tmpdir/grok-pi" "$INSTALL_DIR/grok-pi"
else
  cp "$tmpdir/grok-pi" "$INSTALL_DIR/grok-pi"
  chmod 755 "$INSTALL_DIR/grok-pi"
fi

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    printf '%s\n' ""
    printf '%s\n' "Add $INSTALL_DIR to PATH, then open a new terminal:"
    printf '%s\n' "  export PATH=\"$INSTALL_DIR:\$PATH\""
    ;;
esac

printf '%s\n' ""
printf '%s\n' "Installed $INSTALL_DIR/grok-pi"
if "$INSTALL_DIR/grok-pi" --help >/dev/null 2>&1; then
  printf '%s\n' "Binary responds to --help."
fi
printf '%s\n' "Install Pi with: npm install --global @earendil-works/pi-coding-agent"
printf '%s\n' "Run with: grok-pi --pi-bin pi --pi-cwd /path/to/project -- --no-session"
