#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
# Default host: system `pi` (min 0.80.10). Optional: PI_BIN=/path/to/cli.js
PI_BIN="${PI_BIN:-pi}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust/Cargo is required" >&2
  exit 1
fi
if [[ ! -e "$PI_BIN" ]] && ! command -v "$PI_BIN" >/dev/null 2>&1; then
  echo "error: Pi executable not found: $PI_BIN" >&2
  echo "install Pi >= 0.80.10: npm i -g @earendil-works/pi-coding-agent" >&2
  exit 1
fi

"$ROOT/scripts/setup-shared-cargo-target.sh"

# Optional: rebuild submodule pi-main coding-agent when present (dev only).
if [[ -f "$GROK_ROOT/pi-main/packages/coding-agent/package.json" ]]; then
  echo "Building pi-main coding-agent (submodule, optional)..."
  (cd "$GROK_ROOT/pi-main/packages/coding-agent" && npm run build)
fi

(cd "$GROK_ROOT" && cargo build -p xai-grok-pager-bin --bin grok-pi)

echo "Built: $GROK_ROOT/target/debug/grok-pi"
echo "Pi:    $PI_BIN (min compatible 0.80.10)"
