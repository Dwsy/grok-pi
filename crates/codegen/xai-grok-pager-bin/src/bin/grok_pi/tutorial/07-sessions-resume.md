# Sessions & Resume

Pi owns local append-only JSONL sessions. grok-pi discovers and presents them
through native Pager surfaces.

- `/new` starts a fresh Pi session and `/rename` assigns a useful title.
- `/resume` opens the rich session picker with names, paths, times, model,
  messages and persisted usage when available.
- `grok-pi -c` continues the latest session.
- Startup options include `--session`, `--session-id`, `--session-dir`,
  `--fork`, `--no-session` and `--name`.
- Session scanning is on demand rather than startup work, and custom Pi session
  directories remain discoverable.

These are Pi-local sessions, not Grok cloud history. On exit, grok-pi prints an
accurate resume command, including a custom session directory when needed.
