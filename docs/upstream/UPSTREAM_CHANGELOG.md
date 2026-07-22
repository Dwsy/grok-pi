# Upstream Changelog

Changelog of upstream **Grok Build** (`xai-org/grok-build`) changes absorbed by
this fork (`Dwsy/grok-pi`). This is the **upstream update record**: it lists what
upstream changed and which features were affected, so each sync can be reviewed
before and after the merge.

> [!NOTE]
> Upstream commits are titled `Synced from monorepo` but each carries a full
> **`Changes:`** bullet list and a **`Source-Revision:`** trailer in its message
> body. Feature descriptions below are **transcribed from those commit messages**
> (the authoritative source). Diff analysis is used only to fill the Areas-touched
> statistics and to derive descriptions for the rare commit that lacks a
> `Changes:` list.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Entries are **newest first**. This file is maintained by the
[`upstream-changelog`](../../.pi/skills/upstream-changelog/SKILL.md) skill.

## Entry schema

Each entry records:

| Field | Meaning |
|---|---|
| Upstream tip | Full upstream commit SHA being synced to |
| Range | `FROM..TO` git range (`merge-base..upstream-tip`) |
| SOURCE_REV | Monorepo revision from the `Source-Revision:` trailer / `SOURCE_REV` file at the upstream tip |
| Date | Date the record was generated (YYYY-MM-DD) |
| Stats | Files changed, insertions(+), deletions(−) |
| Added / Changed / Fixed | Feature bullets transcribed from upstream commit `Changes:` lists |
| Areas touched | Per-crate/area change statistics table (from `git diff --numstat`) |

<!-- entries below this line -->

## [3af4d5d] — 2026-07-22

> **Status:** Merged into grok-pi (branch `sync/upstream-3af4d5d` @ `a5ffbcb`, pending merge back to `main`).

- **Sync range:** `a881e67..3af4d5d` (`a881e6703f46b01d8c7d4a5437683546df30449d` → `3af4d5d39897855bdcc74f23e690024a5dc05573`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `0f4d7c91b8b2b408333f6de1e8a76cb8eaa71899` (was `c5c4ce03436b4bb2cec43d3feaa27dee0109bf37`)
- **Diff size:** 556 files changed, +56609 / −21892

### Summary

Large monorepo sync dominated by a brand-new **workflow engine** crate
(`xai-workflow`), a major **permission/security overhaul** in
`xai-grok-workspace` (exec-risk scoring, auto-mode, hardened shell access), and
extensive **Shell** and **Pager** changes (working-directory relocation, model
providers, doctor diagnostics, prompt-queue batching). Multiple security fixes
close RCE and credential-plugin attack vectors.

### Added

- Workflow: new `xai-workflow` crate — durable workflow execution engine with journaling, metadata, validation, and host interface
- Workflow authoring skills: `create-workflow` and `import-claude-workflow` docs
- Worktree: kind-aware auto-GC TTLs and config knobs
- Worktree: macOS process CWD scan and Unix PID liveness for GC guards
- Worktree: automatic throttled GC on startup (Linux age-based; non-Linux dead-only)
- Pager: `[ui].combine_queued_prompts` config to batch queued follow-ups
- Pager: expose `doctor` in the TUI
- Pager: edit minimal prompts in an external editor
- Shell: working-directory relocation state primitives and storage primitives
- Shell: resume sessions when the working directory moves
- Shell: `max` as a distinct reasoning effort tier
- Shell: model providers
- Shell: attach author identity to feedback when the deployment opts in
- Tools: scheduler lifecycle version clock
- Proto: `ClientToolResult` and `ChatConfig` client-side tools
- `/usage` shows per-session token and dollar usage in the TUI
- Voice: diagnose silent-mic failures (macOS permission) and add doctor/terminal-setup Voice section
- App builder deployer: `allow_forking` and `show_built_with_grok`
- Doctor: read-only `grok doctor` command

### Changed

- Shell: accept target response id on rewind execute
- Shell: stamp response id on chat user message chunks
- Shell: give side model calls their own conversation ids
- Shell: recap rides the parent turn's prompt cache
- Worktree: optional rebuild and stale git registration cleanup in auto-GC
- Tools: read markdown in `skills/` directories untruncated
- Tools: serialize background `/loop` fires on the whole work unit
- Pager: idle watcher cue — "1 subagent still running" instead of "watching · 1 subagent"
- Pager: make actions screen-mode aware
- Pager: centralize terminal diagnostics and probes
- Pager: standardize backgrounding on Ctrl+B
- Chat: select App Builder product on the Build path
- Sandbox: apply Landlock without a controlling TTY
- Workspace: gate inline shell file access

### Fixed

- Shell: stop overwriting user skills
- Security: prompt on environment-dumping `ps` variants
- Security: `kubectl` no longer runs arbitrary kubeconfig credential plugins without permission
- Security: peel `env -S` / `--split-string` operands in the Bash permission gate (managed deny/ask)
- Security: block unauthorized RCE via abused safe commands
- Security: block `rg --pre` arbitrary code execution in auto-mode
- Tools: make scheduler deletion durable
- Workflow: fix five workflow-runtime bugs (budget, pause, cancel, reconnect)
- Pager: stop stacking duplicate "Worked for" markers on parked turns
- Pager: recover image paste over grok wrap on headless remotes
- Doctor: fix for SSH wrap setup

### Areas touched

| Area | Files | +/− | Notes |
|------|------:|----:|-------|
| Shell (agent runtime) | 167 | +19642/−16719 | relocation, model providers, reasoning tiers, recap caching |
| Pager (TUI) | 266 | +19117/−4076 | doctor, prompt combine, external editor, diagnostics, Ctrl+B |
| Workspace / Permission | 14 | +3693/−225 | exec-risk scoring, auto-mode, shell access hardening |
| Worktree / GC | 7 | +3774/−127 | auto-GC TTLs, PID liveness, startup GC |
| Workflow (new crate) | 9 | +3174/−0 | durable workflow engine + journaling + validation |
| Config | 9 | +2847/−3 | new config types for workflow/GC knobs |
| Tools | 27 | +1989/−309 | scheduler durability, `/loop` serialization, skills reading |
| Chat state | 9 | +619/−29 | App Builder product selection |
| Pager render | 9 | +553/−85 | rendering updates |
| Pager PTY harness | 9 | +431/−94 | test harness updates |
| Voice | 8 | +315/−55 | silent-mic diagnostics, PCM processing |
| Sampler / Sampling types | 7 | +444/−74 | model provider plumbing |
| Prompt queue | 4 | +301/−4 | `combine_queued_prompts` batching |
| Sandbox | 2 | +121/−4 | Landlock without controlling TTY |
| Test support | 5 | +167/−113 | test infrastructure |
| Shared | 2 | +165/−65 | shared utilities |
| Subagent resolution | 2 | +41/−16 | subagent updates |
| Agent lifecycle | 2 | +31/−4 | agent identity |
| Shell base | 1 | +15/−15 | shell base updates |
| Hunk tracker | 1 | +13/−10 | file utils |
| Plugin marketplace | 1 | +12/−8 | marketplace updates |
| Tools API | 2 | +10/−8 | tool API updates |
| Tool runtime / protocol | 3 | +11/−18 | identifier validation, error conversion |
| Computer Hub | 2 | +9/−10 | notification, bridge |
| Textarea | 2 | +4/−2 | minor textarea adjustments |
| Markdown | 1 | +3/−6 | markdown updates |
| MCP | 1 | +3/−3 | MCP updates |
| Hooks | 1 | +1/−2 | hook updates |
| Memory | 1 | +1/−2 | memory updates |
| Version | 1 | +1/−1 | version bump |
| Root / meta | 3 | +116/−10 | Cargo.toml, Cargo.lock, SOURCE_REV |
| **Total** | **556** | **+56609/−21892** | |

### Merge risk for grok-pi

- **High:** `xai-grok-workspace/permission/` — exec-risk scoring, auto-mode, and shell-access hardening overlap with Pi-Grok's bash tool bridging and trust model. Review carefully during merge.
- **High:** `xai-grok-shell` (167 files, +19642/−16719) — massive churn in the agent runtime; relocation primitives, model providers, and reasoning tiers may shift APIs the adapter depends on.
- **Medium:** `xai-grok-pager` (266 files) — doctor, prompt combine, external editor, and diagnostics touch Pager surfaces that Pi-Grok maps to native components.
- **Low:** `xai-workflow` is a new isolated crate; `xai-prompt-queue/combine` is additive; voice/config changes are self-contained.
