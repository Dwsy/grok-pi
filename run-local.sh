#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
BIN="${GROK_PI_BIN:-$GROK_ROOT/target/debug/grok-pi}"
# Prefer the repo-bundled Pi (pi-main). System `pi` still works for extension-host
# Remote TUI (no Pi source patch required).
BUNDLED_PI="$GROK_ROOT/pi-main/packages/coding-agent/dist/cli.js"
if [[ -n "${PI_BIN:-}" ]]; then
  :
elif [[ -x "$BUNDLED_PI" || -f "$BUNDLED_PI" ]]; then
  PI_BIN="$BUNDLED_PI"
else
  PI_BIN="pi"
fi

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <project-directory> [Pi arguments...]" >&2
  exit 2
fi
PROJECT_DIR="$(cd "$1" && pwd)"
shift

if [[ ! -x "$BIN" ]]; then
  echo "error: grok-pi is not built: $BIN" >&2
  echo "run ./build.sh first" >&2
  exit 1
fi
if [[ ! -e "$PI_BIN" ]] && ! command -v "$PI_BIN" >/dev/null 2>&1; then
  echo "error: Pi executable not found: $PI_BIN" >&2
  exit 1
fi

# Remote TUI / Bash default ON inside grok-pi (disable with =0).
# Shell-level unset is fine; binary treats missing as enabled.
if [[ "${PI_GROK_REMOTE_TUI:-1}" != "0" ]]; then
  if [[ ! -f "$GROK_ROOT/extensions/pi-grok-remote-tui/index.ts" ]]; then
    echo "warning: Remote TUI enabled but extensions/pi-grok-remote-tui/index.ts missing" >&2
  else
    echo "Remote TUI: ON (extension host; set PI_GROK_REMOTE_TUI=0 to disable) PI_BIN=$PI_BIN" >&2
  fi
fi

ui_args=()
[[ "${GROK_PI_MINIMAL:-0}" == "1" ]] && ui_args+=(--minimal)
[[ "${GROK_PI_FULLSCREEN:-0}" == "1" ]] && ui_args+=(--fullscreen)
[[ "${GROK_PI_NO_ALT_SCREEN:-0}" == "1" ]] && ui_args+=(--no-alt-screen)

exec "$BIN" \
  --pi-bin "$PI_BIN" \
  --pi-cwd "$PROJECT_DIR" \
  "${ui_args[@]}" \
  -- "$@"
