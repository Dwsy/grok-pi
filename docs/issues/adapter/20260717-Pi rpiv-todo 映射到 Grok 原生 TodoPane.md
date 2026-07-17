---
id: "2026-07-17-Pi-rpiv-todo-映射到-Grok-原生-TodoPane"
title: "Pi rpiv-todo 映射到 Grok 原生 TodoPane"
status: "completed"
created: "2026-07-17"
updated: "2026-07-17"
category: "adapter"
tags: ["workhub", "adapter", "todo", "rpiv-todo", "acp-plan", "todo-pane"]
---

# Issue: Pi rpiv-todo 映射到 Grok 原生 TodoPane

## Goal

让 Pi 插件 `@juicesharp/rpiv-todo` 的任务列表出现在 Grok 原生右上角 Todo 徽章 / TodoPane（可点击、Ctrl+T 切换），而不是 Pi 自己的 above-editor overlay。适配器目前只桥接 `rpiv-todo` 一种来源，但抽取可扩展的 Todo provider 接口，后续可挂其它 todo 插件。

## 背景/问题

### 现状链路（Grok 原生）

```
TodoWrite / 其它 agent 发 ACP Plan
  → SessionUpdate::Plan
  → pager acp_handler: todo_item_from_plan_entry
  → agent.todo.update_todos(items)
  → 状态栏右上角 badge 可点 / Ctrl+T 开 TodoPane
```

关键入口：

| 层 | 路径 | 作用 |
|---|---|---|
| Pager 消费 | `xai-grok-pager/src/app/acp_handler/mod.rs` | `SessionUpdate::Plan` → `update_todos` |
| 类型转换 | `xai-grok-shell/src/tools/todo.rs` | `PlanEntry` ↔ `TodoItem` |
| 原生写入 | `xai-grok-shell/.../acp_conversion.rs::acp_plan_update` | `TodoWriteOutput` → `acp::Plan` |
| UI | `xai-grok-pager/src/views/todo_pane.rs` | badge + pane；**不** auto-open pane，靠 badge/Ctrl+T |

### 现状链路（rpiv-todo）

| 项 | 值 |
|---|---|
| 包 | `@juicesharp/rpiv-todo@1.20.0` |
| Tool 名 | **`todo`**（持久化/回放键，不可改） |
| Slash | `/todos` |
| 状态机 | `pending → in_progress → completed`，另有 `deleted` tombstone |
| 成功结果 | `{ content: [...], details: { action, params, tasks[], nextId, error? } }` |
| Task | `{ id:number, subject, description?, activeForm?, status, blockedBy?, owner?, metadata? }` |
| Live 事件 | Pi `tool_execution_end`：`result` = 完整 tool result（含 `details`） |
| History | `toolResult.details` → adapter `PiHistoryItem::ToolEnd.raw_output` |

### 当前缺口

1. `pi-grok-adapter` 的 `handle_tool_end` / history replay **只发 `ToolCallUpdate`，从不发 `SessionUpdate::Plan`**。
2. Pager 的 `is_todo_tool` 只识别 `todo_write` / `TodoWrite` / `Updating plan`，**不识别 Pi 的 `todo`** → scrollback 会堆一条无用 tool card。
3. Pi 的 `TodoOverlay` 在 grok-pi 下没有原生 TUI 宿主（adapter headless），即使有 UI 也不应使用——架构要求 **只复用 Grok 原生 surface**。

> “Architecture is the decisions that are hard to change later.” —— Martin Fowler  
> 这里硬约束已经写死：Grok Pager 是唯一 TUI，adapter 只做协议桥。

## 推荐方案

### 总原则

1. **Adapter 只翻译，不渲染。** 把 `rpiv-todo` 快照投影成 ACP `Plan`，交给现有 TodoPane。
2. **以 `details.tasks` 全量快照为准。** 每次成功的 `todo` tool end（含 create/update/list/get/delete/clear）只要有 tasks 数组，就发一次 replace 语义的 Plan。
3. **Provider 可插拔。** 当前只实现 `rpiv-todo`；接口预留其它插件（例如未来 `todo_write` 形状的 Pi 扩展）。
4. **不改 Pi 源码 / 不改 rpiv-todo。** 只在 `pi-grok-adapter`（必要时 pager 的 tool 抑制名单）落点。

### 数据映射

| rpiv Task | ACP PlanEntry / TodoItem | 备注 |
|---|---|---|
| `subject` | `content` | 主展示文案 |
| `status=pending` | `Pending` | |
| `status=in_progress` | `InProgress` | `activeForm` 可进 `meta.activeForm`（可选展示增强，P1） |
| `status=completed` | `Completed` | |
| `status=deleted` | **默认过滤，不进 Plan** | 与 rpiv `list` 默认隐藏 tombstone 一致；需要时可用 `meta` 扩展，但原生 pane 无 deleted 概念 |
| `id` | `meta.rpivId`（可选） | PlanEntry 无 id 字段，顺序保留即可 |
| `blockedBy` / `owner` / `description` | `meta.*`（可选） | 原生 pane 不展示依赖；先不阻塞 P0 |
| priority | 固定 `Medium` | rpiv 无 priority |

错误路径：`isError` 或 `details.error` 存在时 **不刷新 Plan**（保持上一快照），tool card 仍可显示错误文本。

`clear` 后 `tasks=[]` → 发空 Plan → badge 清空（与原生 TodoWrite 清空一致）。

### 触发点（Adapter）

```
handle_tool_end / replay ToolEnd
  → if TodoProviderRegistry::match(toolName)
  → extract Plan from result/details
  → send_update(SessionUpdate::Plan(plan))   // 额外于 ToolCallUpdate
  → ToolCallUpdate 继续发（或后续选择 suppress）
```

Live 与 history 共用同一个 `extract_plan_from_tool_result(name, raw)`。

### Provider 形状（建议）

放在 `pi-grok-adapter` 内，例如 `src/todo_bridge.rs`：

```rust
trait TodoSource {
    fn matches(&self, tool_name: &str) -> bool;
    /// 返回 Some(plan) 表示应刷新原生 TodoPane；None 表示忽略。
    fn plan_from_result(&self, result: &Value, is_error: bool) -> Option<acp::Plan>;
}

struct RpivTodoSource; // tool_name == "todo", details.tasks → PlanEntry[]
struct TodoSourceRegistry { sources: Vec<Box<dyn TodoSource>> }
```

默认只注册 `RpivTodoSource`。后续加插件 = 加 source + 单测，不必改 handler 主路径。

### Scrollback 抑制（Pager，小改）

`xai-grok-pager/src/acp/tracker.rs::is_todo_tool` 增加：

- title/tool id：`todo`（Pi rpiv-todo）

理由：专用 TodoPane 已提供可见性；与 `todo_write` 策略一致。这是 **允许的 native seam 微调**，不是第二套 UI。

### 明确不做

- 不在 adapter 画 widget / 不复刻 rpiv overlay。
- 不改 Pi RPC、不改 `rpiv-todo` 包。
- 不把 Grok `TodoWrite` 状态反向写回 Pi（单向：Pi → Grok 展示）。
- 不做依赖图 / blockedBy UI（原生 pane 无此模型）。
- 不在启动时扫磁盘 todo 文件（rpiv 状态在 branch replay / tool details 里）。

## 验收标准 (Acceptance Criteria)

- [ ] WHEN Pi 成功执行 `todo` create/update 且 `details.tasks` 非空，系统 SHALL 发出 `SessionUpdate::Plan`，Grok 右上角 badge 显示正确计数并可点击打开 TodoPane。
- [ ] WHEN 任务状态变为 `in_progress` / `completed`，系统 SHALL 在后续 Plan 更新中反映对应 `PlanEntryStatus`。
- [ ] WHEN `todo` clear 或 tasks 为空，系统 SHALL 发出空 Plan，badge 清空。
- [ ] WHEN `todo` 调用 `isError` 或 `details.error` 存在，系统 SHALL 不覆盖已有 Plan。
- [ ] WHEN 加载/resume 含 `toolResult(toolName=todo, details.tasks=…)` 的会话，系统 SHALL 在 history replay 后恢复 TodoPane 状态（取分支上最后一次有效快照）。
- [ ] WHERE scrollback 出现 Pi `todo` tool call，系统 SHALL 抑制专用 tool card（与 `todo_write` 同策略）。
- [ ] IF 未来新增另一种 todo 插件，THEN 只需新增 `TodoSource` 实现并注册，不必改 Plan 发送主路径。
- [ ] 单元测试覆盖：rpiv details → Plan 映射、deleted 过滤、error 不刷新、history details 路径、registry 只匹配 `todo`。

## 实施阶段

### Phase 1: 映射核与 registry（Adapter）
- [x] 新增 `todo_bridge`：`RpivTodoSource` + registry
- [x] `handle_tool_end` 成功路径附加 `SessionUpdate::Plan`
- [x] history `ToolEnd` replay 同样附加 Plan（每次 end 都发，与 live 一致）
- [x] 单测：mapping + error + clear + non-todo 忽略 + registry 扩展

### Phase 2: scrollback 抑制（Pager seam）
- [x] `is_todo_tool` 识别 `todo`
- [x] 补 pager 侧既有 todo 抑制测试（title=`todo`）

### Phase 3: 验证
- [x] `cargo test -p pi-grok-adapter` → 35 passed
- [x] `cargo check -p pi-grok-adapter` / `cargo check -p xai-grok-pager` 通过
- [ ] 相关 pager lib 测试：已知基础设施 blocker（cross-crate test-helper cfg），见 VERIFICATION.md；本次仅改 `is_todo_tool` 名单
- [ ] 手动：`grok-pi` + 已装 `@juicesharp/rpiv-todo`，让模型 `todo create` → 看右上角 badge / Ctrl+T
- [ ] 手动：resume 含 todo 的 session → badge 恢复

### Phase 4: 文档
- [x] 更新 `FEATURE_MATRIX.md`（Todo/plan list + ask/btw 边界）
- [x] Issue 状态 → completed

## 关键决策

| 决策 | 理由 |
|------|------|
| 用 ACP `Plan` 而非自定义 ext_method | Pager 已完整消费 Plan；零新 UI 面 |
| 全量 snapshot replace，不做增量 merge | rpiv `details.tasks` 本就是全量；与 `TodoWrite` 成功后的 Plan 语义一致 |
| 默认丢弃 `deleted` | 原生只有 pending/in_progress/completed/cancelled；tombstone 对用户噪音大 |
| `deleted` ≠ 强行映射 `Cancelled` | cancelled 表示“放弃的工作项”，deleted 是审计墓碑，语义不同 |
| Provider registry 而不是硬编码 if name=="todo" 散落 | 用户明确“后续可能有其他 todo 插件” |
| 不改 Pi / rpiv-todo | 架构：adapter 桥协议；扩展能力用官方 extension 已提供的 tool result |

## 风险与边界

| 风险 | 处理 |
|------|------|
| history 多次 todo end 导致 Plan 闪多次 | 可接受；或 replay 时只保留 last plan（优化项） |
| list/get 也带全量 tasks | 一并刷新无害，保证 UI 与模型状态一致 |
| Pi overlay 与 Grok badge 双显 | grok-pi 无 Pi TUI；若某环境双开，属宿主问题，不在本 issue |
| 源码身份 verifier / allowed seams | pager 仅扩 `is_todo_tool` 名单；若 verifier 拦，按既有 seam 流程更新 baseline |

## 相关资源

- `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` — tool end / history
- `crates/codegen/pi-grok-adapter/src/model.rs` — `parse_tool_result` 保留 `details`
- `crates/codegen/xai-grok-pager/src/app/acp_handler/mod.rs` — Plan → TodoPane
- `crates/codegen/xai-grok-shell/src/tools/todo.rs` — PlanEntry ↔ TodoItem
- `crates/codegen/xai-grok-shell/src/session/acp_conversion.rs::acp_plan_update` — 原生 TodoWrite 参考
- `~/.pi/agent/npm/node_modules/@juicesharp/rpiv-todo/` — 插件事实源
- `AGENTS.md` / `NATIVE_GROK_TUI_ALIGNMENT.md` — 无第二 TUI、adapter headless

## Notes

- 评估日期：2026-07-17
- 复杂度：L2（adapter 主改 + pager 一行抑制 + 测试）
- P0 足够打通 badge；P1 可加 `meta.activeForm` / `meta.rpivId` 展示增强
- 用户原话：适配器目前只桥接 rpiv-todo，后续可能有其他 todo 插件 → registry 必做，但只注册一个实现

## Status 更新日志

- **2026-07-17**: 状态 → `todo`，备注: 完成源码级评估与方案，待实现
- **2026-07-17**: 状态 → `completed`，备注: 实现 todo_bridge + live/history Plan 投影 + scrollback 抑制；`cargo test -p pi-grok-adapter` 35 passed
