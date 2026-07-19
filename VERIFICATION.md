# Grok Native TUI × Pi Verification Report

Verification date: 2026-07-17
Delivered version: `pi-grok-native-v4.0.0`

## Conclusion

The current delivery has passed production build, adapter unit tests, and native Grok architecture plus Pi protocol-contract verification. The entry point genuinely uses Grok Build's production Pager, not a standalone Ratatui/fallback/character-art frontend.

We cannot yet claim **all** verification is green: the Rust syntax stage of `verify.sh` depends on undeclared Python packages `tree_sitter` / `tree_sitter_rust`, and currently stops because that dependency is missing from the environment; two Pager focused lib tests also fail on a pre-existing unrelated Pager test-configuration issue (unrelated to adapter logic). No new `grok-pi` PTY end-to-end smoke test has been added.

## 2026-07-18 Subagent Adaptation Increment

A built-in `pi-grok-subagents` extension was added: it creates, tracks, cancels, and persists a child `AgentSession` using the official Pi extension API, and hands it to the adapter through a `pi-grok-subagent/v1` custom-message bridge. The adapter only validates/dedupes and projects to the Pager-consumed `x.ai/session/update` and child-session-id-tagged ACP `SessionNotification`; the Pager body continues to reuse the existing SubagentBlock, Tasks Pane, child AgentView, and cancel UI.

| Verification layer | Result | Notes |
|---|---:|---|
| Pi custom-message bridge probe | PASS | RPC JSONL `message_start`/`message_end` both preserve `customType`, `display:false`, and structured `details` |
| Tempfile extension load | PASS | Copied the extension to a standalone tempfile and loaded it via `pi --mode rpc --extension <temp>.ts`; the hidden cancel command appears in the command catalog |
| Adapter unit tests | PASS | `cargo test -p pi-grok-adapter`: 53 passing |
| `grok-pi` binary unit tests | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`: 7 passing |
| `grok-pi` check | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` succeeds; only a pre-existing `PiModel.reasoning` dead-code warning |
| Pager child-route lib test | BLOCKED | Focused test compilation blocked by a pre-existing unrelated Pager test config error: missing `set_voice_mode_enabled_for_test`, layout parameter drift, `ActiveModal: Debug`, `AppView` init field drift |
| Native TUI E2E with a real model | PENDING | Manual verification of spawn/progress/child view/finish/cancel/resume/replay is not yet done; static passes must not be treated as runtime acceptance |

## Executed Results

| Verification layer | Result | Notes |
|---|---:|---|
| Native Grok architecture audit | PASS | `grok-pi` lives in `xai-grok-pager-bin` and enters `xai_grok_pager::app::run_external` |
| Self-draw/fallback exclusion | PASS | adapter is library-only, no Ratatui/Crossterm/terminal loop; old `pi-grok-tui` does not exist |
| Grok native source integrity | PASS | 2696 files in the original tree remain SHA-256 identical; only 19 declared composition/ACP/state/command seams changed |
| Renderer/Input/Markdown integrity | PASS | 283 core files are byte-for-byte identical to the uploaded Grok source |
| Pi RPC command contract | PASS | all 13 RPC commands used by the adapter exist in the in-package Pi `rpc-types.ts` |
| Pi event contract | PASS | all 20 mapped lifecycle/stream/tool/queue/compaction/retry/UI event types are locatable in Pi source |
| Extension UI | PASS | all 9 methods exposed by Pi RPC have a native Grok UI route |
| Mock JSONL RPC | PASS | 27 interactions covering bootstrap, history, commands, stream, tool, UI response, and `agent_settled` |
| Rust tree-sitter parsing | BLOCKED | `verify.sh` does not declare or pre-check `tree_sitter` / `tree_sitter_rust`; the module is missing in the current environment |
| Shell script syntax | PASS | `build.sh`, `run-local.sh`, `run-installed.sh`, `verify.sh` pass `bash -n` |
| Patch applicability | PASS | `patch --dry-run -p1` against the uploaded original Grok tree applies cleanly for all 29 source/manifest files |
| `cargo check` | PASS | `cargo check -p xai-grok-pager-bin --bin grok-pi` succeeds; only 1 pre-existing dead-code warning in the adapter |
| Adapter Rust unit tests | PASS | `cargo test -p pi-grok-adapter`: 17 passing |
| `grok-pi` binary unit tests | PASS | `cargo test -p xai-grok-pager-bin --bin grok-pi`: 1 passing |
| Pager focused lib tests | BLOCKED | depends on `xai-grok-pager-render`'s `#[cfg(test)]` helper; the test dependency's test-support feature is not enabled, so compilation fails |
| Local Pi npm build | PASS | `npm run build` succeeded in a Node.js `v24.15.0` environment |

Machine-readable reports:

- `crates/codegen/pi-grok-adapter/docs/native-grok-verification.json`
- `crates/codegen/pi-grok-adapter/docs/mock-pi-contract.json`
- `crates/codegen/pi-grok-adapter/docs/rust-syntax-verification.json`
- `verification-logs/cargo-status.json`
- `verification-logs/environment-status.json`
- `verification-logs/patch-status.json`

## Key Architecture Evidence

### Production Grok Pager Entry Point

`crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs` performs only composition work:

1. Start `pi --mode rpc`;
2. Convert Pi JSONL RPC to ACP;
3. Construct `AcpConnection::external`;
4. Call `xai_grok_pager::app::run_external`.

This file creates no Ratatui `Terminal`, `Frame`, or Widget, and does not read Crossterm input.

### Native Component Reuse

`run_external` continues to use Grok's:

- terminal init/restore and writer thread;
- production event loop;
- PromptWidget and keyboard input;
- slash `CommandRegistry`, suggestion/dropdown;
- Markdown/code/diff/tool rendering;
- scrollback, find, copy, transcript, export;
- QuestionView;
- toast, sticky banner, terminal title;

so every visible terminal surface is Grok Pager, not a second TUI.

### Modification Boundaries

Grok-side changes are limited to:

- adding the external ACP connection/profile;
- gating product features of the external backend;
- Pi Extension UI notifications entering existing Grok surfaces;
- QuestionView gaining `initialText`/`noFreeform` semantic hints;
- merging dynamic Pi commands with allowed Grok builtins;
- `/compact <instructions>` parameter pass-through;

The renderer, input engine, Markdown engine, tool renderer, and minimal renderer bodies are not rewritten.

## Must Run On A Machine With The Toolchain

Requirements:

- Rust toolchain `1.92.0` (see `rust-toolchain.toml`);
- Node.js `22.19.0` or higher;
- Python 3 (for verification scripts);
- workspace dependencies installable.

Run:

```bash
./build.sh
cargo test -p pi-grok-adapter
cargo test -p xai-grok-pager-bin --bin grok-pi
cargo check -p xai-grok-pager-bin --bin grok-pi
```

Or run step by step:

```bash
./build.sh
cargo test -p pi-grok-adapter
cargo test -p xai-grok-pager-bin --bin grok-pi
cargo check -p xai-grok-pager-bin --bin grok-pi
```

Then build the full run chain:

```bash
./build.sh
```

## Runtime Acceptance Checklist

After a successful build, manually verify at least:

1. The screen, PromptWidget, command dropdown, Markdown, and tool cards match Grok Build Pager;
2. `/help` shows only allowed Grok local commands, merged with Pi dynamic commands;
3. Pi extension `notify`/`setStatus` no longer produce fallback text messages;
4. `select`, `confirm`, `input`, `editor` use the Grok QuestionView;
5. `/model` and `/effort` actually change the Pi model/thinking level;
6. a normal submission during the active turn enters Pi follow-up, send-now enters steer;
7. `!command` uses the Pi `bash` RPC and renders as a Grok tool card;
8. `/new`, `/compact instructions`, `/rename` take effect;
9. restarting an existing Pi session restores history, reasoning, images, and tool results;
10. minimal/fullscreen is selected via startup arguments, and the terminal restores correctly on exit.

## Upstream Integration Record

Date: 2026-07-17
Branch: `sync/upstream-98c3b24` (not yet merged back to `main`)

| Item | Result |
|---|---|
| Upstream tip | `98c3b24` (includes `8adf901`) |
| Strategy | Git merge with a common ancestor `c68e39f` plus seam fixes, **not** a blind merge onto main |
| `grok-pi` unit tests | 4/5 PASS; 1 item `--append-system-prompt` naming drift is a pre-existing main failure |
| Architecture invariants | adapter headless; Pager is the only TUI; Pi is the only core |

Known remaining infrastructure blockers are in the `verify.sh` / Pager focused lib tests sections above.
