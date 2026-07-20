# Changelog

All notable changes to **grok-pi** (Pi agent core in Grok Build's production TUI) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

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
