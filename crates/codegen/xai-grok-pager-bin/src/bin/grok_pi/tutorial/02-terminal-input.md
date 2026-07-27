# Terminal, Prompt & Input

grok-pi enters Grok Pager's production terminal lifecycle instead of wrapping
Pi in a second shell.

- Choose fullscreen, inline or minimal mode at startup. The tutorial itself is
  fullscreen-only because minimal mode has no modal host.
- Fresh starts use the native Welcome screen and prewarm a Pi session behind it.
- The PromptWidget supports multiline editing, optional Vim mode and pasted
  text or images; responses use native Markdown, code blocks and scrollback.
- `/hotkeys` shows the active profile's keys. Appearance controls include
  `/theme`, `/timestamps`, `/timeline` and mouse reporting.
- `/voice` inserts Pager STT text into the prompt. Its xAI speech credential is
  separate from the provider Pi uses for the coding model.

Use `Tab` to move between prompt and scrollback, then search, select, copy or
export without leaving the native Pager.
