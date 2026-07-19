# Native Grok TUI Alignment

## Acceptance Conclusion

The current entry point is not a self-drawn Ratatui shell. `grok-pi` lives inside Grok Build's production binary package `xai-grok-pager-bin` and calls `xai_grok_pager::app::run_external`. The adapter only produces ACP requests/notifications; every terminal surface is created by the Grok Pager.

## Reused Grok Production Components

| Capability | Native implementation | Pi integration path |
|---|---|---|
| Terminal lifecycle | `xai-grok-pager/src/app/mod.rs` | `run_external` enters the same init/writer/event-loop/restore path |
| Fullscreen / minimal / inline | `xai-grok-pager` + `xai-grok-pager-minimal` | same screen-mode resolver and IoC hook |
| Input editor | `views/prompt_widget` | Pi prompt and `set_editor_text` enter the PromptWidget |
| Slash completion | `slash` + `views/completion_dropdown` | Pi `get_commands` converted to ACP `AvailableCommand` then merged |
| Markdown / code | `xai-grok-markdown` | `AgentMessageChunk`/`AgentThoughtChunk` enter the native scrollback pipeline |
| Tools and diffs | `acp/tracker`, native `RenderBlock` | Pi tool lifecycle converted to ACP `ToolCall`/`ToolCallUpdate` |
| Q&A overlay | `views/question_view` | `select`/`confirm`/`input`/`editor` converted to `x.ai/ask_user_question` |
| Status and notifications | native toast / sticky surface | `notify`/`setStatus`/`setWidget` converted to narrow ACP notifications |
| Scroll and transcript | native scrollback / transcript | both historical and live events are ACP `SessionUpdate` |
| Model selection | native model selector | Pi models / thinking levels converted to `SessionModelState` |

## Evidence of What Is Unchanged

The verification checklist establishes a SHA-256 baseline against the uploaded Grok source:

- 283 renderer/input/Markdown files remain byte-for-byte identical;
- 2698 non-seam Grok files remain byte-for-byte identical;
- the 17 allowed-to-change files live only in workspace manifest, ACP connection, App state/dispatch/effect, and slash profile seams;
- `pi-grok-adapter` contains no Ratatui/Crossterm, Terminal, Frame, Widget, draw, `event::read`, or raw-mode calls;
- `grok-pi.rs` contains no direct drawing or input loop.

## Why A Few Grok Pager Files Still Change

The ACP standard does not cover all of Pi's UI/command semantics, so narrow seams are needed:

1. `UiProfile::External`: disables Grok.com product capabilities without changing the renderer.
2. `AcpConnection::external`: lets the existing Pager accept an external ACP channel.
3. `run_external`: reuses the production terminal/event-loop, skipping Grok Agent startup and login.
4. Pi UI notification handlers: map fire-and-forget status to native toast/banner/title/editor.
5. QuestionView hints: reuse the native freeform editor and support Pi timeout revocation.
6. slash profile: only selects existing Grok commands that are meaningful for Pi and fully work under the external ACP composition; Pi dynamic commands remain managed by the native registry.
7. `/compact <instructions>`: passes the optional text from the native Grok command to Pi `customInstructions`.
8. screen-mode boundary: Grok's native minimal/fullscreen renderer is retained, but the original slash re-exec would rebuild Grok's own `--resume` argv and cannot carry `grok-pi`'s Pi startup arguments, so only the startup option is exposed, not the broken `/minimal`/`/fullscreen` re-exec.

These seams do not create a new renderer, nor copy the PromptWidget, QuestionView, Markdown, tool, or diff components.

## What Is Not Done

- Do not re-implement the Grok TUI;
- do not copy Pi-TUI;
- do not add an adapter-specific command palette;
- do not simulate toast, widget, or modal with character art;
- do not wrongly expose Grok login, cloud session, usage, plugin, voice, or other product features to Pi;
- do not forge an extension component factory that Pi RPC does not expose.
