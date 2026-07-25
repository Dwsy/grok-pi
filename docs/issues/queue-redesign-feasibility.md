# 可行性分析：Queue 架构重设计

> **历史研究，方案已被 2026-07-25 的混合实现取代。** Pager UI 本身不直接替代 Pi，但 adapter 现在拥有 client/extension 隔离队列，并把绕过拦截的 Pi 私有队列作为只读外部通道。本文仅保留研究过程，当前 SSOT：`docs/issues/adapter/20260717-桥接-Pi-queue_update-到-原生队列面板.md`。

**原结论（已失效）：方案完全可行，无硬性阻塞点。** 有 2 个需要注意的软性约束。

---

## 1. 关键发现：Interject 路径已经存在且语义正确

### 1.1 Shell-native 后端

```
Pager Effect::SendInterject
  → ACP ExtRequest "x.ai/interject"
  → Shell extensions/interject.rs:handle()
  → SessionCommand::Interject { text, id, images }
  → run_loop.rs:
      if turn_running {
          pending_interjections.push(...)  // ← 缓冲，不打断
      } else {
          queue_interjection_fallback_prompt(...)  // ← idle 时作为新 turn
      }
  → drain_pending_interjections():
      // "An interjection never cancels the turn"
      inject_synthetic_user_message(...)  // ← 在下一个安全点注入
```

**✅ 不取消 turn，不打断 AI。语义完全正确。**

### 1.2 Pi Adapter 后端

```
Pager Effect::SendInterject
  → ACP ExtRequest "x.ai/interject"
  → Pi Adapter ext_method():
      "x.ai/interject" => self.handle_steer_message(params).await
  → handle_steer_message():
      Pi RPC { type: "prompt", message, streamingBehavior: "steer" }
  → Pi agent-session.ts:
      _queueSteer(text):
          _steeringMessages.push(text)
          agent.steer({ role: "user", content, timestamp })  // ← 注入，不打断
```

**✅ 不取消 turn，不打断 AI。语义完全正确。**

### 1.3 结论

`x.ai/interject` 是**已有的、统一的、语义正确的** steering 路径。两个后端都已实现。Pager 的 `dispatch_interject()` 已经在使用它。

**当前的 bug 是因为 "Send Now" 走了 `dispatch_send_prompt_now()` → `Effect::SendPromptNow` → shell `queue_input(send_now=true)` → cancel turn，而不是走 `dispatch_interject()`。**

---

## 2. 逐点可行性验证

### 2.1 Send Now → 改走 interject 路径

| 检查项 | 状态 | 说明 |
|--------|------|------|
| `dispatch_interject()` 已存在 | ✅ | `interject.rs:18` |
| `Effect::SendInterject` 已存在 | ✅ | `effects/mod.rs:4045` |
| Shell 处理 `x.ai/interject` | ✅ | `extensions/interject.rs` |
| Pi Adapter 处理 `x.ai/interject` | ✅ | `pi_adapter.rs:2556` → `handle_steer_message` |
| 不取消 turn | ✅ | Shell: "An interjection never cancels the turn"; Pi: `agent.steer()` |
| Scrollback 渲染 | ✅ | `dispatch_interject()` 已有 `push_block(RenderBlock::interjection_prompt(&text))` |
| 多 pane 同步 | ✅ | Shell: `broadcast_interjection()`; Pi: adapter 转发 |
| 图片支持 | ✅ | `dispatch_interject()` 已处理 images → blocks |
| Prompt history | ✅ | `record_interject_prompt_history()` 已存在 |
| 幂等性（连按） | ✅ | 每次 interject 是独立注入，不会追加到队列 |

**改造量**：将 `Action::SendPromptNow` 的路由从 `dispatch_send_prompt_now()` 改为 `dispatch_interject()`。约 5 行代码。

**⚠️ 注意**：`dispatch_interject()` 当前不消费 composer 文本（它接收已提取的 text）。需要确认 `Action::Interject { text, images }` 的 producer 已经做了 `set_text("")`。

验证：`prompt.rs:621`:
```rust
let images = self.prompt.drain_images();
self.prompt.set_text("");  // ← 已清空
return InputOutcome::Action(Action::SendPromptNow { text, images });
```

只需将 `Action::SendPromptNow` 改为 `Action::Interject`，或直接在 router 中将 `SendPromptNow` 路由到 `dispatch_interject()`。

### 2.2 Follow-up → 本地 `pending_prompts` 队列

| 检查项 | 状态 | 说明 |
|--------|------|------|
| `pending_prompts` 已存在 | ✅ | `agent/session.rs` |
| `enqueue_prompt()` 已存在 | ✅ | |
| `maybe_drain_queue()` 已存在 | ✅ | idle 时自动 drain |
| 队列面板渲染 | ✅ | `sync_queue_pane()` 已渲染 local rows |
| Turn 结束后自动发送 | ✅ | `maybe_drain_queue()` 在 turn-end 时被调用 |
| 图片支持 | ✅ | `drain_prompt_state_to_last_queued()` |
| Skill 支持 | ✅ | `enqueue_prompt_with_skill_tokens()` |
| 编辑队列行 | ✅ | `PromptMode::EditingQueued` |
| 删除队列行 | ✅ | `remove_local_queue_row()` |
| 重排序 | ✅ | 本地 VecDeque 操作 |

**改造量**：将 `immediate_server_send` 路径改为 `enqueue_prompt()`。即删除 `immediate_server_send_eligible()` 的 server-busy 分支，统一走本地 enqueue。约 30 行代码变更。

**关键验证**：`maybe_drain_queue()` 的 drain 条件：
```rust
if !agent.session.state.is_idle() { return blocked; }  // turn running → 不 drain
if agent.shared_queue.iter().any(...) { return blocked; }  // ← 需要删除这个检查
```

改造后 `shared_queue` 不再存在，这个检查自然消失。

### 2.3 删除 QueueMirror（Pi Adapter）

| 检查项 | 状态 | 说明 |
|--------|------|------|
| `QueueMirror` 只被 adapter 使用 | ✅ | 只在 `pi_adapter.rs` 中引用 |
| 删除后 Pi 仍能正常工作 | ✅ | Pi 内部的 `_steeringMessages` / `_followUpMessages` 不受影响 |
| `queue_update` 事件处理 | ⚠️ | 需要改为 read-only 转发或忽略 |
| `x.ai/queue/changed` 广播 | ⚠️ | Pi 会话不再需要此广播 |

**改造量**：删除 `queue_bridge.rs` 整个文件 + `pi_adapter.rs` 中约 40 行引用。

**`queue_update` 处理方案**：
- 方案 A：完全忽略（Pi 插件消息不在 pager 队列面板显示）
- 方案 B：转发为只读通知（pager 显示 "[Pi] message queued" toast）
- **推荐方案 A**：插件消息是 Pi 内部行为，用户不需要在队列面板看到

### 2.4 删除 Shell 的 `broadcast_queue_changed`

| 检查项 | 状态 | 说明 |
|--------|------|------|
| Pi 会话是否经过 shell queue | ❌ **不经过** | Pi 会话的 ACP 直接连 adapter，不经过 shell |
| Shell-native 会话是否需要 | ✅ | Shell-native 仍需要（多 pane 同步） |
| 是否可以只删 Pi 路径 | ✅ | Pi 路径根本不触发 shell 的 `queue_input` |

**关键发现**：对于 Pi 会话，shell 的 `pending_inputs` 和 `broadcast_queue_changed` **根本不参与**。Pager 的 ACP 连接直接到 Pi adapter。所以 shell 侧不需要任何修改。

**改造量**：0 行（shell 不需要改）。

### 2.5 Pi 插件消息拦截

| 检查项 | 状态 | 说明 |
|--------|------|------|
| 能否在 adapter 层拦截 | ❌ | 插件 `sendMessage()` 是 Pi 内部调用，不经过 adapter |
| 能否修改 Pi 源码 | ✅ | `pi-main/` 在 repo 中 |
| 是否必须拦截 | ❌ | 可以不拦截，让 Pi 自己管理插件消息 |
| 不拦截的后果 | ⚠️ | 插件消息在 Pi 内部 steer/followUp，pager 不知道 |

**分析**：

插件调 `pi.sendMessage({ deliverAs: "steer" })` 时：
1. Pi 内部 `agent.steer()` 注入当前 turn
2. Pi emit `queue_update` 事件
3. Adapter 收到 `queue_update`（改造后忽略或只读转发）
4. AI 在下一个断点读取插件消息

**这实际上是正确的行为**。插件消息是 Pi 内部行为，不需要 pager 管理。pager 只需要管理**用户输入**的 follow-up 和 steering。

**结论：不需要修改 Pi 源码，不需要拦截插件消息。**

如果未来需要让插件消息也经过 pager（例如显示在队列面板），可以：
1. 在 `agent-session.ts` 加 `_externalQueueHandler` 钩子
2. Adapter 通过 Pi RPC 设置钩子
3. 钩子将消息转发给 pager

但这是 **Phase 2** 的增强，不是 MVP 阻塞点。

### 2.6 多客户端一致性

| 检查项 | 状态 | 说明 |
|--------|------|------|
| Follow-up 队列跨客户端可见 | ⚠️ 降级 | 本地队列是 per-client |
| Steering 跨客户端可见 | ✅ | `broadcast_interjection()` 已处理 |
| 单用户场景 | ✅ | 无影响 |
| Dashboard 模式 | ⚠️ | 其他 pane 看不到 follow-up 队列 |

**缓解**：
- 单用户（绝大多数）：无影响
- 多客户端：follow-up 是 per-client 的（每个客户端有自己的输入框），这是合理的
- 如果需要跨客户端可见性，可以后续加轻量级 "queue hint" 通知

---

## 3. 阻塞点总结

### 硬性阻塞：无

所有改造点都有现成的基础设施支撑，不需要发明新机制。

### 软性约束（需要注意但不阻塞）

| # | 约束 | 影响 | 缓解 |
|---|------|------|------|
| 1 | 多客户端 follow-up 队列不再同步 | Dashboard 模式下其他 pane 看不到队列 | 可接受；后续可加只读通知 |
| 2 | Pi 插件消息不在 pager 队列面板显示 | 用户看不到插件排了什么消息 | 可接受；插件消息是 Pi 内部行为 |

### 需要确认的设计决策

| # | 决策 | 选项 | 推荐 |
|---|------|------|------|
| 1 | Send Now 的 scrollback 渲染 | A: `interjection_prompt` 样式 / B: `user_prompt` 样式 | A（区分于普通消息） |
| 2 | 空 composer + 队列有行时按 Send Now | A: 发送队列第一行（interject） / B: no-op | A（保持现有 UX） |
| 3 | Pi `queue_update` 事件 | A: 忽略 / B: 只读 toast | A（MVP 简单） |
| 4 | Shell-native 会话是否也改 | A: 一起改 / B: 只改 Pi 路径 | A（统一语义） |

---

## 4. 改造路径（最小可行）

### Step 1: 路由修改（5 行）

```rust
// router.rs — 将 SendPromptNow 路由到 interject
Action::SendPromptNow { text, images } => {
    // 改前: super::interject::dispatch_send_prompt_now(app, text, images)
    // 改后:
    super::interject::dispatch_interject(app, text, images)
}
```

### Step 2: Follow-up 统一走本地（30 行）

```rust
// dispatch/prompt.rs — 删除 immediate_server_send 分支
// 改前:
if immediate_server_send {
    // ... 40 行 server-send 逻辑
    return vec![Effect::SendPrompt { ... }];
}
agent.session.enqueue_prompt_with_skill_tokens(text, skill_token_ranges);

// 改后:
// 统一走本地 enqueue（turn running 时留在队列，idle 时立即 drain）
agent.session.enqueue_prompt_with_skill_tokens(text, skill_token_ranges);
```

### Step 3: 删除 mirror 相关代码（~200 行删除）

- `queue_bridge.rs` 整个文件
- `pi_adapter.rs` 中 `queue_mirror` 相关
- `app_view.rs` 中 `optimistic_prompt_echoes` / `shared_prompt_queues` / `apply_queue_changed`
- `dispatch/queue.rs` 中 `push_server_queue_echo` / `retire_optimistic_echo`
- `agent_view/queue.rs` 中 `send_now_awaiting_confirm` / `optimistic_queue_ids`

### Step 4: 简化 `maybe_drain_queue`（10 行删除）

```rust
// 删除 server_queue 检查:
// 改前:
if agent.shared_queue.iter().any(|e| Some(e.id.as_str()) != running) {
    return QueueDrain::blocked();
}
// 改后: 删除这个 block
```

### Step 5: 简化 `sync_queue_pane`（20 行删除）

```rust
// 改前: sync_from_merged(local, server, running_id, send_now_id, painted)
// 改后: sync_from_local(local)  — 只渲染本地队列
```

---

## 5. 验证清单

改造完成后，以下场景必须通过：

- [ ] Idle 按 Enter → 立即发送
- [ ] Turn running 按 Enter → 消息进入本地队列，turn 结束后自动发送
- [ ] Turn running 按 Send Now → 消息注入当前 turn（不打断）
- [ ] 快速连按 Send Now ×5 → 5 条独立 interjection，无重复
- [ ] 空 composer + 队列有行 + 按 Send Now → 发送队列第一行
- [ ] Pi 插件 `sendMessage({ deliverAs: "steer" })` → Pi 内部处理，不影响 pager
- [ ] Pi 插件 `sendMessage({ deliverAs: "followUp" })` → Pi 内部处理，不影响 pager
- [ ] 多 pane 查看同一 session → interjection 在所有 pane 显示
- [ ] 队列编辑/删除/重排 → 正常工作（本地操作）
- [ ] Reconnect 期间按 Send Now → 本地 requeue
- [ ] Goal turn 期间按 Send Now → steering 注入（不 cancel goal）
- [ ] `/loop` 插件的 `updateStatus` → 不进入队列

---

## 6. 最终结论

| 维度 | 评估 |
|------|------|
| 技术可行性 | ✅ 完全可行，所有基础设施已存在 |
| 硬性阻塞 | ❌ 无 |
| 改造量 | ~250 行删除 + ~40 行修改 |
| 风险 | 低（主要是删除代码，新增极少） |
| 向后兼容 | ⚠️ 多客户端 follow-up 可见性降级（可接受） |
| Pi 源码修改 | ❌ 不需要（MVP） |
| Shell 源码修改 | ❌ 不需要 |
| 核心改动 | Pager 路由 + 删除 mirror |

**方案通畅，可以执行。**
