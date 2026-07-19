id: "2026-07-19-pi-extension-widget-keyboard-gap"
title: "[TUI] Pi Extension Widget Rendering vs Keyboard Interaction Gap in grok-pi"
status: "research"
created: "2026-07-19"
updated: "2026-07-19"
category: "TUI"
tags: ["grok-pi", "pager", "pi-tui", "rpc", "extension-ui", "setWidget", "onTerminalInput"]
---

# Issue: Pi extension widget rendering vs keyboard interaction gap in grok-pi

## Goal

Document the exact coverage gap between Pi interactive TUI extension UI and what
`grok-pi` (which runs `pi --mode rpc` + Grok Pager) can actually render and make
interactive. Focus: `@tintinweb/pi-subagents` FleetView list + keyboard navigation
(arrow keys / Enter to open a subagent conversation).

## Background / Problem

`grok-pi` does not draw its own widgets; it forwards Pi RPC `extension_ui_request`
events to Grok Pager, which renders them with native surfaces. The previous audit
established that `setWidget` (string array) is covered. This research clarifies two
things the previous audit left open:

1. Whether `setWidget` in original Pi TUI is keyboard-interactive at all.
2. How `@tintinweb/pi-subagents` actually implements "arrow keys + Enter opens a
   subagent", and whether that path is reachable under RPC mode.

### Finding 1 — `setWidget` is NOT keyboard-interactive in Pi interactive TUI

`interactive-mode.ts:1894` `setExtensionWidget` has two branches:

- **String array branch** (`:1916`): each line wrapped in `Text` (`components/text.ts:7`).
  `Text` implements only `Component`, NOT `Focusable` (`tui.ts:104`,
  `isFocusable` returns false at `:110`). It never receives focus; the prompt editor
  keeps focus (`interactive-mode.ts:737`). So string-array widgets are read-only by
  design.
- **Factory branch** (`:1926`): `component = content(this.ui, theme)`. The factory may
  return a `Focusable` component, but `setExtensionWidget` does NOT call
  `this.ui.setFocus(component)`. So even a focusable factory component is not
  auto-focused; the extension must self-manage focus (call `setFocus`, register input
  listeners). The `setWidget` API itself provides no keyboard interaction.

**Conclusion:** keyboard-interactive widgets in Pi interactive TUI are possible ONLY
via a factory component that the extension itself wires up (focus + listeners). That
path is dropped by the RPC layer (see below).

### Finding 2 — `@tintinweb/pi-subagents` FleetView uses two layers

Source: `~/.pi/agent/npm/node_modules/@tintinweb/pi-subagents/src/ui/fleet-list.ts`.

- **Display layer (read-only widget):** `FleetList.update()` calls
  `ui.setWidget("fleet", factory, { placement: "belowEditor" })` (`:165`). The factory
  returns a component with only `render()` / `invalidate()` — no focus, no `onKey`.
  It draws `main` + per-agent rows + hint text. This is the same read-only surface as
  the string-array widget.
- **Interaction layer (global terminal input listener):** `FleetList.setUICtx()`
  registers `ui.onTerminalInput(data => this.handleKey(data))` (`:114`). `handleKey`
  (`:212`) consumes keys BEFORE the focused editor and navigates the list:
  - empty prompt + `↓`/`←` → activate list (`active = true`, `:231`);
  - `↓`/`↑` move `selectedIndex`; `Enter` → `openSelected()`; `Esc` → deactivate
    (`:242-255`);
  - gated on `getEditorText() === ""` and `editorHasFocus()` (`:224`, `:270`) so normal
    typing is untouched.
- **Open subagent:** `openSelected()` (`:281`) calls `ui.custom(factory, { overlay: true })`
  (`:298`) to open a `ConversationViewer` overlay. `custom()` is the component-factory
  UI path.

### Finding 3 — RPC mode does NOT provide the interaction primitives

`pi --mode rpc` (what `grok-pi` uses) implements the extension UI context at
`rpc-mode.ts`:

- `onTerminalInput()` (`:168`) is a **no-op**: `return () => {};` with comment
  "Raw terminal input not supported in RPC mode". The FleetView listener never fires.
- `custom()` (`:233`) returns `undefined` unless `PI_GROK_REMOTE_TUI=1` (experimental
  Remote TUI frame projection). Otherwise factory components are not supported.
- `setWidget` (`:200`) forwards only `string[]` (or `undefined`); factory functions are
  explicitly ignored (`:212`).

This matches `AGENTS.md` architecture invariants: `raw terminal hook` is a deliberate
**boundary** ("Pi RPC 明确不支持"), and `custom header/footer/component` is a boundary
("Pi RPC 明确不支持 component factory").

## Coverage matrix (Pi interactive vs grok-pi / RPC)

| Pi extension UI | Interactive TUI | grok-pi (RPC) | Surface in Pager |
|---|---|---|---|
| `setWidget` string[] | read-only rows | rendered | `external_widgets_above/below_editor` (`render.rs:1924` / `:2026`) |
| `setWidget` factory | read-only rows (no auto-focus) | dropped by RPC | — |
| `setStatus` | status bar segment | rendered | `status_bar` `external_status` (`render.rs:1147`) |
| `setTitle` | terminal title | rendered | OS title bar |
| `set_editor_text` | editor content | rendered | PromptWidget |
| `notify` | toast / scrollback | rendered | toast + scrollback system block |
| `select`/`confirm`/`input`/`editor` | QuestionView | rendered | QuestionView |
| `onTerminalInput` | global key interception | **no-op** | **not reachable** |
| `custom(factory)` | overlay component | **no-op** (unless `PI_GROK_REMOTE_TUI=1`) | **not reachable** |
| `setFooter`/`setHeader` factory | footer/header component | not exported by RPC | **not reachable** |

## Impact on `@tintinweb/pi-subagents` under grok-pi

- The FleetView **list renders** (belowEditor widget, covered).
- The agent **status spinner / token stats** render (driven by `setStatus` + widget
  re-render via `tui.requestRender()` inside the factory — note: `requestRender` is a
  Pi-tui stub method; under RPC the factory is dropped, so only the string-array form
  reaches Pager, and animation cadence is driven by Pager-side refresh instead).
- **Keyboard navigation is dead**: `onTerminalInput` is a no-op in RPC mode, so arrow
  keys / Enter / Esc never reach `handleKey`. The prompt also keeps normal key flow.
- **Opening a subagent conversation is dead**: `openSelected()` needs `ctx.ui.custom`,
  which returns `undefined` under RPC (unless Remote TUI experimental flag is set).

So in `grok-pi`, this extension degrades to a **static read-only agent list** with no
way to browse or open subagent conversations from the keyboard.

## Options to close the gap

### Option A — Native Pager Fleet surface (recommended direction)
Map Pi `queue_update` / agent lifecycle already projected to Pager native
`SubagentBlock` / Tasks Pane / child `AgentView` (per FEATURE_MATRIX.md). The
`pi-grok-subagents` built-in extension already bridges child sessions to these native
surfaces. Keyboard navigation of running agents should go through the existing native
Tasks Pane / child AgentView, not a custom widget + `onTerminalInput`. This keeps the
"no second TUI, reuse native Grok surfaces" invariant.

### Option B — Bridge `onTerminalInput` over RPC (large, out of contract)
Add a reverse ACP channel Pager → Pi carrying raw key events while the prompt is empty,
so extensions can `consume` them. Requires:
- Pi RPC to actually emit/handle a terminal-input subscription (currently `return () => {}`);
- A new ACP method + Pager-side forwarding from the PromptWidget input loop.
Violates the "raw terminal hook is a boundary" decision and the "do not modify Pi
source to extend RPC" rule. Not recommended without an upstream RPC feature.

### Option C — Remote TUI experimental rail
`PI_GROK_REMOTE_TUI=1` already projects `custom()` factory frames to Pager as ANSI
lines. Extending it to also proxy `onTerminalInput` keystrokes back to the Pi component
would make FleetView + ConversationViewer partially work. Still experimental, read-only
rendering with limited interaction fidelity.

## Acceptance criteria (for the gap, not yet implemented)

- [x] Pi interactive `setWidget` focus behavior documented (read-only by design).
- [x] `@tintinweb/pi-subagents` FleetView mechanism traced: widget (display) +
  `onTerminalInput` (interaction) + `custom` (overlay).
- [x] RPC-mode no-op for `onTerminalInput` and `custom` confirmed in `rpc-mode.ts`.
- [x] Coverage matrix (interactive vs grok-pi) captured.
- [ ] Decide whether to pursue Option A (native surface) or document FleetView as a
  known degraded experience under grok-pi.

## References

- `pi-main/packages/coding-agent/src/modes/interactive/interactive-mode.ts:1894` (`setExtensionWidget`)
- `pi-main/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2136` (`onTerminalInput` impl)
- `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:168` (`onTerminalInput` no-op)
- `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:200` (`setWidget` string-only)
- `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:233` (`custom` no-op)
- `pi-main/packages/tui/src/components/text.ts:7` (`Text` is `Component`, not `Focusable`)
- `pi-main/packages/tui/src/tui.ts:104` (`Focusable` interface / `isFocusable`)
- `~/.pi/agent/npm/node_modules/@tintinweb/pi-subagents/src/ui/fleet-list.ts:114` (`onTerminalInput` registration)
- `~/.pi/agent/npm/node_modules/@tintinweb/pi-subagents/src/ui/fleet-list.ts:165` (belowEditor widget)
- `~/.pi/agent/npm/node_modules/@tintinweb/pi-subagents/src/ui/fleet-list.ts:298` (`custom` overlay)
- `crates/codegen/pi-grok-adapter/src/pi_adapter.rs:1502` (`setwidget` → `pi/ui/widget`)
- `crates/codegen/xai-grok-pager/src/app/agent_view/render.rs:2026` (below-editor widget render)
- `FEATURE_MATRIX.md` (Extension UI / subagent mapping)
- `AGENTS.md` (boundary: raw terminal hook, custom component factory)
