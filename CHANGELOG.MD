# Changelog

All notable changes to **grok-pi** (Pi agent core in Grok Build's production TUI) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.0.8] - 2026-07-22

Range: `v0.0.7` → `v0.0.8` (2026-07-21 → 2026-07-22).

### Added

#### Product state isolation

- Default user home is now **`~/.grok-pi`** (`$GROK_HOME`) instead of sharing stock Grok’s `~/.grok`
- Default project config tree is **`<repo>/.grok-pi`** (`$GROK_PROJECT_DIR`); paths resolve via `xai_grok_config::project_config_dirname()` / `project_config_dir`
- Startup injects home + project dirname before any library pins `grok_home()` via `OnceLock`
- **`grok-pi migrate-home`**: copy allowlisted files from legacy `~/.grok` (or `$GROK_LEGACY_HOME`) with `--status` / `--dry-run` / `--force` / `--from` / `--into` / optional `--include-auth`
- Safe **auto-migrate once** when target home is empty and legacy has data; writes `.migrated-from-legacy` marker
- **Not** dual-scanned: stock `~/.grok` / `<repo>/.grok` are not auto-read for discovery (true isolation)
- Unit tests without env keep stock `.grok` default so upstream-style tests stay green

#### Pi workflows (Rhai, F2 off by default)

- Reuse upstream **`xai-workflow`** engine + shell orchestration; **SpawnBackend** pluggable (`Grok` default / `Pi` bridge for grok-pi)
- Adapter: `WorkflowHost`, `pi_workflow_backend`, ACP `x.ai/workflow/{launch,pause,stop}`, `x.ai/workflows/list`, `workflow_updated` notifications
- Injected extension + slash: `/workflow`, `/workflows`, `/create-workflow` (+ named scripts); bridge cmds `__pi_workflow_*` filtered from catalog
- F2 **`[ui].pi_workflows`** default **off**, restart required to inject extension
- Project scripts live under `<repo>/.grok-pi/workflows` (and user `~/.grok-pi/workflows`)
- `/create-workflow` is Pager PassThrough user prompt (not a Pi skill)

#### Goal mode (F2 off by default)

- F2 **`[ui].pi_goal`** default **off**, restart required
- Injected extension: `/goal` + `update_goal` tool + control file
- Adapter **`GoalHost`**: state machine → native `GoalUpdated` (`goal_detail` / status bar)
- Active goal continues via `agent_settled` follow-up (not full shell multi-agent classifier/planner/strategist stack)

#### Export / share

- Default-on **`pi-grok-export`**: `/export-html` (Pi HTML or `.jsonl` path) and `/pi-share` (private gh gist + pi.dev viewer)
- Grok `/export` remains Markdown transcript; no second TUI

#### Website & CI

- Marketing + docs site under **`website/`** (Next.js 15, EN/ZH, landing + docs)
- GitHub Actions **`Website`** workflow: `workflow_dispatch` only; `npm ci` + `npm run build`; uploads standalone / static / public artifacts
- Release workflow: **Swatinem/rust-cache** for faster multi-platform builds

#### Session picker & theming

- Session picker **full-text search** (`Ctrl+F`) for external Pi sessions; dedicated search page on the external catalog, state reset on each toggle
- Transparent themes (`pi:transparent` / `pi:transparent-light`): code blocks use terminal default background (`Color::Reset`); user messages get a **left accent bar** when canvas bg is Reset
- Transparent theme JSON assets + Pi theme map (`theme/pi/map.rs`) updates for code/user surfaces
- Pager app/view/dispatch wiring for session load, jump, modals, welcome, and overlay list polish that landed with the picker/search work

#### Adapter / queue (pre-isolation wave)

- `pi-grok-adapter` model / RPC / session / queue bridge updates: richer session catalog fields, queue mirror alignment with Pager optimistic dequeue, RPC client/session path hardening for workflow/goal hosts that follow

#### Upstream & tooling

- Sync Grok Build **`3af4d5d`** (`SOURCE_REV` `0f4d7c91`): monorepo pull including new **`xai-workflow`** crate, workspace permission/security overhaul (exec-risk, auto-mode, hardened shell), shell working-directory relocation, doctor/terminal-setup surfaces, prompt-queue batching knobs — full list in `docs/upstream/UPSTREAM_CHANGELOG.md`
- **`upstream-changelog`** skill (`.pi/skills/upstream-changelog/`) + first structured entry for 3af4d5d; AGENTS.md documents two-phase sync (changelog first, merge second)
- `pi-main` submodule advanced to latest **earendil-works/pi** `main` (`a5afc3f1`); **local dirty RPC edits in the submodule are rejected** — pin only
- Release CI: **Swatinem/rust-cache**; release notes body built from `CHANGELOG.MD` via `scripts/extract-changelog-section.py` (strict, optional cumulative `--since 0.0.6`, install footer) then GitHub auto commit list

#### Docs / legal

- Queue architecture redesign research: `docs/issues/queue-architecture-redesign.md`, `queue-redesign-feasibility.md` (Pager-owned queue / eliminate multi-layer reconcile — research only, not shipped)
- Isolation / workflow / goal Issues under `docs/issues/架构/` and `docs/issues/adapter/`
- LICENSE: retain SpaceXAI upstream Apache-2.0; note Dwsy fork copyright for modifications
- README / FEATURE_MATRIX / AGENTS bilingual updates for product isolation, workflows, goal, export

### Changed

- Adapter model / RPC / session / queue bridge updates for upstream + workflow/goal hosts
- Pager app logic, jump/session picker views, dispatch, effects, and `grok-pi` bin wiring for new F2 gates (`pi_workflows`, `pi_goal`) and extension injection order
- `install.sh` / path helpers respect product-isolated homes where applicable
- Slash command registration: external-profile gating consistency across Pager-local commands; `/doctor` treated as pager-native terminal diagnostics

### Fixed

- `pi-grok-bash` extension updates aligned with background-task kill control path (`x.ai/task/kill`)

### Notes

- Local dirty edits to **`pi-main`** source are **not** allowed; submodule is a clean pin only
- F2 **Pi workflows** / **Pi goal** require full quit + restart (extension inject at process start)
- Uncommitted WIP (e.g. `/review-session` polish) is **not** part of this tag unless landed before cut
- Upgrading from **0.0.6**: read **[0.0.7]** as well (plan mode, `/jump`, `/fork`/`/clone`/`/reload`, resource policy, Remote TUI, tree rollback, update proxy) — GitHub Release notes for 0.0.8 embed both sections by default

---

## [0.0.7] - 2026-07-21

Range: `v0.0.6` → `v0.0.7`. (Previously omitted from this file; reconstructed from git history.)

### Added

#### Plan mode (native Pager ↔ Pi)

- Pager-native Plan toggle bridged to adapter-owned state machine: `Inactive` / `Pending` / `Active` / `ExitPending`
- Session-private `.plan.md` sidecar + `.plan-mode.json` persistence
- Injected tool gate blocks `edit` / `write` / `bash` except the plan file while active
- Pi `exit_plan_mode` → native `x.ai/exit_plan_mode` approval surface
- Full/sparse system-reminder prefixes depending on mode phase
- Keyboard chord **Ctrl+Shift+T** toggles plan mode; plan file path published for `/view-plan`

#### Session navigation & tree

- **`/jump`**: native turn picker with timeline previews, compact wall-clock time (`HH:MM`), viewport restore
- External profile: empty **double-Esc** and **`/rewind`** open **SessionTree** (Pi tree navigation, not Grok destructive rewind)
- SessionTree selected-row highlight aligned with Pi selection semantics
- **Tree file rollback** (F2 `pi_tree_file_rollback`, external-only, restart required): SessionTree `r` = preview, `R` = execute; checkpoint preimages for write/edit via `pi-grok-rollback` + adapter `pi/session/rollback_preview|execute`

#### Pi session branch ops

- **`/fork`** (external): jump-style `ListOverlay` from RPC `get_fork_messages`; `fork` creates branched session file; same agent rebinds `sessionId` + `session/load` replay; selected text prefills prompt
- **`/clone`** (external): RPC `clone` duplicates current leaf; rebind + replay; prompt cleared (Pi parity)
- Grok peer-agent `/fork` unchanged for non-external profiles

#### Reload, hotkeys, session alias

- **`/reload`**: `__pi_reload` → `ctx.reload()`; blocks while **streaming or compacting**; adapter refreshes command/model catalogs; Pager rediscovers Pi themes and re-applies active `pi:*` theme; toast copy matches Pi interactive
- **`/hotkeys`** (aliases `shortcuts` / `keys`): opens native ShortcutsHelp modal (same as Ctrl+.)
- **`/session`** alias for `/session-info`; payload can include `session_file` + message/turn/tool-call counts

#### Resource policy & CLI

- **Pi resource admission policy** (`ResourcePolicy`): allow/block lists + heuristics (e.g. block `pi-tool-display`, custom header/footer); applied at `grok-pi` startup
- Startup disables Pi auto-discovery (`--no-extensions/skills/prompt-templates/themes` as needed) and injects only policy-approved resources via explicit flags
- Catalog discovery aligned with Pi package auto-entry rules; package dirs expand to declared entry only (not nested private modules)
- `/pi-config` modal: All/Enabled/Disabled filter, policy view (`a`), refresh (`r`), Tab scope, expanded shortcuts; F2 Enter on `pi_config` opens modal
- CLI tool flags: `--no-tools` / `-nt` / `--no-builtin-tools` / `-nbt`, `--exclude-tools` / `-xt`, env `PI_GROK_EXCLUDE_TOOLS`
- First-class Pi flags forwarded: model, session, prompts, resources, tools, trust/network; args after `--` still passthrough

#### Remote TUI, auth, recap

- Remote TUI host uses official Pi **`rpc-entry.js`** + extension mode facade (`ctx.mode` appears as `tui` to extensions while transport stays RPC) — **no Pi source fork**
- `pi-grok-remote-tui`: multi-select capability lab (header/footer widgets, status, title, editor text); overlay stacking with restore
- `pi-grok-auth`: fix nested `openCustom` tearing down LoginDialog; `showOverlay` / `showAuthPrompt` / `prompt.signal`
- `pi-grok-recap`: optional **Mermaid** via F2 `recap_mermaid`; markdown cleaner preserves inner mermaid fences; recap body rendered as Markdown

#### Settings (external-only gates)

- `SettingMeta.external_only`: rows hidden unless grok-pi / external profile active
- F2: `pi_tree_file_rollback` (restart-required), `recap_mermaid`, `remote_tui_footer`
- `EXTERNAL_AGENT_ACTIVE` atomic set once at startup from UI profile

#### Timeline / theme render

- Timeline rail glyphs (chevron, heavy/light horizontal, active/hover ticks) with ConHost fallbacks; horizontal strokes for Pi visual parity
- Pi theme registry **`rediscover()`** after `/reload`

#### Update channel

- GitHub release discovery via **JSP proxy** fallback (`jsp.dwsy.link`) to avoid unauthenticated API rate limits; fall back to `api.github.com`

### Fixed

- Expanded edit card: throttle redraw when in viewport; sticky-header cache + edit gutter height estimate
- Live-turn sticky suppress + Pi-aligned spinner
- Version compare: ignore dirty prerelease; use `+dirty` build metadata so local builds do not falsely report “update available”
- Package extension discovery: expand package dirs as Pi auto-entries (manifest entry only)
- Bash task kill: `x.ai/task/kill` reaches Pi Bash extension with `runningTaskIds` validation (`KillOutcome`)
- Widget rendering alignment + picker `label_color` polish

### Changed

- Adapter: Pi RPC stderr ring buffer diagnostics; rustfmt cleanup
- Bilingual README streamlining; Remote TUI docs / flow demo
- Upstream monorepo syncs absorbed during the window (see git `Synced from monorepo` commits)

### Docs

- FEATURE_MATRIX rows for `/fork` `/clone` `/reload` `/hotkeys` `/session`
- Resume preview & search alignment issue specs

---

## [0.0.6] - 2026-07-20

### Added

- PSM session catalog in the adapter with enriched metadata and tree editor text support
- Session picker sorting, expand-all, rich metadata display, and tree navigation

### Fixed

- Model picker height is now constrained to the available viewport
- Remote TUI paste events are forwarded correctly
- Subagents live traffic uses `appendEntry` with simplified recap emit

## [0.0.5] - 2026-07-18

### Added

- Native Pi resource manager for extensions, skills, prompts, and themes
- Context visualisation bridge with native Pager rendering
- Session recap, timeline, and Pi model-selection integration improvements

### Changed

- Welcome hero card now uses the `grok-pi` name, product version, and Pi description
- Release builds, update checks, and update installs use only `GROK_PI_VERSION`

### Fixed

- Upstream Grok `GROK_VERSION` and workspace version no longer affect `grok-pi` version display or update comparison

## [0.0.4] - 2026-07-17

### Added

- Context click / `/context`: adapter implements `x.ai/session/info` from Pi `get_session_stats` + message estimate → native `ContextInfoBlock`
- Queue pane bridge: Pi `queue_update` full arrays → `x.ai/queue/changed` so optimistic dequeue works
- Product version stamp via `GROK_PI_VERSION` / git describe (no more upstream `0.1.220-alpha.*` on `--version`)

### Fixed

- Context breakdown no longer dumps the whole window into Reasoning/overhead when message estimate is empty (fallback → Messages)
- Session-info payload omits null Option fields for safer pager deserial
- Welcome / dashboard / session load paths for Pi catalog continuity

## [0.0.3] - 2026-07-17

### Changed

- Update check/install is **GitHub-only** (`Dwsy/grok-pi` releases JSON + install.sh/ps1)
- Removed npm registry fallback (unscoped `grok-pi` is a foreign package; scoped package not published yet)

## [0.0.2] - 2026-07-17

### Added

- Native Welcome screen with Pi π block logo (default start; `-c/--continue` resumes)
- Agent Dashboard adaptation: `/dashboard` · Ctrl+\ · Pi session catalog → dormant roster
- Update check & install:
  - Sources: GitHub `Dwsy/grok-pi` releases JSON (npm fallback removed in 0.0.3)
  - CLI: `grok-pi update`, `grok-pi update --check`, `grok-pi update --to 0.0.2`
  - Welcome **Ctrl+U** runs the same installer after quit-for-update
- Welcome **Changelog** opens this file on GitHub
- Welcome **Resume session** via native SessionPicker (Pi catalog)

### Changed

- Welcome **New worktree** hidden for Pi (no Grok worktree product path)
- Canonical GitHub repo: `Dwsy/grok-pi`
- Release workflow stamps `GROK_VERSION` from the git tag

## [0.0.1] - 2026-07-17

### Added

- Initial `grok-pi` composition: Pi JSONL RPC ↔ ACP adapter + Grok Pager TUI
- Install scripts and multi-platform GitHub release packaging
