#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GROK_ROOT="$ROOT"
PI_ROOT="$ROOT/pi-main"
ADAPTER="$GROK_ROOT/crates/codegen/pi-grok-adapter"
LOG_DIR="$ROOT/verification-logs"
mkdir -p "$LOG_DIR"

python3 "$ADAPTER/scripts/verify_native_grok.py" \
  --workspace "$GROK_ROOT" \
  --pi-source "$PI_ROOT" \
  --json-out "$ADAPTER/docs/native-grok-verification.json" \
  | tee "$LOG_DIR/native-grok-verification.log"

python3 "$ADAPTER/tests/mock_pi_contract.py" \
  --pi-source "$PI_ROOT" \
  | tee "$LOG_DIR/mock-pi-contract.log"

python3 "$ADAPTER/scripts/check_rust_syntax.py" \
  --workspace "$GROK_ROOT" \
  --json-out "$ADAPTER/docs/rust-syntax-verification.json" \
  | tee "$LOG_DIR/rust-syntax-verification.log"

if ! command -v cargo >/dev/null 2>&1; then
  cat > "$LOG_DIR/cargo-status.json" <<JSON
{
  "status": "NOT_RUN",
  "reason": "cargo is not installed in this environment",
  "commands": [
    "cargo check -p xai-grok-pager-bin --bin grok-pi",
    "cargo test -p pi-grok-adapter",
    "cargo test -p xai-grok-pager --lib external_builtin_filter_accepts_aliases_and_omits_product_commands",
    "cargo test -p xai-grok-pager --lib slash_compact_with_context_enqueues_command"
  ]
}
JSON
  echo "Static/protocol/mock/syntax verification passed; Cargo verification NOT RUN." >&2
  exit 2
fi

(
  cd "$GROK_ROOT"
  cargo check -p xai-grok-pager-bin --bin grok-pi
  cargo test -p pi-grok-adapter
  cargo test -p xai-grok-pager --lib external_builtin_filter_accepts_aliases_and_omits_product_commands
  cargo test -p xai-grok-pager --lib slash_compact_with_context_enqueues_command
) 2>&1 | tee "$LOG_DIR/cargo-verification.log"

cat > "$LOG_DIR/cargo-status.json" <<JSON
{
  "status": "PASS"
}
JSON

echo "All verification passed."
