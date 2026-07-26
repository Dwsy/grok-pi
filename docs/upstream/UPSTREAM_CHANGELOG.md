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

## [47348d1] — 2026-07-26

> **Status:** Pending — not yet merged into grok-pi.

- **Sync range:** `6e38642..47348d1` (`6e386420825bd44ae648c63e7c8cba12fcec9401` → `47348d13ec4508dcfe440e34c6d511bb02998fb2`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `d02693a856a54f1030695b36b91d276e96b30b23` (was `9b8d35b46d959c042ea9aa31cbbebbd1f0c5c527`)
- **Diff size:** 138 files changed, +7283 / −5796

### Summary

This sync is dominated by Pager and Shell reliability changes plus workspace-security, managed-config signing, and hook configuration updates. It makes startup/runtime failures recoverable, preserves completed terminal output across gateway loss, tightens workspace file confinement and hook-root approval boundaries, and changes lifecycle behavior around session termination. Pager `app/`, session visibility, inline/freeform input, task output, hooks, config paths, and external-agent integration are high-risk Pi-Grok seam areas and must be merged in isolation.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 30 | +1891/−2608 | 5/1 | recoverable HTTP/runtime/session failures and terminal transport behavior |
| Pager (TUI) | 69 | +2181/−2148 | 1/0 | task details, paste parity, session visibility, status-marker rendering |
| Workspace / Permission | 16 | +1334/−325 | 2/0 | workspace file confinement and `acceptEdits` hook-root security |
| Config | 9 | +1039/−138 | 0/0 | signing key and managed-config verification controls |
| Hooks / Plugins | 7 | +722/−414 | 0/0 | config-file hook parsing and SessionEnd behavior |
| Agent lifecycle | 2 | +40/−148 | 0/0 | recoverable session thread/runtime spawning |
| Tools | 1 | +53/−6 | 0/0 | supporting tool behavior |
| Other crates | 1 | +16/−3 | 0/0 | supporting shared crate changes |
| Root / meta | 2 | +6/−5 | 0/0 | lockfile and SOURCE_REV |
| Update / Version | 1 | +1/−1 | 0/0 | version metadata |
| **Total** | **138** | **+7283/−5796** | **8/1** | |

### Added

- Raise the Linux file-descriptor soft limit and log effective limits at startup.
- Embed the deployment-config signing public key.
- Parse hooks from configuration files.
- Add a remote kill-switch for managed-config signature verification.

### Changed

- Keep completed terminal output when the gateway connection is lost.
- Show duration-only detail for single-task task output.
- Prevent a stale registry turn counter from hiding local sessions.
- Make HTTP client construction failures non-fatal.
- Make session-thread and runtime-spawn failures recoverable.
- Fire `SessionEnd` hooks on `/exit` and headless quit.

### Fixed

- Report invalid MCP server configuration instead of failing startup.
- Restore main-prompt paste parity in the question freeform input.
- Repaint paste-chip backgrounds on inline panel inputs.
- Security: prevent `acceptEdits` from auto-approving writes into the always-trusted global hook root.
- Render stacked “Worked for” markers correctly so parks appear as status and turns close with exactly one marker.
- Security: keep workspace file-reference resolution inside workspace filesystem confinement.

### Merge risk for grok-pi

- Pager changes span 69 files and overlap Pi-Grok `app/`, modal/input, task-output, session and external-profile seams.
- Hook/config changes must preserve `.grok-pi` product isolation and `project_config_dir()` routing while absorbing upstream security fixes.
- Runtime recovery and gateway-loss behavior must not transfer agent/session ownership away from Pi or make the adapter stateful/UI-owning.
- Managed-config signing changes may alter verifier baselines and root metadata; `SOURCE_REV`, `AGENTS.md` base and baselines change only after a verified merge.

## [6e38642] — 2026-07-25

> **Status:** Merged into grok-pi `main` by ff-only through verified integration tip `92b7c3d` (two-parent upstream merge `963ccf5`).

- **Sync range:** `a5727c5..6e38642` (`a5727c5960452e7527a154b25cb5bf00cda0545e` → `6e386420825bd44ae648c63e7c8cba12fcec9401`)
- **Upstream commits:** 2 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `9b8d35b46d959c042ea9aa31cbbebbd1f0c5c527` (was `30192d2eef5d91a8fff0e53957de5bd05b43398c`)
- **Diff size:** 349 files changed, +27899 / −10881

### Summary

Large sync dominated by Pager and Shell changes: title-based resume, queue editing, tutorial and privacy surfaces, auth/provider hardening, true-noop turn handling, workflow recovery, and expanded tool/workspace behavior. Pager `app/`, session, queue, voice, settings, workflow overlay, and Shell ACP/auth paths overlap heavily with Pi-Grok integration seams, so the merge must remain isolated and preserve the fork's external-agent and Pi-owned runtime boundaries.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 111 | +6726/−7519 | 4/2 | auth refresh, provider gateways, turn stop/origin, resumable workflows |
| Pager (TUI) | 130 | +9666/−1587 | 16/0 | resume, queue edit, tutorial/privacy, voice and workflow overlay |
| Tools | 40 | +5623/−887 | 7/0 | managed catalog refresh and tools-server callback surface |
| Sandbox | 8 | +2103/−262 | 2/0 | persistent hook-source protection and deny-path hardening |
| Config | 8 | +1020/−2 | 2/0 | global hook sources and managed configuration |
| Workspace / Permission | 10 | +915/−63 | 0/0 | readiness failure reporting and fail-closed policy behavior |
| Update / Version | 5 | +349/−436 | 1/1 | soft and required CLI version checks |
| Agent lifecycle | 6 | +464/−28 | 1/0 | agent/session metadata and lifecycle changes |
| Models / Sampling | 7 | +338/−29 | 1/0 | image/session metadata and default web-search model |
| Other | 6 | +307/−5 | 0/0 | documentation and supporting project assets |
| Workflow | 2 | +183/−2 | 0/0 | scratch quotas and failed-run resume support |
| Other crates | 5 | +85/−16 | 0/0 | shared support changes outside mapped areas |
| Root / meta | 3 | +27/−31 | 0/0 | workspace metadata, lockfile, and SOURCE_REV |
| Hooks / Plugins | 2 | +44/−14 | 0/0 | marketplace URL validation and hook discovery |
| Chat state | 4 | +20/−0 | 0/0 | deploy-state and turn metadata plumbing |
| Telemetry / Mixpanel | 1 | +16/−0 | 0/0 | gateway lifecycle telemetry |
| Voice | 1 | +13/−0 | 0/0 | interim text submission and editing behavior |
| **Total** | **349** | **+27899/−10881** | **34/3** | |

### Added

- ACP terminal output recorder
- Cross-platform provider-auth commands in the Shell
- Custom provider gateways and subprocess-environment policy in the Shell
- `/tutorial`, an opt-in Grok Build onboarding tour
- Soft and required CLI version checks in the Shell
- Remote flag to override the image-edit model
- Edit control on queued prompt rows
- Setting to disable the Ctrl+Space/F8 voice shortcut
- Privacy upsell banner in agent view until acted on
- Tools-server client callback surface
- Documentation for marketplaces, plugins, and organization controls
- Chat API fields for deploy archive, taken-down, limit, and in-progress reasons
- Chat-supplied per-session turn index in turn hooks
- Metrics for true-noop and stationarity stops
- Gateway bridge lifecycle telemetry

### Changed

- Default `/resume` to Grok sessions and show a hint for hidden external sessions
- Resume sessions by title with `--resume`
- Limit app-builder archive size
- Drive slash-command tag labels from data
- Surface Grok Computer media-generation results as file-path chunks
- Stamp session ID on image-generation direct-to-API requests
- Make auto mode consider recent user intent
- Show Bash mode chrome in minimal mode
- Include voice interim text on prompt submission
- Silently end a turn on true-noop thrash
- Quiet the copy toast when clipboard delivery is confirmed
- Make the idle “still running” watcher cue open the tasks pane
- Default the web-search model to Grok 4.5
- Let plugin subagents inherit parent MCP servers
- Gate the no-op end-turn reminder on system reminders
- Allow editing finalized text while voice is open
- Relocate the token carrier to turn-commit events and plumb per-turn origin context
- Raise workflow scratch quotas and make failed runs resumable
- Auto-progress workflow-overlay phases, show live agent status, and remove the budget meter

### Fixed

- Report workspace-server `/ready` failure with dwell when hub connection fails
- Refresh the Grok agent OIDC token in the Shell
- Fix tmux issues through Doctor remediation
- Preserve privacy-banner environment overrides across live settings updates
- Return auth-info profile fields even when the access token is expired
- Keep fail-closed behavior when clearing orphans with no team
- Pass `--raw` to `pw-record` for Linux dictation on older PipeWire
- Validate Git URLs when adding marketplace entries
- Stop shipping stale tool-doc parameter and tool names
- Re-point dashboard attach after `/fork` only when the parent was attached
- Clear the web background-task tray on kill while retaining the task description
- Protect persistent global hook sources
- Refresh tool search when the managed MCP catalog is re-fetched
- Prevent duplicate leader-process spawn and startup hangs from stale leaders
- Correct auto-mode blocked documentation
- Enforce a fail-closed auth-refresh contract for Shell clients
- Fix session forks truncating at the wrong prompt in rewound sessions

### Integration result

- Resolved 16 conflicts in an isolated worktree and preserved upstream ancestry in two-parent merge commit `963ccf5`.
- Preserved Pi-owned workflow spawning, external-agent routing, product-isolated paths, Pi session/tree/queue/settings, DirectPi effects and `pi_update`; adapted upstream mailbox, voice, tutorial, privacy, slash-tag and send-now contracts.
- Passed adapter tests (128), serial `grok-pi` binary tests (56), `cargo check`, and `./build.sh`.
- Remaining source/renderer/slash/mock verifier failures reproduce unchanged on pre-merge `main`; allowlists were not broadened. Workflow focused tests are 73/74 with the known macOS `/var` canonical-path assertion failure.

### Merge risk for grok-pi

- Upstream changes heavily overlap `xai-grok-pager/src/app/`, including session lifecycle, queue editing, settings, voice, workflow overlay, event loop, actions, effects, mouse handling, and task results.
- Preserve Pi-Grok seams: `external_agent` routing, Pi-owned queue/session/trust behavior, OpenPiConfig and product-isolated paths, DirectPi effects/results, model-picker guards, and native Pager-only rendering.
- Shell auth/workflow changes are upstream-owned and should normally take upstream behavior; do not let them pull Grok runtime ownership into `pi-grok-adapter`.
- Update `SOURCE_REV`, `AGENTS.md` base, and source-identity/renderer baselines only after the isolated merge is resolved and verified.

## [a5727c5] — 2026-07-23

> **Status:** Merged into grok-pi `main` via `sync/upstream-a5727c5` @ `4d19f00` (ff-only).

- **Sync range:** `3af4d5d..a5727c5` (`3af4d5d39897855bdcc74f23e690024a5dc05573` → `a5727c5960452e7527a154b25cb5bf00cda0545e`)
- **Upstream commits:** 1 (`Synced from monorepo`)
- **SOURCE_REV (monorepo SHA):** `30192d2eef5d91a8fff0e53957de5bd05b43398c` (was `0f4d7c91b8b2b408333f6de1e8a76cb8eaa71899`)
- **Diff size:** 482 files changed, +37627 / −13402

### Summary

Medium-large monorepo sync focused on **Doctor remediation consolidation**, **auto-mode classifier / permission gate behavior**, **marketplace reliability**, **working-directory relocation recovery**, and broad **Pager UX** (Esc cancel, queue edit newlines, permission auto-focus, dashboard hit targets). Shell/runtime and workspace permission crates dominate the +/−; Pager `app/` and `dispatch/` remain high-risk seam surfaces for grok-pi.

### Areas touched

| Area | Files | +/− | Added / Deleted | Notes |
|------|------:|----:|-----------------|-------|
| Shell (agent runtime) | 136 | +13903/−6593 | 3/0 | relocation recovery, doctor, toolOverrides, workflows default-on |
| Pager (TUI) | 234 | +11313/−3749 | 1/0 | Esc cancel, queue edit, dashboard, session-info, doctor UI |
| Workspace / Permission | 28 | +4083/−1405 | 1/0 | auto-mode classifier, Bash(git:*) chain match, folder trust |
| Test support | 9 | +3227/−341 | 1/0 | shared process lifecycle + sandbox |
| Tools | 25 | +2075/−362 | 2/0 | bang timeout, scheduler expiry, toolOverrides wire |
| Voice | 13 | +1022/−350 | 3/1 | out-of-process macOS mic capture |
| Common / models / agent | 16 | +1623/−376 | 1/0 | sampling types, agent lifecycle, file-utils |
| Config / hooks / chat / meta | 21 | +240/−133 | 0/0 | feedback.user docs, marketplace, SOURCE_REV |
| **Total** | **482** | **+37627/−13402** | **12/1** | |

### Added

- Non-blocking coding-data sharing upsell banner
- `toolOverrides` wire types and session/agent wiring
- Out-of-process macOS mic capture
- Shared test process lifecycle and shared test sandbox
- Relocation transaction state machine
- Privacy notice rollout flag
- One-shot occurrence journal persistence
- Durable scheduler expiry persistence
- Document `[feedback.user]` author identity config

### Changed

- Consolidate remediation in Doctor; apply doctor fixes in the TUI; route startup warnings to doctor
- Auto mode defers fail-closed gate asks to the classifier; classifier honors recorded approvals; timeouts prompt instead of silent deny
- Marketplace: coalesce list fetches; remove source by name; contain hung git sources (timeouts, non-blocking refresh, unbrick modal)
- Report real exit codes for completed background shells
- Narrow the date-rollover reminder to date-bearing templates
- Split prompt-trigger telemetry and record classifier provenance
- Raise connectors-manager timeout to 60s
- Scope subagent completion drains to the owning session
- Set `client_identifier=grok-agent-sdk`
- Accept both spellings of the workspace-teleport kill switch
- Stop turns that poll the exact same tool call 16× in a row
- Copy compaction checkpoint files when forking sessions
- Auto-focus permission prompt from scrollback
- Esc cancels the running turn in non-vim and minimal modes
- List Ctrl+Z undo and redo in keyboard shortcuts
- Show active auth mode on session-info
- Install the npm binary under `$GROK_HOME`
- Shift/Alt+Enter inserts newline when editing a queued prompt
- Gate project Claude permissions on folder trust
- Echo `response.create.event_id` on `response.created`
- Toast when session creation fails from disk full
- Enable dynamic workflows by default
- Integrate relocation recovery
- Confirm before removing extensions-modal items
- Re-run compact and prompt after login when compact hit expired auth
- Recap sends hosted tools under backend search
- Extend bang command timeout
- Label failed workspace RPCs with `error_kind`
- Drop redundant explicit tonic/prost deps from `xai-grok-shell`

### Fixed

- Security: Bash(`git:*`) allowlist matches whole command chain by prefix
- Close combine-queued edit-hold race
- Break harness discovery ref cycle so connections can idle-evict
- Remove hover/click dead zones between dashboard items
- Surface auth failures on model-switch compact

### Merge risk for grok-pi

- **Do not merge on `main`.** A trial `git merge upstream/main` produced **48 unmerged paths** and was aborted; use an isolated worktree/branch.
- High seam overlap: `xai-grok-pager/src/app/` (69 files, +5675/−1761), `dispatch/` (+2208/−481), `acp/tracker`, `event_loop`, `slash/`, `pager-bin/src/main.rs`.
- Permission/auto-mode and queue-edit changes may interact with Pi queue mirror + External profile intercepts — reapply narrow Pi-Grok seams after taking upstream core logic.
- `SOURCE_REV` / `AGENTS.md` base updated on merge-back (`30192d2e…` / `a5727c5`). Source-identity baselines may still need a deliberate regen if verifiers fail.
- Pi-Grok-only crates (`pi-grok-adapter`, `extensions/`) are not in this upstream range.


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
