# Changelog

All notable changes to **grok-pi** (Pi agent core in Grok Build's production TUI) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.0.2] - 2026-07-17

### Added

- Native Welcome screen with Pi π block logo (default start; `-c/--continue` resumes)
- Agent Dashboard adaptation: `/dashboard` · Ctrl+\ · Pi session catalog → dormant roster
- Update check & install:
  - Sources: GitHub `Dwsy/grok-pi` releases JSON → npm mirrors (`npmmirror` / `registry.npmjs.org`)
  - CLI: `grok-pi update`, `grok-pi update --check`, `grok-pi update --to 0.0.2`
  - Welcome **Ctrl+U** runs the same installer after quit-for-update
- Welcome **Changelog** opens this file on GitHub
- Welcome **Resume session** via native SessionPicker (Pi catalog)

### Changed

- Welcome **New worktree** hidden for Pi (no Grok worktree product path)
- Release workflow stamps `GROK_VERSION` from the git tag

## [0.0.1] - 2026-07-17

### Added

- Initial `grok-pi` composition: Pi JSONL RPC ↔ ACP adapter + Grok Pager TUI
- Install scripts and multi-platform GitHub release packaging
