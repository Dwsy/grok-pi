---
id: "2026-07-26-add-built-in-herdr-lifecycle-integration"
title: "Add built-in Herdr lifecycle integration"
status: "completed"
created: "2026-07-26"
updated: "2026-07-26"
category: "grok-pi"
tags: ["workhub", "grok-pi", "herdr", "integration", "f2"]
---

# Issue: Add built-in Herdr lifecycle integration

> Historical / superseded — do not implement this issue's default-on policy. The current opt-in policy is tracked in [`docs/issues/note.md`](../note.md).

## Goal

Make `grok-pi` report authoritative Pi agent lifecycle and native session state to Herdr without requiring Herdr's global stock-Pi integration. Keep the bridge enabled by default and restart-disableable through F2.

## Background

`grok-pi` disables Pi resource auto-discovery and passes policy-approved resources explicitly. Herdr's managed stock-Pi extension can therefore be absent, blocked, or stale. The host already materializes trusted built-in TypeScript extensions as temporary files, so lifecycle reporting belongs in that host-owned injection chain.

Herdr 0.7.5 exposes a local newline-delimited JSON socket through `HERDR_SOCKET_PATH`, with the active pane in `HERDR_PANE_ID`. The current Pi integration contract reports session identity through `pane.report_agent_session` and lifecycle through `pane.report_agent`. Herdr grants full lifecycle authority only to the exact pair `source = "herdr:pi"`, `agent = "pi"`.

## Acceptance Criteria

- [x] With bridge extensions enabled and `[ui].pi_herdr` unset or `true`, `grok-pi` injects a built-in Herdr lifecycle extension.
- [x] Outside Herdr, or without the required `HERDR_*` variables, the extension is a silent no-op.
- [x] Root interactive Pi sessions report ordered native session identity plus `working`, `blocked`, and `idle` state without false release during reload/new/resume/fork transitions.
- [x] F2 exposes external-only, restart-required **Pi Herdr integration**, defaulting to on and persisting to `[ui].pi_herdr`.
- [x] `[ui].pi_herdr = false` and the normal `grok-pi --no-extensions` path skip the built-in bridge for the next process.
- [x] When the built-in bridge is active, only Herdr's auto-discovered managed Pi file is skipped; explicit user `--extension` arguments remain untouched.
- [x] Existing uncommitted work is preserved.
- [x] Targeted Rust tests, a fake-socket lifecycle smoke test, product build checks, and bilingual user guidance are included.

## Implementation

- [x] Added `extensions/pi-grok-herdr/index.ts` with fail-closed socket delivery, retry, session-before-state ordering, root-session filtering, blocked precedence, and settled-idle semantics.
- [x] Added `grok_pi/herdr_extension.rs` to materialize the trusted temporary extension and identify only Herdr-managed stock-Pi files.
- [x] Injected the bridge by default from `grok-pi` and filtered the managed auto-discovered duplicate.
- [x] Added `UiConfig.pi_herdr` with serde default-on behavior.
- [x] Added F2 metadata, typed action, setter, persistence, reset, rollback, and settings-test move-away support.
- [x] Added English and Chinese setup/troubleshooting guides and linked them from the corresponding READMEs.

## Key Decisions

| Decision | Rationale |
|---|---|
| Use a built-in temporary Pi extension | Matches existing trusted host bridges and works with policy-controlled Pi resource loading. |
| Default `[ui].pi_herdr` to `true` | Provides zero-setup behavior; the extension is inert outside Herdr. |
| Keep the F2 setting restart-required | Extension admission occurs before the Pi child is spawned. |
| Use `source = "herdr:pi"`, `agent = "pi"` | Herdr grants full lifecycle authority only to this exact pair. |
| Suppress only the managed auto-discovered duplicate | Prevents two authoritative writers while preserving explicit user extensions and stock Pi's global integration. |
| Do not emit `pane.release_agent` | Current Herdr Pi v7 semantics let process/pane lifecycle own teardown and prevent false release during session replacement. |
| Implement the public socket contract independently | Avoids coupling `grok-pi` to Herdr's managed extension file layout. |

## Validation Evidence

- [x] `node --test extensions/pi-grok-herdr/index.test.mjs` — 2 passed.
- [x] `cargo test -p xai-grok-pager-bin --bin grok-pi herdr -- --nocapture` — 3 passed.
- [x] `cargo test -p xai-grok-pager-bin --bin grok-pi -- --test-threads=1` — passed before its existing cleanup restored the working-tree snapshot; final files were re-applied and revalidated with the safe filtered target.
- [x] `cargo test -p xai-grok-shared ui_config -- --nocapture` — 5 passed.
- [x] `cargo check -p xai-grok-shared -p xai-grok-shell -p xai-grok-pager -p xai-grok-pager-bin --bin grok-pi` — 0 errors.
- [x] `cargo build -p xai-grok-pager-bin --bin grok-pi` and `./target/debug/grok-pi --help` — passed.
- [x] `git diff --check` over all feature files — passed.

## Existing Repository Blockers

| Blocker | Handling |
|---|---|
| Full-workspace `cargo fmt --all -- --check` reports unrelated formatting drift across the already-dirty tree. | Formatted the new Rust module directly and used feature-only `git diff --check`; unrelated files were not rewritten. |
| `xai-grok-pager` test targets currently fail to compile because the pre-existing PTY harness has duplicate `wait_for_text`, missing `PtyPump`/`pump_one`, and related errors. | Recorded the blocker and validated the normal product build plus the exact `grok-pi` test target instead. |
| Parallel `grok-pi` tests race on the existing `PI_GROK_NATIVE_COMMANDS` environment mutation. | Re-ran the complete target with `--test-threads=1`; it passed without changing unrelated tests. |
| Aggregate `xai-grok-pager-bin` tests also hit an unrelated missing `Command::Leader` match in the other binary. | Used the exact `--bin grok-pi` target. |
| The complete `grok-pi` test target restores a saved working-tree snapshot on completion, deleting untracked feature files and reverting tracked task edits. | Re-applied the exact unique-anchor patch after that run; final verification used only the safe Herdr-filtered target, Node socket tests, `cargo check`, and static audit. |

## Related Files

- `extensions/pi-grok-herdr/index.ts`
- `extensions/pi-grok-herdr/index.test.mjs`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok_pi/herdr_extension.rs`
- `crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs`
- `crates/codegen/xai-grok-shared/src/ui_config.rs`
- `crates/codegen/xai-grok-pager/src/settings/`
- `docs/usage/grok-pi-herdr.md`
- `docs/usage/grok-pi-herdr.zh-CN.md`

## Status Log

- **2026-07-26 19:08 JST**: Research completed; Herdr guide, v7 integration contract, authority rules, and host injection/F2 seams mapped.
- **2026-07-26 19:33 JST**: Status → completed. Implementation, documentation, focused tests, product build, and final feature audit passed; unrelated dirty-tree blockers recorded above.
