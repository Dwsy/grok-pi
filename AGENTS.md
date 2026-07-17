# AGENTS.md — Pi-Grok Native TUI

## Project root and Git

`grok-build-main/` is the project root and the **only** Git working tree.

```text
origin   https://github.com/Dwsy/pi-grok-build.git
upstream https://github.com/xai-org/grok-build.git
base     c68e39f60462f28d9be5e683d9cbe2c57b1a5027
```

- Work from this directory; do not use its parent wrapper as a repository.
- `origin/main` is the Pi-Grok integration branch; `upstream` is read-only Grok Build.
- Never directly merge an upstream root commit into this repository without a migration plan. Reapply the narrow integration seams and validate them instead.
- Keep commits focused. Do not stage generated `pi-main` model catalog changes unless they are intentional.
- The bundled Pi source has its own guidance at [`pi-main/AGENTS.md`](pi-main/AGENTS.md); follow it for all files below `pi-main/`.

## Architecture invariants

1. **Grok Pager is the only terminal UI.** All visible terminal surfaces must come from `xai-grok-pager` or native component crates.
2. **Pi is the only agent core.** Pi owns models, providers, agent loop, tools, extensions, compaction, retries, and sessions.
3. **`pi-grok-adapter` is headless and library-only.** It may translate JSONL RPC ↔ ACP, but must not render widgets, own a terminal, read keyboard events, or depend on Ratatui/Crossterm.
4. **Reuse native Grok surfaces.** Map Pi capabilities to existing Pager prompt, slash, QuestionView, toast, banner, tool card, diff, and scrollback surfaces. Do not create a second TUI or ASCII fallback UI.
5. **Do not modify Pi source to extend RPC.** When a Pi core capability is not exposed over RPC, prefer the official extension API. Preserve Pi semantics rather than emulating them with JSONL edits or unrelated RPCs.

Read [`NATIVE_GROK_TUI_ALIGNMENT.md`](NATIVE_GROK_TUI_ALIGNMENT.md) and [`FEATURE_MATRIX.md`](FEATURE_MATRIX.md) before changing protocol or UI behavior.

## Important paths

| Concern | Path |
|---|---|
| Composition binary | `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` |
| Pi JSONL RPC transport | `crates/codegen/pi-grok-adapter/src/pi_rpc.rs` |
| Pi data parsers | `crates/codegen/pi-grok-adapter/src/model.rs` |
| ACP adapter and UI mapping | `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` |
| Native Pager external-profile seams | `crates/codegen/xai-grok-pager/src/app/` |
| Bundled Pi RPC facts | `pi-main/packages/coding-agent/src/modes/rpc/` |
| Pi session lifecycle facts | `pi-main/packages/coding-agent/src/core/agent-session.ts` |
| Architecture and task records | `docs/` |

## Session and tree rules

- Pi owns session files, trees, and the active leaf.
- `/resume` must use the native Grok `SessionPicker`; catalog scanning is on-demand, never startup work.
- Respect Pi's default session root, `--session-dir`, `PI_CODING_AGENT_SESSION_DIR`, and `sessionFile`-derived custom directories.
- `navigateTree()` changes Pi's in-memory leaf and context. Do not fake it with `fork`, `switch_session`, or direct JSONL mutation.
- If tree navigation is added without changing Pi source, bridge the official Pi extension API (`ctx.navigateTree`) and render only with native Grok components.

## Build and verification

Run from the project root:

```bash
./build.sh
cargo test -p pi-grok-adapter
cargo test -p xai-grok-pager-bin --bin grok-pi
cargo check -p xai-grok-pager-bin --bin grok-pi
```

`./build.sh` builds bundled Pi and `grok-pi` together. It requires Node.js >= 22.19.0 and the repository Rust toolchain.

`./verify.sh` additionally runs architecture, mock, syntax, and Pager checks. Current known infrastructure blockers are documented in [`VERIFICATION.md`](VERIFICATION.md): Python tree-sitter dependencies are not provisioned, and Pager focused lib tests have an upstream cross-crate test-helper configuration issue. Do not claim full verification is green unless those blockers are resolved.

Before reporting completion:

1. Run the narrowest relevant tests and build/check.
2. Read the exit status and complete output.
3. Review the diff for scope and whitespace errors.
4. State known blockers separately from passing checks.

## Documentation and change control

- Complex work must have a record in `docs/issues/` before implementation.
- Update the relevant Issue after each completed phase.
- Keep `README.md`, `README.zh-CN.md`, `FEATURE_MATRIX.md`, and `VERIFICATION.md` aligned with actual behavior.
- The source-identity verifier allows only declared Pager seams. If a required native seam changes, update its baseline/allowed-seam metadata deliberately; never weaken the verifier broadly.

## Safety

- Do not run destructive Git commands (`reset --hard`, broad restore, force checkout).
- Do not remove source files with `rm`; use `trash` for intentional deletion.
- Do not push, rename, delete, or recreate remote repositories unless the user explicitly authorizes it.
- Treat credentials, tokens, and user session data as private.
