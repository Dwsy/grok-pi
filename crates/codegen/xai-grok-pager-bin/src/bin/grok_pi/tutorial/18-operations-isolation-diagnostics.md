# Export, Updates & Product State

grok-pi separates product operations from stock Grok while retaining native
Pager surfaces.

- `/export` writes the Pager Markdown transcript.
- Default-on `/export-html` uses Pi's HTML exporter; `/pi-share` can create a
  private GitHub gist and Pi viewer URL when `gh` is available.
- `grok-pi update`, `--check` and Welcome's update action use GitHub Releases for
  `Dwsy/grok-pi`, not Grok's CDN.
- Grok-facing configuration defaults to `~/.grok-pi` and `<repo>/.grok-pi`.
  `grok-pi migrate-home` copies only an allowlisted subset from legacy stock
  state; it does not merge workflow directories.
- Pi sessions, providers and Pi ecosystem resources remain Pi-owned.
- `/notify`, `/doctor`, `/debug`, `/help` and `/hotkeys` expose notifications,
  terminal checks, overlays and command discovery.

This product does not expose stock Grok cloud history, worktree creation or
chat rewind as Pi capabilities.
