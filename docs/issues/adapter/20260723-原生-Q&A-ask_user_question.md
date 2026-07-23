---
id: "2026-07-23-原生-Q&A-ask_user_question"
title: "原生 Q&A（默认关闭）— Grok QuestionView"
status: "done"
created: "2026-07-23"
updated: "2026-07-23"
category: "adapter"
tags: ["workhub", "ask-user-question", "question-view", "f2", "extension"]
---

# Issue: 原生 Q&A（默认关闭）

## Goal

在 grok-pi 中提供 **默认关闭** 的原生 Q&A 能力：模型可调用 `ask_user_question`，弹出 Grok `QuestionView`，用户提交后答案写回 Pi tool result。

> Grok Build asks the right questions to nail the details.

## 约束

1. 不改 Pi 源码；不改 juicesharp 包。
2. adapter headless；只复用 `x.ai/ask_user_question` → QuestionView。
3. F2 `[ui].pi_ask_user_question` **default OFF**，`external_only`，`restart_required`。
4. 包冲突：`assets/native_feature_conflicts.toml` → `features.pi_ask_user_question`；仅原生 Q&A 开启时 block；F2 描述动态附带 When on, blocks…。

## 设计（路径 1）

| 层 | 职责 |
|---|---|
| Extension `pi-grok-ask-user-question` | 注册 tool；`execute` 轮询 control 响应文件 |
| Adapter | `tool_execution_start` 时用 args 发 `x.ai/ask_user_question`，结果写 control |
| F2 | `pi_ask_user_question` 控制是否注入 extension |
| Pager | 已有 QuestionView，无改 |

## 验收

- [x] 默认 OFF：tool 不在 active tools。
- [x] F2 开 + restart：模型调用弹出原生多题 QuestionView（adapter bridge）。
- [x] 提交 → tool result 含答案；取消 → decline 文案。
- [x] multiSelect / Other notes 尽量保真（QuestionView 原生路径）。

## Status

- **2026-07-23**: done — F2 + extension + adapter control-dir bridge 落地
- **2026-07-23**: 冲突包外挂 TOML + 条件 block；Q&A/goal/workflows/subagents 共用同一表
