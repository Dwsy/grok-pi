# Extension UI & Remote TUI

grok-pi maps Pi extension UI calls onto existing Pager components:

- notifications, status, widgets, title and editor text use native surfaces;
- select, confirm, input and multiline editor requests use QuestionView;
- timeout or cancellation revokes the matching question instead of leaving a
  dead overlay.

Pi's `ctx.ui.custom` components require the experimental Remote TUI compatibility
host. It is default on but can be disabled with `PI_GROK_REMOTE_TUI=0`; the Pi
child still runs real JSONL RPC either way. The bridge projects component frames
and keys into Pager—it does not give the child a real TTY.

`/pi-shortcut-manager` manages shortcuts registered by Pi extensions without
claiming Pager's built-in keys. Raw terminal hooks and arbitrary custom
header/footer factories remain unsupported boundaries.
