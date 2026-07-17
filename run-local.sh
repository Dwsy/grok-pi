#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
PI_CLI="$ROOT/pi-main/packages/coding-agent/dist/cli.js"
BIN="${GROK_PI_BIN:-$GROK_ROOT/target/debug/grok-pi}"

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <project-directory> [Pi arguments...]" >&2
  exit 2
fi
PROJECT_DIR="$(cd "$1" && pwd)"
shift

if [[ ! -f "$PI_CLI" ]]; then
  echo "error: local Pi is not built: $PI_CLI" >&2
  echo "run ./build.sh first" >&2
  exit 1
fi
if [[ ! -x "$BIN" ]]; then
  echo "error: grok-pi is not built: $BIN" >&2
  echo "run ./build.sh first" >&2
  exit 1
fi

ui_args=()
[[ "${GROK_PI_MINIMAL:-0}" == "1" ]] && ui_args+=(--minimal)
[[ "${GROK_PI_FULLSCREEN:-0}" == "1" ]] && ui_args+=(--fullscreen)
[[ "${GROK_PI_NO_ALT_SCREEN:-0}" == "1" ]] && ui_args+=(--no-alt-screen)

exec "$BIN" \
  --pi-bin node \
  --pi-prefix-arg "$PI_CLI" \
  --pi-cwd "$PROJECT_DIR" \
  "${ui_args[@]}" \
  -- "$@"
