#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PI_ROOT="$ROOT/pi-main"
GROK_ROOT="$ROOT"

if ! command -v node >/dev/null 2>&1; then
  echo "error: Node.js is required" >&2
  exit 1
fi
if ! command -v npm >/dev/null 2>&1; then
  echo "error: npm is required" >&2
  exit 1
fi
if ! node -e '
const [major, minor] = process.versions.node.split(".").map(Number);
process.exit(major > 22 || (major === 22 && minor >= 19) ? 0 : 1);
'; then
  echo "error: Pi requires Node.js >= 22.19.0; found $(node --version)" >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust/Cargo is required" >&2
  exit 1
fi

if [[ ! -d "$PI_ROOT/node_modules" ]]; then
  (cd "$PI_ROOT" && npm install)
fi
(cd "$PI_ROOT" && npm run build)
(cd "$GROK_ROOT" && cargo build -p xai-grok-pager-bin --bin grok-pi)

echo "Built: $GROK_ROOT/target/debug/grok-pi"
