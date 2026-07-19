# pi-grok-adapter

`pi-grok-adapter` is a **headless, library-only** Pi JSONL RPC ↔ ACP adapter. It does not create a terminal, read the keyboard, call Ratatui/Crossterm, or render any widget.

The actual executable lives at:

```text
crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs
```

This binary starts the Pi RPC inside Grok Build's production composition package, then hands the ACP channel to:

```text
xai-grok-pager  (production Pager)
```

So the UI is natively implemented by `xai-grok-pager`, `xai-grok-pager-minimal`, and `xai-grok-markdown`.

## Adaptation Responsibilities

- Start and supervise `pi --mode rpc`;
- Convert Pi JSONL RPC to ACP, and ACP back to Pi JSONL;
- Project Pi session/agent lifecycle events to ACP `SessionUpdate`;
- Pi `queue_update` full-array → `x.ai/queue/changed` (native QueuePane mirror + dequeue);
- Pi queue/compaction/retry/session-name state ↔ Grok native state/title;
- Pi tool lifecycle → ACP `ToolCall`/`ToolCallUpdate` rendered by the native tool card;
- Extension UI (`notify`/`setStatus`/`setWidget`/`setTitle`/`select`/…) → narrow ACP notifications / `x.ai/ask_user_question`.

## Explicitly Not Responsible For

- terminal initialization and restore;
- keyboard and mouse input handling;
- the agent loop, model selection, or tool execution (owned by Pi);
- Markdown, code, diff, tool card, image, and scrollback renderers;
- theme, mouse, Vim/multiline modes;
- any adapter-specific slash UI.

## Verification

```bash
cargo test -p pi-grok-adapter
```

For the full delivery, run `./verify.sh` from the repository root.
