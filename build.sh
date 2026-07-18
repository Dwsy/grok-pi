#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
BUNDLED_PI="$GROK_ROOT/pi-main/packages/coding-agent/dist/cli.js"
PI_BIN="${PI_BIN:-}"
if [[ -z "$PI_BIN" ]]; then
  if [[ -f "$BUNDLED_PI" ]]; then
    PI_BIN="$BUNDLED_PI"
  else
    PI_BIN="pi"
  fi
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust/Cargo is required" >&2
  exit 1
fi
if [[ ! -e "$PI_BIN" ]] && ! command -v "$PI_BIN" >/dev/null 2>&1; then
  echo "error: Pi executable not found: $PI_BIN" >&2
  exit 1
fi

# Rebuild bundled Pi when sources are present (picks up experimental RPC patches).
if [[ -f "$GROK_ROOT/pi-main/packages/coding-agent/package.json" ]]; then
  echo "Building bundled Pi coding-agent..."
  (cd "$GROK_ROOT/pi-main/packages/coding-agent" && npm run build)
fi

(cd "$GROK_ROOT" && cargo build -p xai-grok-pager-bin --bin grok-pi)

echo "Built: $GROK_ROOT/target/debug/grok-pi"
echo "Pi:    $PI_BIN"
