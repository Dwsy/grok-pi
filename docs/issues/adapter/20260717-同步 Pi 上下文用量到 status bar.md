---
id: "2026-07-17-同步-pi-上下文用量到-status-bar"
title: "同步 Pi 上下文用量到 status bar"
status: "completed"
created: "2026-07-17"
updated: "2026-07-17"
category: "adapter"
tags: ["workhub", "context", "status-bar"]
---

# Issue: 同步 Pi 上下文用量到 status bar

## Goal

让 Grok 原生右上角 context bar（`8.5K / 1.0M`）在 Pi 后端下显示当前上下文占用。

## 根因

Pager 从 ACP `SessionNotification._meta.totalTokens` 刷新 `context_state.used`，分母来自模型 `totalContextTokens`。Pi adapter 此前从不写入 `totalTokens`，因此 `context_bar_line_for_session` 因 `used` 为 `None` 整段不渲染。

## 修复

- 在 session updates 上附带 `_meta.totalTokens`（缓存最近一次用量）。
- assistant `message_end` 从 message.usage 即时更新。
- `turn_end` / `agent_settled` / `load_session` 通过 `get_session_stats.contextUsage` 校准。

## 验证

- `cargo test -p pi-grok-adapter` → 22 passed
