id: "2026-07-21-hunk-review-external-tui-handoff"
title: "[Adapter] hunk-review external TUI handoff"
status: "implementation"
created: "2026-07-21"
updated: "2026-07-21"
category: "adapter"
tags: ["grok-pi", "hunk", "external-tui", "nvim", "pty", "rpc", "pager"]
---

# Issue: hunk-review external TUI handoff

## Goal

将 `/hunk-review` 从“在 Pi `pi-tui` overlay 中解析 hunk ANSI”改为
“暂停当前宿主 TUI，让 hunk 直接继承真实终端”，保证 hunk 的全屏 TUI、
alternate screen、光标、鼠标、resize 和键盘语义不被二次渲染破坏。

## Background / Decision

当前实验扩展链路为：

```text
hunk → zigpty → xterm-headless → SerializeAddon → Pi Component overlay
```

PTY 本身可以运行全屏 TUI；异常来自将 hunk 的 terminal screen buffer 再投影为
Pi `Component.render()` 文本行。该路径不透明地重建 alternate-screen、光标、清屏、
鼠标和布局控制序列，不满足 hunk 作为原生全屏 TUI 的要求。

Pi 已有可复用的 external-editor 生命周期：

```text
Pi TUI stop → spawn(command, { stdio: "inherit" }) → wait exit
→ Pi TUI start → requestRender(true)
```

证据：系统 Pi `interactive-mode.js:3093-3138`，扩展编辑器
`components/extension-editor.js:87-124`。

## Architecture boundary

### Ordinary Pi

`/hunk-review` extension may use the official interactive TUI lifecycle and spawn:

```text
hunk diff --watch
```

with `stdio: "inherit"`. The extension owns only command registration and process
lifecycle; hunk owns the visible full-screen TUI.

### Grok-Pi

Grok-Pi runs Pi as an RPC child with stdin/stdout/stderr pipes
(`pi-grok-adapter/src/pi_rpc.rs:44-73`). Pi RPC has no real terminal and explicitly
leaves `custom()` and raw terminal input unsupported
(`pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:162-165,194-230`).
Therefore a global Pi extension cannot make `spawn(..., { stdio: "inherit" })` inherit
the user's Pager terminal.

Grok-Pi requires one narrow host-owned external-terminal seam:

```text
Pi extension command
  → semantic external_tui request
  → adapter / Pager host
  → stop or suspend Pager event/render loop
  → host spawn hunk with stdio inherited from the real terminal
  → wait for hunk exit
  → restore Pager terminal and force redraw
```

This is distinct from Remote TUI frame projection. Remote TUI remains suitable for
Pi components that can be represented as native Pager frames, but must not be used to
re-render a full-screen hunk terminal.

## Scope

- [x] Confirm PTY can launch TUI but current ANSI-to-Component projection is wrong for this use.
- [x] Confirm Pi's external-editor lifecycle and `stdio: "inherit"` implementation.
- [x] Confirm Grok-Pi Pi child has RPC pipes rather than a real TTY.
- [ ] Replace `~/.pi/agent/extensions/hunk-review` implementation with external process handoff.
- [ ] Add a narrow Grok-Pi external-TUI request/response seam without modifying Pi source.
- [ ] Forward `/hunk-review` command from the native Grok slash registry to the Pi extension.
- [ ] Restore Pager terminal and redraw after hunk exits, including failure and cancellation.
- [ ] Add ordinary Pi and Grok-Pi manual acceptance checks.

## Acceptance Criteria

1. Ordinary Pi `/hunk-review` launches `hunk diff --watch` in the real terminal, not inside
   a Pi `Component` and not through `zigpty`/xterm serialization.
2. Hunk's file tree, split diff, alternate screen, cursor, keyboard navigation, mouse
   reporting and resize behavior remain owned by hunk.
3. Grok-Pi `/hunk-review` does not write hunk ANSI frames through `setWidget` or
   `remote_tui_frame`.
4. Grok-Pi returns to the native Pager after hunk exits and forces a full redraw.
5. The adapter remains headless and library-only; no terminal or renderer code is added
   to `pi-grok-adapter`.
6. Pi source (`pi-main`) is not modified; the seam uses the official extension API and
   existing Grok-Pi host/ACP composition.
7. Launch failures produce a native Pager/Pi notification and do not leave the terminal
   in raw or alternate-screen mode.

## Risks / Open Questions

- A host-owned handoff must not run while an ACP/Pager action is in a critical streaming
  state without a defined pause/restore policy.
- The current ACP extension notification path is fire-and-forget; a handoff that needs
  completion must define a correlation id and a response path, or use a host command
  invocation with an explicit completion notification.
- The Pager event loop and terminal restoration APIs must be used rather than duplicated.
- Hunk's `--watch` process should be foreground for the handoff; it must not be kept as
  a detached background process after the user returns to Pager.

## Errors Encountered

| Date | Error | Resolution |
|---|---|---|
| 2026-07-21 | `hunk-review` overlay rendered TUI incorrectly after `allowProposedApi` fix | Rejected ANSI-to-Component embedding; switch to external terminal handoff |
| 2026-07-21 | `zigpty` not resolvable from flat global extension | Replaced package-resolution hack with a dedicated pnpm package; dependency remains obsolete after handoff rewrite |

## Evidence Pointers

- Pi external editor: installed `@earendil-works/pi-coding-agent/dist/modes/interactive/interactive-mode.js:3093-3138`
- Pi extension editor: installed `.../components/extension-editor.js:87-124`
- Pi RPC no raw TTY/custom component: `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts:162-230`
- Grok-Pi RPC child pipes: `crates/codegen/pi-grok-adapter/src/pi_rpc.rs:44-73`
- Existing Remote TUI frame bridge: `extensions/pi-grok-remote-tui/index.ts`,
  `crates/codegen/pi-grok-adapter/src/pi_adapter.rs:1806-1833,2537-2554`
- Architecture invariant: `NATIVE_GROK_TUI_ALIGNMENT.md`, `FEATURE_MATRIX.md`
