---
id: "2026-07-17-Pi-rpiv-ask-user-question-RPC-映射-QuestionView"
title: "Pi rpiv-ask-user-question 在 RPC 下映射 Grok QuestionView"
status: "todo"
created: "2026-07-17"
updated: "2026-07-17"
category: "adapter"
tags: ["workhub", "adapter", "ask-user-question", "rpiv-ask-user-question", "rpc", "question-view"]
---

# Issue: Pi rpiv-ask-user-question 在 RPC 下映射 Grok QuestionView

## Goal

在 **不修改 `@juicesharp/rpiv-ask-user-question` 源码**、且运行形态为 **Pi JSONL RPC + grok-pi** 的前提下，评估（并在可行时实现）如何把该插件的结构化问卷映射到 Grok 原生 `QuestionView`（`x.ai/ask_user_question`）。

## 约束（用户确认）

1. 不修改 juicesharp 插件源码。
2. 宿主是 Pi RPC 模式 + pi-grok adapter + Grok Pager（无 Pi 原生 TUI）。
3. 架构：adapter headless；只复用原生 QuestionView，不建第二套问卷 UI。

## 背景 / 事实

### 插件行为

| 项 | 值 |
|---|---|
| 包 | `@juicesharp/rpiv-ask-user-question@1.20.0` |
| Tool | **`ask_user_question`** |
| UI 路径 | `ctx.ui.custom((tui, theme, kb, done) => QuestionnaireSession…)` |
| 输入 | 1–4 题；每题 2–4 options；`multiSelect?`；`options[].preview?` |
| 输出 details | `{ answers[], cancelled, error? }`，answer kind: `option\|custom\|chat\|multi` |

### Pi RPC 对 UI 的支持（bundled pi-main）

| `ctx.ui.*` | RPC | adapter 现状 |
|---|---|---|
| `select` / `confirm` / `input` / `editor` | `extension_ui_request` | **已桥接** → `x.ai/ask_user_question` → QuestionView |
| `notify` / `setStatus` / `setTitle` / `setWidget` / `set_editor_text` | 有 | 已部分转发 |
| **`custom(factory)`** | **stub：`return undefined as never`** | 无；且 factory 是函数，**不可 JSON 序列化** |

源码：`pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts` 的 `async custom() { return undefined as never }`。

因此在 grok-pi 下，rpiv-ask 的 `execute`：

1. `ctx.hasUI` 为 true（有 RPC uiContext）→ 不会走 `no_ui` 早退；
2. 调用 `custom()` → 立即得到 `undefined`；
3. `buildQuestionnaireResponse(null)` → **固定 DECLINE**（“User declined to answer questions”）；
4. 用户 **从未看到** 任何问卷 UI。

### 与 Grok 原生 QuestionView 的能力对照

| 能力 | Grok QuestionView / `x.ai/ask_user_question` | rpiv-ask |
|---|---|---|
| 多题 | 有 | 有（≤4） |
| multiSelect | 有 | 有 |
| option label/description | 有 | 有 |
| option preview | 有（annotation） | 有 |
| freeform / notes | 有（Other + notes） | Type something / notes |
| Chat about this | plan mode 路径 | 有（sentinel） |
| Submit 汇总 tab | 原生有自己的交互 | 插件有 Submit tab |
| 阻塞工具直到用户答完 | 原生 tool 阻塞在 shell | 插件阻塞在 `custom()` |

形状高度同构；**协议层几乎可 1:1 映射**。卡点不在 QuestionView，而在 **插件只走 `custom(factory)`，RPC 无法传函数**。

## 结论（评估）

### 在「不改插件源码 + 纯 adapter」下

**不可完整适配。** 原因：

1. 交互发生在 Pi 进程内的 `ctx.ui.custom(factory)`；
2. factory 不可序列化 → 无法变成 `extension_ui_request`；
3. adapter 只能看到 `tool_execution_start/end`，此时要么已 decline，要么仍在 Pi 内阻塞（若有人改 custom 但仍无 UI）；
4. adapter **不能** 在不打断 Pi tool execute 的情况下「劫持」同一 tool call 并代答。

> “You can't serialize a closure over a wire.” —— 分布式系统常识  
> 这里的 factory 就是闭包；RPC 边界把它砍死了。

### 与现有 adapter 能力的关系

已有 `ask_extension_question` 只服务 Pi **原语对话框**（select/confirm/input/editor）。  
rpiv-ask **不调用这些原语**，所以现有桥 **零覆盖** 该插件。

### 若放宽「不改 Pi」但仍不改 juicesharp 包

| 方案 | 改哪里 | 是否满足不改插件 | 评价 |
|---|---|---|---|
| A. 改 bundled `rpc-mode.custom` 支持某种序列化问卷 | pi-main | 是 | **仍不够**：插件传入的是 factory，不是问卷 JSON；不改插件就拿不到 questions 结构从 custom 入口 |
| B. 在 tool_execution_start 用 adapter 弹 QuestionView，并伪造 tool result | adapter only | 是 | **不可行**：Pi 侧 tool 已在跑自己的 execute，结果由 Pi 写 session；adapter 无法替换 Pi 内部 tool result |
| C. **pi-grok 自有 extension** 注册同名 `ask_user_question`，内部用 select 原语或直接走 host 协议 | 新 extension（不改 juicesharp） | 是 | 与 juicesharp **工具名冲突**；需卸载/禁用 juicesharp 包 |
| D. 卸载 juicesharp，只用 Grok 后端自带 AskUserQuestion | 无 Pi 插件 | 是 | 仅原生 Grok shell 有；Pi 后端无此 tool，除非 C |
| E. 上游改 juicesharp：RPC 下改走 `select` 循环或 emit 结构化 event | juicesharp | **否** | 产品正确，但违反当前约束 |

**在当前硬约束下：没有诚实的 P0 实现路径能让「已安装的 juicesharp 包」在 grok-pi 里真正弹出 QuestionView。**

## 可选后续（需用户拍板）

### 路径 1 — 推荐产品路径（换皮不改原包）

- 提供 **pi-grok 自有** 扩展（仓库内 / 可选安装），实现同名 tool `ask_user_question`：
  - 参数 schema 对齐 rpiv / Grok；
  - execute 时通过 **已存在的** `extension_ui_request` 原语 **或** 新增单一 `extension_ui_request.method = "ask_user_question"`（结构化 questions JSON）→ adapter → QuestionView；
  - 用户 **禁用/不装** `@juicesharp/rpiv-ask-user-question`。
- 满足：不改 juicesharp 源码、RPC only、原生 UI。
- 代价：不是「映射该 npm 包」，而是「功能对等替换」。

### 路径 2 — 改 Pi RPC + 仍不改 juicesharp

- **不可单独成立**（见上表 A）。除非再加对 tool runner 的劫持，侵入过大，否决。

### 路径 3 — 接受现状 + 文档

- FEATURE_MATRIX 标明：`rpiv-ask-user-question` 在 grok-pi = **始终 decline**；
- 引导使用路径 1 或等待上游 RPC-friendly 实现。

## 若走路径 1 的映射草案（备查）

### Request：rpiv params → `AskUserQuestionExtRequest`

| rpiv | Grok |
|---|---|
| `questions[i].question` | `Question.question` / header 可塞 meta 或拼接 |
| `questions[i].header` | 可用作短标题；Grok `Question` 若无 header 字段则并入 question 前缀 |
| `options[].label/description/preview` | `QuestionOption` 同名字段 |
| `multiSelect` | `multiSelect` |
| mode | 默认 `Default`（Pi 无 plan-mode 上下文时可固定） |

### Response：`AskUserQuestionExtResponse` → rpiv `QuestionnaireResult`

| Grok outcome | rpiv |
|---|---|
| `Accepted` answers/annotations | `kind: option\|multi\|custom`；notes → `notes`；Other+notes → `custom` |
| `ChatAboutThis` | `kind: chat` 或 cancelled + 特殊 envelope（需对齐模型提示词） |
| `Cancelled` | `cancelled: true` + DECLINE_MESSAGE |

## 验收标准（仅当选择可实现路径后）

- [ ] WHEN 模型调用桥接后的 `ask_user_question`，系统 SHALL 弹出 Grok QuestionView 而非 silent decline。
- [ ] WHEN 用户提交选项，系统 SHALL 把答案写回 Pi tool result，模型继续。
- [ ] WHEN 用户取消，系统 SHALL 返回 cancelled/decline envelope。
- [ ] WHERE multiSelect / preview / freeform notes，系统 SHALL 尽量保真映射。
- [ ] 不修改 `@juicesharp/rpiv-ask-user-question` 包内文件。

## 决策（待确认）

| 选项 | 建议 |
|---|---|
| 坚持「映射已装 juicesharp 包」 | **不可行** → 文档 + 矩阵标红 |
| 接受「功能对等自有 extension」 | **可行 P0**，与 todo 适配同优先级可排期 |
| 改 juicesharp 上游 | 超出当前约束 |

## Notes

- 与 todo 适配对比：todo 是 **tool result → Plan 单向投影**（adapter 可做）；ask 是 **双向阻塞 UI**（必须在 Pi 进程内有可 RPC 的 UI 原语）。
- 现有 `FEATURE_MATRIX` 已写 select/confirm/input/editor 原生+适配；**未**覆盖 rpiv-ask 的 custom 问卷。
- 评估日期：2026-07-17

## Status 更新日志

- **2026-07-17**: 状态 → `todo`，备注: 完成 RPC 约束下的可行性评估；默认结论为不可直接桥接 juicesharp 包
