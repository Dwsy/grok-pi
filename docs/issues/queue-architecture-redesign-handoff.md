# Queue Architecture Redesign — Handoff Prompt

> **Superseded 2026-07-25. Do not continue this handoff.** Phase 2–4 were replaced by an adapter-owned isolated client/extension lane plus a read-only Pi-external mirror; QueueMirror was expanded rather than deleted. Continue from `docs/issues/adapter/20260717-桥接-Pi-queue_update-到-原生队列面板.md` and current source.

## 原目标（已停止）

继续完成 `docs/issues/queue-architecture-redesign.md` 中定义的 Phase 2-4。Phase 1（核心 bug 修复）已完成。

## 已完成的工作（Phase 1 + Phase 5 部分）

### 核心修复：Send Now → Steering 语义

| 文件 | 改动 |
|------|------|
| `dispatch/router.rs:362-367` | `Action::SendPromptNow` 路由到 `dispatch_interject`（不再调用已删除的 `dispatch_send_prompt_now`） |
| `dispatch/interject.rs:29-50` | 新增 reconnect guard（mid-outage 时 requeue 本地而非发送到死通道） |
| `dispatch/prompt.rs:728` | parked-sendable-wait 路径改走 `dispatch_interject` |
| `dispatch/interject.rs` | 删除 `dispatch_send_prompt_now()` 函数（~83 行） |
| `actions.rs` | 删除 `Effect::SendPromptNow` 变体 + `TaskResult::SendPromptNowFailed` 变体 |
| `effects/mod.rs` | 简化 `Effect::SendPromptBlocks` handler（删除 send_now meta/requeue 逻辑） |
| `task_result.rs` | 删除 `SendPromptNowFailed` handler（~47 行） |
| `tests/prompt.rs` + `tests/queue.rs` | 更新 6 个测试断言为 `Effect::SendInterject` |

### 验证状态

- `cargo check -p xai-grok-pager`: **0 errors**
- `cargo test -p xai-grok-pager --lib dispatch::`: **1213 passed, 6 failed**
  - 6 个失败均为预存在问题（foreign sessions ×3, settings ×2, parked notifications ×1），与队列无关
- 净减 **152 行**代码

### 关键决策

- **保留 `Action::SendPromptNow`**：仍被 paste deferred-send (`paste.rs:259`)、InterjectPrompt arm (`prompt.rs:621`)、queue row send-now (`queue.rs:509`) 使用，全部路由到 `dispatch_interject`
- **保留 `push_send_now_user_block` / `arm_send_now_and_paint`**：仍被 `acp_handler/queue.rs:208` 和 `queue.rs:1005`（queue row edit-then-send）使用
- **保留 `send_now_awaiting_confirm` / `optimistic_queue_ids`**：仍被 `acp_handler/queue.rs:169,203` 和 `agent_view/queue.rs:491-568` 使用

---

## 剩余任务

### Phase 2：删除乐观回显 + reconcile（~200 行）

**目标**：删除 follow-up 路径的乐观回显机制，改为"只写不 reconcile"。

**需要删除的代码**：

1. **`push_server_queue_echo` 调用**（2 处活跃调用）：
   - `dispatch/prompt.rs:779` — follow-up immediate-send 路径
   - `dispatch/prompt.rs:923` — bash 路径

2. **`push_server_queue_echo` 函数定义**：
   - `dispatch/queue.rs:75` — 函数体 + `optimistic_queue_ids.insert`

3. **`retire_optimistic_echo` 调用 + 函数**：
   - `dispatch/prompt.rs:1078-1080` — 调用处
   - `dispatch/queue.rs:112` — 函数定义

4. **`optimistic_prompt_echoes` 字段**（AppView 级）：
   - `app_view.rs:864` — 字段声明
   - `app_view.rs:1547, 6224` — 初始化
   - `app_view.rs:2475, 2507, 2541` — `apply_queue_changed` 中的使用

5. **`shared_prompt_queues` 字段**（AppView 级）：
   - `app_view.rs:856` — 字段声明
   - `app_view.rs:1546, 6223` — 初始化
   - `app_view.rs:2470, 2511, 2513, 2548, 2562` — 使用处

6. **`apply_queue_changed` 中的 reconcile 逻辑**：
   - `app_view.rs:2456-2562` — 整个 reconcile 方法（id matching、echo retirement、rekey）
   - `acp_handler/queue.rs:132` — 调用处 `let rekeyed_echo_ids = app.apply_queue_changed(changed);`

7. **`optimistic_queue_ids` 字段**（AgentView 级）：
   - `agent_view/mod.rs:1451` — 字段声明
   - `agent_view/session.rs:312, 375` — 初始化 + clear
   - `agent_view/queue.rs:491-492, 534, 554, 566` — 使用处
   - `dispatch/queue.rs:93` — insert

8. **`send_now_awaiting_confirm` 字段**（AgentView 级）：
   - `agent_view/mod.rs:1460` — 字段声明
   - `agent_view/session.rs:313, 376` — 初始化 + clear
   - `agent_view/queue.rs:492, 528-568` — `resolve_send_now_awaiting_confirm` + 使用
   - `acp_handler/queue.rs:203` — 调用处

**⚠️ 风险**：
- 删除 `push_server_queue_echo` 后，follow-up 路径的队列面板不再立即显示消息（需等待服务器广播）。这是 **UX 变更**，需确认是否可接受。
- `arm_send_now_and_paint`（`queue.rs:716`）和 `push_send_now_user_block`（`queue.rs:671`）仍被 queue row edit-then-send 路径使用（`queue.rs:1005`），**不能删除**。
- `expects_send_now_cancel` / `arm_send_now_expectation`（`agent_view/queue.rs:97,106`）仍被 `arm_send_now_and_paint` 使用，**不能删除**。

**替代方案**：
- 如果不想改变 follow-up 的 UX，可以只删除 `optimistic_prompt_echoes` + `shared_prompt_queues` + `apply_queue_changed` 的 reconcile 逻辑，保留 `push_server_queue_echo` 作为纯显示（不做 id matching）。

### Phase 3：删除适配器 QueueMirror（~180 行）

**前置条件**：Phase 2 完成。

**需要删除的代码**：

1. **`queue_bridge.rs`**（整个文件）：
   - `crates/codegen/pi-grok-adapter/src/queue_bridge.rs` — QueueMirror 结构体 + reconcile 逻辑

2. **`pi_adapter.rs` 中的 mirror 字段**：
   - 搜索 `mirror` / `queue_mirror` / `QueueMirror` 在 `pi_adapter.rs` 中的引用

3. **`lib.rs` 中的模块声明**：
   - `crates/codegen/pi-grok-adapter/src/lib.rs` — `mod queue_bridge;` 或 `pub mod queue_bridge;`

4. **`prompt_bridge.rs` 中的 mirror 交互**：
   - 搜索 `queue_bridge` / `mirror` 在 `prompt_bridge.rs` 中的引用

**⚠️ 风险**：
- 需要确认 `pi-grok-adapter` 的 QueueMirror 是否被 pager 侧的 `apply_queue_changed` 依赖。如果 Phase 2 已删除 `apply_queue_changed`，则 Phase 3 安全。
- 需要 `cargo check -p pi-grok-adapter` 验证。

### Phase 4：Pi 插件拦截（~100 行新增）

**前置条件**：Phase 3 完成。

**需要修改的文件**：

1. **`extensions/pi-grok-rust-tui-bridge/index.ts`**（或 `agent-session.ts`）：
   - 添加 `_externalQueueHandler` hook
   - 在 Pi 的 `session/prompt` 处理中，如果检测到 `_meta.sendNow`，调用 `agent.steer()` 而非 cancel-and-restart

2. **设计决策**：
   - Pi 侧是否需要知道 "steering" 语义？还是 pager 侧已经完全通过 `x.ai/interject` 处理？
   - 如果 pager 侧已经完全通过 interject 处理（Phase 1 已完成），Pi 侧可能不需要任何改动。

**⚠️ 风险**：
- 需要确认 Pi 的 `agent.steer()` API 是否已经存在且稳定。
- 需要确认 `x.ai/interject` 端点是否已经正确路由到 `agent.steer()`。

---

## 并发 Agent 注意事项

另一个 agent 正在重构以下文件，**不要修改**：

- `session_picker.rs` / `session_tree.rs`
- `modals.rs` / `modal_window.rs`
- `foreign_sessions.rs`
- `settings/defs.rs` / `settings/registry.rs`
- `effects/helpers.rs`
- `extensions/*`（TypeScript 文件）
- `shortcut_manager.rs`（已修复其 API 变更：`Shortcut {label, clickable, id}`、`Theme` 字段化、`ModalSizing.width_pct`）

---

## 验证命令

```bash
# 编译检查
cargo check -p xai-grok-pager

# 队列相关测试
cargo test -p xai-grok-pager --lib dispatch::

# 全量测试（确认无回归）
cargo test -p xai-grok-pager --lib

# 适配器编译（Phase 3 后）
cargo check -p pi-grok-adapter
```

## 关键文件索引

| 文件 | 用途 |
|------|------|
| `docs/issues/queue-architecture-redesign.md` | 设计文档（5 阶段计划） |
| `dispatch/router.rs:362-367` | Phase 1 核心改动 |
| `dispatch/interject.rs` | steering 分发 + reconnect guard |
| `dispatch/queue.rs:75-130` | `push_server_queue_echo` + `retire_optimistic_echo`（Phase 2 删除目标） |
| `dispatch/prompt.rs:779,923` | 活跃的 `push_server_queue_echo` 调用（Phase 2 删除目标） |
| `app_view.rs:856-864,2456-2562` | `shared_prompt_queues` + `optimistic_prompt_echoes` + `apply_queue_changed`（Phase 2 删除目标） |
| `agent_view/mod.rs:1451,1460` | `optimistic_queue_ids` + `send_now_awaiting_confirm`（Phase 2 删除目标） |
| `agent_view/queue.rs:97-568` | send-now expectation + resolve 逻辑（部分可删） |
| `acp_handler/queue.rs:132,169,203` | `apply_queue_changed` 调用 + reconcile 使用（Phase 2 删除目标） |
| `pi-grok-adapter/src/queue_bridge.rs` | QueueMirror（Phase 3 删除目标） |

## 6 个预存在的测试失败（与队列无关）

```
app::dispatch::tests::session::foreign::active_modal_owns_stale_and_external_deep_search_results
app::dispatch::tests::session::foreign::modal_external_filter_clears_native_content_and_blocks_forced_search
app::dispatch::tests::session::foreign::modal_external_filter_restores_native_on_close
app::dispatch::tests::settings::every_persisting_setting_has_rollback_arm
app::dispatch::tests::settings::every_setting_has_action_for_reset_arm
app::dispatch::tests::session::parked::parked_notification_shows_on_reconnect
```

这些是并发 agent 的重构引入的，不需要修复。
