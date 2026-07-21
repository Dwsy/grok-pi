---
id: "2026-07-21-Pi-消息级-session-fork"
title: "Pi 消息级 session fork"
status: "completed"
created: "2026-07-21"
updated: "2026-07-21"
category: "adapter"
tags: ["workhub", "pi", "fork", "session"]
---

# Issue: Pi 消息级 session fork

## Goal

在 grok-pi 中补齐 Pi 原生 `/fork` 语义：从历史 user message 分叉出**新的 session 文件**，同 agent 切换到新 leaf，并把选中文案回填到 prompt。不实现 Grok peer-agent `/fork`（worktree/peer session）。

## 背景/问题

- Pi interactive `/fork` = `getUserMessagesForForking` 选择器 + `runtimeHost.fork(entryId)`（新 JSONL + 预填 editor）。
- Pi RPC 已暴露 `get_fork_messages` / `fork`；RPC 模式没有 TUI 选择器。
- grok-pi 当前 `PI_GROK_NATIVE_COMMANDS` 未包含 `fork`；ACP 透传 `/fork` 无法弹出选择器。
- Grok Pager 的 `/fork` 是 peer agent + `x.ai/session/fork`，与 Pi 消息级 fork 模型不同。

## 验收标准 (Acceptance Criteria)

- [x] WHEN external profile 执行 `/fork`，系统 SHALL 经 `get_fork_messages` 打开与 `/jump` 同款 prompt 区 ListOverlay 列出可 fork 的 user messages。
- [x] WHEN 用户选择一条消息，系统 SHALL 调用 Pi RPC `fork`，在同一 agent 上切换到新 `sessionId`，`session/load` 回放，并将 `selectedText` 写入 prompt。
- [x] IF 无可 fork 消息或 fork 被取消/失败，THEN 保持当前视图并 toast。
- [x] adapter 保持 headless；不改 Pi 源码；不引入第二套 TUI。
- [x] Grok 非 external profile 的 peer `/fork` 行为不变。

## 实施阶段

### Phase 1: 规划
- [x] 对齐 Pi RPC `fork` / `get_fork_messages` 与 interactive 选择器语义。
- [x] 确认与 tree navigate 的差异：fork 换 session 文件/id；navigate 同文件换 leaf。

### Phase 2: 执行
- [x] adapter：`pi/session/fork_messages`、`pi/session/fork`（refresh bootstrap + 返回 sessionId/text）。
- [x] Pager：external 下 `/fork` → 消息 ArgPicker → fork → bind 新 sessionId → LoadSession → 回填 prompt。
- [x] `PI_GROK_NATIVE_COMMANDS` 注册 `fork`。

### Phase 3: 验证
- [x] `cargo test -p pi-grok-adapter` — 96 passed
- [x] `cargo check -p xai-grok-pager-bin --bin grok-pi` — PASS（仅既有 dead_code 警告）
- [x] 更新 FEATURE_MATRIX / Issue。

## 关键决策

| 决策 | 理由 |
|------|------|
| 使用官方 RPC `fork`/`get_fork_messages`，不注入 extension | RPC 已暴露，无需改 Pi 源码 |
| external 下复用 slash 名 `/fork`，分支到 Pi 语义 | 对齐 Pi；Grok peer fork 仅非 external |
| ArgPicker 承载消息列表 | 复用原生表面；选中后 `/fork <entryId>` |
| 同 agent 换 sessionId，不 spawn peer agent | 匹配 Pi fork（新文件替换 runtime），非 Grok peer |

## Notes

- 调用链：`/fork` → `pi/session/fork_messages` → prompt ListOverlay（同 jump）→ Enter → `pi/session/fork` → `replace_bootstrap` → `LoadSession(newId)` + `prompt.set_text(text)`
- UI 不走 ArgPicker/modal；选择框与 `/jump` 共用 `ListOverlay` shell。
- Pi `position` 默认 `before`（从 user message 的 parent 分叉并返回原文）。
- `/clone`：RPC `clone`（`position: at`），无 picker；成功后清空 prompt 并 `LoadSession`。
- `/reload` 对齐 Pi interactive 门禁：`isStreaming` + `isCompacting`；成功后 `theme::pi::rediscover(cwd)`；toast 文案对齐。
