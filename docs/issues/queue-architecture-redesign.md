# Issue: 消息队列架构重设计 — 消除四层 Reconcile，Pager 统一控制 Steering / Follow-up

**Severity**: Critical (用户可复现的消息重复 bug)
**Component**: `xai-grok-pager` / `pi-grok-adapter` / `xai-grok-shell` / `pi-main`
**Labels**: `architecture`, `queue`, `steering`, `follow-up`, `duplicate-message`, `pi-plugin`
**Status**: Historical / superseded — do not implement as written

> **2026-07-25 implementation note:** This document preserves the original failure analysis, but the phase plan was not applied literally. The shipped design uses an adapter-owned isolated lane for client/extension messages plus a read-only mirrored Pi-external lane; QueueMirror remains the protocol boundary. Use `docs/issues/adapter/20260717-桥接-Pi-queue_update-到-原生队列面板.md` and live source as SSOT.

---

## 目录

1. [问题描述](#1-问题描述)
2. [根因分析](#2-根因分析)
3. [完整链路追踪](#3-完整链路追踪)
4. [可行性验证](#4-可行性验证)
5. [FIFO 排序修正](#5-fifo-排序修正)
6. [最终改造方案](#6-最终改造方案)
7. [Pi 插件消息拦截方案](#7-pi-插件消息拦截方案)
8. [删除/保留清单](#8-删除保留清单)
9. [迁移策略](#9-迁移策略)
10. [测试矩阵](#10-测试矩阵)
11. [风险评估](#11-风险评估)
12. [工作量估算](#12-工作量估算)
13. [附录：关键代码位置索引](#13-附录关键代码位置索引)

---

## 1. 问题描述

### 1.1 用户复现路径

1. 发送消息 A，AI 开始输出长文本（turn running）
2. 在输入框输入消息 B
3. 按 Send Now（Ctrl+O / InterjectPrompt）
4. 消息 B 出现在队列面板
5. **再次按 Send Now** → 消息 B 被重复追加到队列
6. 持续按 Send Now → 队列无限增长，同一条消息被反复追加

### 1.2 期望行为

- **Steering**（Pi 语义）：消息注入当前 turn 上下文，**不打断 AI 会话**，AI 在下一个自然断点读取并响应
- **Follow-up**：消息排队等待当前 turn 结束后，作为新的 user turn 自动发送
- 两种行为都不应产生重复消息
- Pi 插件（如 `loop.ts`）的 `sendUserMessage` 消息应与用户消息保持 FIFO 排序

### 1.3 实际行为

- Send Now 走 shell 的 `send_now=true` 路径，**取消当前 turn 并重启**（语义错误）
- 每次按键都 mint 新 `prompt_id`，推入乐观回显队列
- 四层 reconcile 失败导致回显不被 retire，新行不断追加
- Pi 插件（如 `loop.ts`）通过 `pi.sendUserMessage()` 直接操作 Pi 内部队列，与 pager 的 mirror 冲突

### 1.4 Steering 语义错配

**Pi 的 steering 语义**（`agent-session.ts:1435`）：
```typescript
if (this.isStreaming) {
    // steer = 注入当前 turn，不打断
    this._steeringMessages.push(messageText);
    this.agent.steer(appMessage);  // ← 非破坏性注入
}
```

**Pager 的 send-now 实际行为**（`prompt_queue.rs:222`）：
```rust
let cancel_running_turn = send_now && turn_running && !goal_active;
// ↑ 取消当前 turn！这是 cancel-and-restart，不是 steering
```

| 操作 | Pi 语义 | Pager 实际执行 |
|------|---------|---------------|
| Steering | `agent.steer()` — 注入，不打断 | `cancel_turn_for_send_now()` — 取消并重启 |
| Follow-up | `agent.followUp()` — 排队等 turn 结束 | `pending_inputs.push_back()` — 正确 |

---

## 2. 根因分析

### 2.1 四层队列架构（现状）

```
┌─ Layer 1: Pager 本地队列 ─────────────────────────────────────┐
│  pending_prompts: VecDeque<QueuedPrompt>                       │
│  optimistic_prompt_echoes: HashMap<session_id, Vec<QueueEntry>>│
│  shared_prompt_queues: HashMap<session_id, Vec<QueueEntry>>    │
│  send_now_painted_blocks: HashMap<prompt_id, (EntryId, bool)>  │
│  send_now_awaiting_confirm: Option<String>                     │
└────────────────────────────────────────────────────────────────┘
         │ Effect::SendPromptNow / Effect::SendPrompt
         ▼
┌─ Layer 2: Shell 服务端队列 ───────────────────────────────────┐
│  pending_inputs: VecDeque<InputItem>                           │
│  running_prompt_id: Option<String>                             │
│  broadcast_queue_changed() → x.ai/queue/changed               │
└────────────────────────────────────────────────────────────────┘
         │ ACP PromptRequest { sendNow: true }
         ▼
┌─ Layer 3: Pi Adapter Mirror ──────────────────────────────────┐
│  QueueMirror { entries, reserved, running_prompt_id }          │
│  apply_queue_update(steering, followUp) → reconcile by text    │
│  publish_queue_snapshot() → x.ai/queue/changed (第2次广播)     │
└────────────────────────────────────────────────────────────────┘
         │ Pi RPC { type:"prompt", streamingBehavior:"steer" }
         ▼
┌─ Layer 4: Pi 内部队列 ────────────────────────────────────────┐
│  _steeringMessages: string[]                                   │
│  _followUpMessages: string[]                                   │
│  agent.steer(message) / agent.followUp(message)                │
│  _emitQueueUpdate() → queue_update event                       │
└────────────────────────────────────────────────────────────────┘
```

### 2.2 根因 #1：双重广播 + Text 匹配失败

**Shell 广播**（`prompt_queue.rs:270`）：
```rust
// queue_input() 完成后
self.broadcast_queue_changed(&state);
```

**Pi Adapter 广播**（`pi_adapter.rs:1120`）：
```rust
async fn apply_pi_queue_update(&self, event: &Value) {
    let steering = string_list(event.get("steering"));
    let follow_up = string_list(event.get("followUp"));
    state.queue_mirror.apply_queue_update(&steering, &follow_up);
    self.publish_queue_snapshot().await;  // ← 第2次 x.ai/queue/changed
}
```

**QueueMirror reconcile 靠 text 精确匹配**（`queue_bridge.rs:88`）：
```rust
if let Some(pos) = self.reserved.iter()
    .position(|item| item.lane == *lane && item.text == *text)
//                                       ^^^^^^^^^^^^^^^^^^^^^^^^
// Pi 内部可能对 text 做了处理（<user_query> 标签、trim、wrap）
// 导致 text 不匹配 → 分配新 id "pi-queue-N"
```

**Pager 的 `apply_queue_changed` 靠 id 匹配 retire 乐观回显**（`app_view.rs:2365`）：
```rust
opt.retain(|e| {
    let id_matches_running = running_prompt_id.as_deref() == Some(e.id.as_str());
    let id_matches_entry = entries.iter().any(|x| x.id == e.id);
    // ↑ 如果 mirror 分配了新 id，这里匹配不上
    // → 乐观回显不被 retire → 被 pin 到 entries 末尾
    !retired
});
for e in opt.iter() {
    if !entries.iter().any(|x| x.id == e.id) {
        entries.push(pinned);  // ← 追加！
    }
}
```

### 2.3 根因 #2：快速连按的级联效应

```
第1次按 Send Now:
  composer="B" → set_text("") → Action::SendPromptNow { text: "B" }
  → mint prompt_id_1 → push_server_queue_echo(prompt_id_1, "B")
  → Effect::SendPromptNow → shell queue_input(send_now=true)
  → shell broadcast: entries=[{id: prompt_id_1, text: "B"}]
  → Pi queue_update: steering=["B"]
  → mirror: reserve(prompt_id_1, "B") vs Pi text "B" → 匹配？
    → 如果 Pi 加了 <user_query> 标签 → 不匹配 → 新 id "pi-queue-1"
    → pager 收到 entries=[{id: "pi-queue-1", text: "B"}]
    → 乐观回显 prompt_id_1 不被 retire → pin 到末尾
    → 队列显示: [pi-queue-1: "B", prompt_id_1: "B"]  ← 重复！

第2次按 Send Now:
  composer="" → try_send_now_queued_from_prompt()
  → sync_queue_pane() → 找到第一行（server row "pi-queue-1"）
  → force_interject_queue_row(id)
  → is_server=true → Action::QueueInterjectShared { id: "pi-queue-1" }
  → dispatch_queue_interject_shared() → arm_send_now_and_paint()
  → Effect::QueueInterject → shell x.ai/queue/interject
  → 又触发一次 broadcast...
  → 又追加一行...
```

### 2.4 根因 #3：Pi 插件绕过 ACP

**插件调用链**（`agent-session.ts:2343`）：
```typescript
// 插件调 pi.sendMessage()
sendMessage: (message, options) => {
    this.sendCustomMessage(message, options)  // ← 直接调 Pi 内部
},
```

**`sendCustomMessage` 在 streaming 时**（`agent-session.ts:1430`）：
```typescript
if (this.isStreaming) {
    if (options?.deliverAs === "followUp") {
        this._followUpMessages.push(messageText);
        this._emitQueueUpdate();  // ← 触发 queue_update 事件
        this.agent.followUp(appMessage);
    } else {
        this._steeringMessages.push(messageText);
        this._emitQueueUpdate();  // ← 触发 queue_update 事件
        this.agent.steer(appMessage);
    }
}
```

**问题**：
- 插件消息完全不经过 ACP prompt 路径
- Adapter 只能通过 `queue_update` 事件事后知道
- 事件到达时 mirror 已经和 pager 的乐观回显冲突
- 无法区分"用户发的"和"插件发的"

### 2.5 根因 #4：Pi 会话不经过 Shell

**关键发现**：对于 Pi 会话，pager 的 ACP 连接直接到 Pi adapter，**不经过 shell 的 `pending_inputs`**。Shell 的 `queue_input` / `broadcast_queue_changed` 只对 shell-native 会话生效。

这意味着：
- Pi 会话的 "Send Now" 走 `Effect::SendPromptNow` → adapter `prompt()` → Pi RPC `streamingBehavior: "steer"`
- Shell 的 `pending_inputs` 和 `broadcast_queue_changed` 对 Pi 会话**完全不参与**
- 但 pager 代码中 `immediate_server_send_eligible` 等逻辑仍然尝试走 shell 路径（历史遗留）

---

## 3. 完整链路追踪

### 3.1 Send Now 链路（现状 — Pi 会话）

```
用户按 Ctrl+O (InterjectPrompt)
│
├─ prompt.rs:621
│   let text = self.prompt.text().trim().to_string();
│   let images = self.prompt.drain_images();
│   self.prompt.set_text("");
│   return InputOutcome::Action(Action::SendPromptNow { text, images });
│
├─ router.rs:359
│   Action::SendPromptNow { text, images } => {
│       super::interject::dispatch_send_prompt_now(app, text, images)
│   }
│
├─ interject.rs:88 — dispatch_send_prompt_now()
│   let prompt_id = uuid::Uuid::new_v4().to_string();
│   agent.note_self_originated_prompt(&prompt_id);
│   if agent.expects_send_now_cancel() {
│       agent.arm_send_now_expectation(prompt_id.clone());
│       super::queue::push_send_now_user_block(agent, &prompt_id, "prompt", &text, false);
│   }
│   super::queue::push_server_queue_echo(app, id, &sid_str, &prompt_id, &text, "prompt");
│   vec![Effect::SendPromptNow { agent_id, session_id, blocks, prompt_id }]
│
├─ effects/mod.rs:1621 — Effect::SendPromptNow
│   let mut meta = prompt_request_meta(&prompt_id, screen_mode);
│   map.insert("sendNow".into(), serde_json::Value::Bool(true));
│   let req = acp::PromptRequest::new(session_id, blocks).meta(meta);
│   acp_send(req, &tx).await
│
├─ Pi Adapter: prompt() (pi_adapter.rs:2339)
│   streaming_behavior = prompt_streaming_behavior(already_active=true, meta={sendNow:true})
│   // → 返回 "steer"
│   queue_mirror.reserve(client_prompt_id, text, QueueLane::Steering)
│   Pi RPC: { type:"prompt", message, streamingBehavior:"steer" }
│
├─ Pi 内部: agent-session.ts prompt()
│   if (this.isStreaming && options.streamingBehavior === "steer") {
│       await this._queueSteer(expandedText, currentImages);
│   }
│   _queueSteer():
│       _steeringMessages.push(text)
│       _emitQueueUpdate()  // → queue_update event
│       agent.steer({ role: "user", content, timestamp })  // ← 注入，不打断
│
├─ Pi Adapter: handle_event("queue_update") (pi_adapter.rs:1397)
│   apply_pi_queue_update()
│   queue_mirror.apply_queue_update(steering=["B"], followUp=[])
│   // reconcile: reserved text "B" vs Pi text "B" → 匹配？
│   // 如果匹配 → 复用 client_prompt_id → pager 能 retire
│   // 如果不匹配 → 新 id "pi-queue-N" → pager 无法 retire → 重复！
│   publish_queue_snapshot()  ← x.ai/queue/changed 广播
│
└─ Pager: acp_handler/queue.rs:119
    let rekeyed_echo_ids = app.apply_queue_changed(changed);
    // reconcile 乐观回显...
    // 如果 id 不匹配 → 回显被 pin 到末尾 → 重复！
```

### 3.2 Interject 链路（已有 — 语义正确）

```
Pager Effect::SendInterject
│
├─ effects/mod.rs:4045
│   let request = acp::ExtRequest::new("x.ai/interject", params);
│   acp_send(request, &tx).await
│
├─ [Shell-native 后端]
│   Shell extensions/interject.rs:handle()
│   → SessionCommand::Interject { text, id, images }
│   → run_loop.rs:862:
│       if turn_running {
│           pending_interjections.push(PendingInterjection { text, attachments })
│           // ← 缓冲，不打断
│       } else {
│           queue_interjection_fallback_prompt(text, images, true)
│           // ← idle 时作为新 turn
│       }
│   → drain_pending_interjections() (interjection.rs:288):
│       // 在 process_conversation_turn 的下一个安全点调用
│       inject_synthetic_user_message(&wrapped, item, false, &images)
│       // "An interjection never cancels the turn"
│
├─ [Pi Adapter 后端]
│   pi_adapter.rs:2556 ext_method():
│       "x.ai/interject" => self.handle_steer_message(params).await
│   → handle_steer_message() (pi_adapter.rs:2984):
│       queue_mirror.reserve(client_id, message, QueueLane::Steering)
│       Pi RPC: { type:"prompt", message, streamingBehavior:"steer" }
│   → Pi agent-session.ts:
│       _queueSteer(text):
│           _steeringMessages.push(text)
│           agent.steer({ role: "user", content, timestamp })
│           // ← 注入，不打断
│
└─ 两个后端都不取消 turn。语义完全正确。
```

### 3.3 Follow-up 链路（现状 — 普通 Enter 在 turn running 时）

```
用户按 Enter (composer 有文本, turn running)
│
├─ prompt.rs:562
│   if let Some(text) = self.prompt.try_send() {
│       let action = action_mode.send_action(text);  // Action::SendPrompt(text)
│   }
│
├─ dispatch/prompt.rs:690 — dispatch_send_prompt_inner()
│   let immediate_server_send = immediate_server_send_eligible(agent) && images.is_empty();
│
│   if immediate_server_send {
│       // → Effect::SendPrompt → ACP PromptRequest (无 sendNow meta)
│       // → Pi Adapter: streaming_behavior = "followUp" (因为 already_active=true, 无 sendNow)
│       // → Pi: _queueFollowUp(text) → agent.followUp()
│       // → 同时 push_server_queue_echo() → 乐观回显
│   } else {
│       // → 本地 enqueue → maybe_drain_queue()
│       // → 如果 idle → 立即 drain → Effect::SendPrompt
│       // → 如果 busy → 留在本地队列等 turn 结束
│   }
```

### 3.4 Pi 插件消息链路（现状）

```
loop.ts: pi.sendUserMessage(loopState.prompt, { deliverAs: "followUp" })
│
├─ agent-session.ts:2352
│   sendUserMessage → this.prompt(text, { streamingBehavior: "followUp", expandPromptTemplates: false })
│
├─ agent-session.ts:1139 (prompt 方法)
│   if (this.isStreaming) {
│       if (options.streamingBehavior === "followUp") {
│           await this._queueFollowUp(expandedText, currentImages);
│       }
│   }
│
├─ agent-session.ts:1376 (_queueFollowUp)
│   this._followUpMessages.push(text);
│   this._emitQueueUpdate();  // → queue_update event
│   this.agent.followUp({ role: "user", content, timestamp });
│
├─ Pi Adapter: handle_event("queue_update")
│   apply_pi_queue_update()
│   // 此时 mirror 可能已经有 pager 的 reserved 条目
│   // 插件消息的 text 和 reserved 不匹配 → 新 id
│   publish_queue_snapshot()  ← 广播给 pager
│
└─ Pager: apply_queue_changed()
    // 收到一个从未见过的 id → 追加到队列显示
    // 用户看到一条"幽灵"队列行
```

---

## 4. 可行性验证

### 4.1 结论：方案完全可行，无硬性阻塞点

### 4.2 Send Now → 改走 interject 路径

| 检查项 | 状态 | 说明 |
|--------|------|------|
| `dispatch_interject()` 已存在 | ✅ | `interject.rs:18` |
| `Effect::SendInterject` 已存在 | ✅ | `effects/mod.rs:4045` |
| Shell 处理 `x.ai/interject` | ✅ | `extensions/interject.rs` → `SessionCommand::Interject` |
| Pi Adapter 处理 `x.ai/interject` | ✅ | `pi_adapter.rs:2556` → `handle_steer_message` |
| 不取消 turn | ✅ | Shell: "An interjection never cancels the turn"; Pi: `agent.steer()` |
| Scrollback 渲染 | ✅ | `dispatch_interject()` 已有 `push_block(RenderBlock::interjection_prompt(&text))` |
| 多 pane 同步 | ✅ | Shell: `broadcast_interjection()`; Pi: adapter 转发 |
| 图片支持 | ✅ | `dispatch_interject()` 已处理 images → blocks |
| Prompt history | ✅ | `record_interject_prompt_history()` 已存在 |
| 幂等性（连按） | ✅ | 每次 interject 是独立注入，不会追加到队列 |
| Composer 已清空 | ✅ | `prompt.rs:621` 在发 Action 前已 `set_text("")` |

**改造量**：将 `Action::SendPromptNow` 的路由从 `dispatch_send_prompt_now()` 改为 `dispatch_interject()`。约 5 行代码。

### 4.3 删除 QueueMirror（Pi Adapter）

| 检查项 | 状态 | 说明 |
|--------|------|------|
| QueueMirror 是否被其他模块依赖 | ✅ 无 | 只在 `pi_adapter.rs` 内部使用 |
| 删除后 Pi 会话是否受影响 | ✅ 不受 | Pi 内部 `_steeringMessages` / `_followUpMessages` 仍然工作 |
| `queue_update` 事件是否可以忽略 | ✅ 可以 | 改造后 pager 不需要 mirror 来显示队列 |
| `handle_steer_message` 是否依赖 mirror | ⚠️ 部分 | 有 `queue_mirror.reserve()` 调用，删除即可 |

### 4.4 Shell 不需要修改

**关键发现**：对于 Pi 会话，shell 的 `pending_inputs` 和 `broadcast_queue_changed` **根本不参与**。Pager 的 ACP 连接直接到 Pi adapter。所以 shell 侧不需要任何修改。

Shell-native 会话（非 Pi）仍然使用 shell 的 interject 路径（`SessionCommand::Interject` → `pending_interjections`），这个路径语义正确，不需要改。

### 4.5 多客户端一致性

| 检查项 | 状态 | 说明 |
|--------|------|------|
| Follow-up 队列跨客户端可见 | ⚠️ 降级 | 本地队列是 per-client |
| Steering 跨客户端可见 | ✅ | `broadcast_interjection()` 已处理 |
| 单用户场景 | ✅ | 无影响 |
| Dashboard 模式 | ⚠️ | 其他 pane 看不到 follow-up 队列 |

---

## 5. FIFO 排序修正

### 5.1 问题

如果 follow-up 改为 pager 本地队列，会打破与 Pi 插件消息的 FIFO 排序：

```
改造前（如果 follow-up 走 pager 本地队列）：

用户按 Enter (turn running)
  → pager pending_prompts.push_back("用户消息B")  ← 本地队列

插件调 sendUserMessage("loop prompt", { deliverAs: "followUp" })
  → Pi _queueFollowUp() → _followUpMessages.push("loop prompt")  ← Pi 内部队列

Turn 结束：
  → Pi 先 drain 自己的 followUp → 处理 "loop prompt"  ← 插件先跑！
  → agent_settled
  → pager drain 本地队列 → 发送 "用户消息B"  ← 用户后跑！
```

**排序反了。** 用户先排的消息反而后执行。

### 5.2 修正方案

**Follow-up 不走 pager 本地队列，仍然发到 Pi。只删除 mirror/reconcile。**

```
修正后：

用户按 Enter (turn running)
  → ACP → adapter → Pi RPC { streamingBehavior: "followUp" }
  → Pi _queueFollowUp()  ← 和插件在同一个队列，FIFO 正确

插件调 sendUserMessage("loop prompt", { deliverAs: "followUp" })
  → Pi _queueFollowUp()  ← 同一个队列

Turn 结束：
  → Pi drain _followUpMessages → FIFO: ["用户消息B", "loop prompt"]
  → 按顺序处理 ✅
```

### 5.3 对改造方案的影响

| 原方案 | 修正方案 |
|--------|----------|
| Follow-up → pager 本地队列 | Follow-up → 仍发到 Pi（保持 FIFO） |
| 删除 `Effect::SendPrompt` 的 server-send 路径 | **保留**，但删除乐观回显和 reconcile |
| 队列面板 = 本地 `pending_prompts` | 队列面板 = 发送记录（只写不 reconcile） |
| 插件消息和用户消息分离 | 插件消息和用户消息仍在同一个 Pi 队列 |

### 5.4 队列面板显示策略（修正后）

```rust
// 发送时：记录到 agent.sent_queue_display（纯展示用）
agent.sent_queue_display.push(QueueDisplayEntry { text, sent_at });

// 收到 agent_settled 时：清空
agent.sent_queue_display.clear();

// 可选：收到 queue_update 时：用 text 刷新显示（不做 id reconcile）
// 这样插件消息也能显示出来（只读）
```

---

## 6. 最终改造方案

### 6.1 设计原则

1. **Steering 走 interject**：Send Now → `dispatch_interject()` → `x.ai/interject` → Pi `agent.steer()`（不打断）
2. **Follow-up 仍发到 Pi**：保持与插件消息的 FIFO 排序
3. **删除所有 mirror/reconcile**：不再有乐观回显、id 匹配、text 匹配
4. **队列面板只写不 reconcile**：发送时记录，turn 结束时清空
5. **Pi 插件消息拦截**：通过原型链钩子，让插件的 `sendMessage` / `sendUserMessage` 经过 pager 决策

### 6.2 改造后的架构

```
┌─ Pager ────────────────────────────────────────────────────────┐
│                                                                 │
│  用户按 Send Now (turn running):                                │
│    → dispatch_interject(text)                                   │
│    → scrollback 显示 interjection block                         │
│    → Effect::SendInterject → x.ai/interject                    │
│    → 不经过 shell queue，不经过 mirror                           │
│                                                                 │
│  用户按 Enter (turn running):                                   │
│    → Effect::SendPrompt (保留现有路径)                           │
│    → ACP → adapter → Pi RPC { streamingBehavior: "followUp" }  │
│    → Pi _queueFollowUp() — 和插件共享 FIFO                      │
│    → 队列面板记录（只写，不 reconcile）                          │
│                                                                 │
│  插件消息 (被拦截后):                                           │
│    → deliverAs="steer" → 同 Send Now 路径 (interject)           │
│    → deliverAs="followUp" → 同 Enter 路径 (Pi followUp)        │
│                                                                 │
│  队列面板:                                                      │
│    → 发送时记录 text                                            │
│    → agent_settled 时清空                                       │
│    → 可选：queue_update 只读刷新                                │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
         │                              │
         │ Effect::SendInterject        │ Effect::SendPrompt
         ▼                              ▼
┌─ Pi Adapter ───────────────────────────────────────────────────┐
│  handle_steer_message():                                        │
│    → Pi RPC { streamingBehavior: "steer" }                     │
│    → Pi agent.steer() — 注入，不打断                            │
│                                                                 │
│  prompt():                                                      │
│    → Pi RPC { type: "prompt", streamingBehavior: "followUp" }  │
│    → Pi agent.followUp() — 排队等 turn 结束                     │
│                                                                 │
│  删除: QueueMirror, apply_pi_queue_update, publish_queue_snapshot│
│  保留: handle_steer_message (去掉 reserve 调用)                  │
└─────────────────────────────────────────────────────────────────┘
         │
         ▼
┌─ Pi 内部 ──────────────────────────────────────────────────────┐
│  agent.steer(message) — 注入当前 turn，不打断                   │
│  agent.followUp(message) — 排队等 turn 结束                     │
│  _followUpMessages: string[] — 用户+插件共享 FIFO               │
│  _steeringMessages: string[] — steering 缓冲                    │
│                                                                 │
│  sendCustomMessage() / sendUserMessage() — 被拦截钩子接管:      │
│    if (this._externalQueueHandler) {                            │
│        return this._externalQueueHandler(deliverAs, message);   │
│    }                                                            │
│    // fallback: 原有逻辑                                        │
└─────────────────────────────────────────────────────────────────┘
```

### 6.3 Steering 行为定义（对齐 Pi 语义）

```
Steering (Send Now / Interject):
  ┌─────────────────────────────────────────────────────────┐
  │  AI 正在输出:                                           │
  │  "让我来分析这个问题...首先我们需要考虑..."              │
  │                                                         │
  │  用户按 Send Now: "换个方向，用 Rust 重写"              │
  │                                                         │
  │  → 不取消当前 turn                                      │
  │  → 消息注入当前 turn 上下文                              │
  │  → AI 在下一个自然断点读取:                              │
  │    "好的，我换个方向，用 Rust 来重写..."                 │
  │                                                         │
  │  scrollback 显示:                                       │
  │  ┌─ User (interjection) ─────────────────────┐          │
  │  │ 换个方向，用 Rust 重写                     │          │
  │  └────────────────────────────────────────────┘          │
  └─────────────────────────────────────────────────────────┘

Follow-up (普通 Enter, turn running):
  ┌─────────────────────────────────────────────────────────┐
  │  AI 正在输出长文本...                                    │
  │                                                         │
  │  用户按 Enter: "完成后帮我写个测试"                      │
  │                                                         │
  │  → 消息发到 Pi _queueFollowUp()                         │
  │  → 队列面板显示: [#1: 完成后帮我写个测试]                │
  │  → 当前 turn 正常结束                                    │
  │  → Pi drain _followUpMessages → 新 turn 开始             │
  └─────────────────────────────────────────────────────────┘
```

---

## 7. Pi 插件消息拦截方案

### 7.1 为什么需要拦截

Pi 插件（如 `loop.ts`）通过 `pi.sendUserMessage()` / `pi.sendMessage()` 直接操作 Pi 内部队列：

```typescript
// loop.ts 中的实际代码
pi.sendUserMessage(loopState.prompt, { deliverAs: "followUp" });
```

如果不拦截：
- 插件消息在 Pi 内部处理，pager 完全不知道
- 队列面板无法显示插件排了什么消息
- 用户无法取消/编辑插件排的消息
- 无法实现"pager 是唯一的队列所有者"的设计目标

### 7.2 拦截方案：修改 Pi 原型链

#### 7.2.1 Pi 侧修改（`agent-session.ts`）

```typescript
// agent-session.ts — 新增外部拦截钩子
class AgentSession {
    private _externalQueueHandler?: (
        deliverAs: "steer" | "followUp",
        message: {
            text: string;
            images?: ImageContent[];
            customType?: string;
            content?: unknown[];
            display?: unknown;
            details?: unknown;
        },
        options?: { triggerTurn?: boolean }
    ) => Promise<boolean>;  // true = 已拦截，不走 Pi 内部

    /**
     * 由 adapter 在 bootstrap 时设置。
     * 设置后，所有 streaming 期间的 sendMessage/sendUserMessage
     * 都先经过此钩子，由 pager 决定如何处理。
     */
    setExternalQueueHandler(handler: typeof this._externalQueueHandler) {
        this._externalQueueHandler = handler;
    }

    async sendCustomMessage<T = unknown>(message, options) {
        const appMessage = { /* ... 同现有 ... */ };

        if (options?.deliverAs === "nextTurn") {
            this._pendingNextTurnMessages.push(appMessage);
            return;
        }

        if (this.isStreaming) {
            // ★ 新增：外部拦截
            if (this._externalQueueHandler) {
                const messageText = contentText(appMessage.content, "");
                const intercepted = await this._externalQueueHandler(
                    options?.deliverAs ?? "steer",
                    {
                        text: messageText,
                        customType: message.customType,
                        content: message.content,
                        display: message.display,
                        details: message.details,
                    },
                    options
                );
                if (intercepted) return;  // pager 接管
            }

            // Fallback：原有逻辑（兼容无 adapter 场景）
            const messageText = contentText(appMessage.content, "");
            if (options?.deliverAs === "followUp") {
                if (messageText) {
                    this._followUpMessages.push(messageText);
                    this._emitQueueUpdate();
                }
                this.agent.followUp(appMessage);
            } else {
                if (messageText) {
                    this._steeringMessages.push(messageText);
                    this._emitQueueUpdate();
                }
                this.agent.steer(appMessage);
            }
            return;
        }

        // idle 时的逻辑不变...
        if (options?.triggerTurn) {
            await this._runAgentPrompt(appMessage);
        } else {
            this.agent.state.messages.push(appMessage);
            // ... persist ...
        }
    }

    // sendUserMessage 同样需要拦截
    async sendUserMessage(content, options) {
        // ... normalize content to text + images ...

        if (this.isStreaming && this._externalQueueHandler) {
            const intercepted = await this._externalQueueHandler(
                options?.deliverAs ?? "steer",
                { text, images },
                undefined
            );
            if (intercepted) return;
        }

        // fallback: 原有逻辑
        await this.prompt(text, {
            streamingBehavior: options?.deliverAs ?? "steer",
            images,
            expandPromptTemplates: false,
        });
    }
}
```

#### 7.2.2 Adapter 侧修改（`pi_adapter.rs`）

Adapter 在 bootstrap 时通过 Pi RPC 设置拦截钩子。Pi 需要暴露一个 extension command 或 RPC 方法：

```rust
// pi_adapter.rs — bootstrap 时注入拦截逻辑
// 方案 A：通过 Pi 的 extension command 机制
// 方案 B：通过 Pi RPC 的 set_external_queue_handler 方法
// 方案 C：adapter 在 Pi 进程中注入一个内置 extension（推荐）

// 拦截后的处理：
async fn handle_plugin_queue_request(&self, params: Value) {
    let deliver_as = params["deliverAs"].as_str().unwrap_or("steer");
    let text = params["text"].as_str().unwrap_or_default();

    match deliver_as {
        "steer" => {
            // 转发给 pager 作为 interject
            self.send_ext_notification("x.ai/plugin_interject", json!({
                "sessionId": self.session_id().0,
                "text": text,
                "source": "plugin",
            })).await;
        }
        "followUp" => {
            // 仍然发到 Pi 的 followUp 队列（保持 FIFO）
            // 但通知 pager 显示
            self.send_ext_notification("x.ai/plugin_followup_queued", json!({
                "sessionId": self.session_id().0,
                "text": text,
                "source": "plugin",
            })).await;
            // 实际执行仍走 Pi 内部（保持 FIFO）
            // 拦截钩子返回 false，让 Pi 继续处理
        }
        _ => {}
    }
}
```

#### 7.2.3 Pager 侧修改

```rust
// 新增 ACP ext notification handler:

// "x.ai/plugin_interject" — 插件的 steer 消息
// → 同 dispatch_interject() 路径
// → scrollback 显示 "[Plugin] message"
// → Effect::SendInterject

// "x.ai/plugin_followup_queued" — 插件的 followUp 消息（只读通知）
// → 队列面板显示 "[Plugin] message"（只读行，不可编辑/删除）
// → 实际执行由 Pi 内部完成
```

#### 7.2.4 拦截后的 FIFO 保证

```
插件调 sendUserMessage("loop prompt", { deliverAs: "followUp" })
│
├─ _externalQueueHandler 被调用
│   → adapter 收到通知
│   → adapter 通知 pager 显示 "[Plugin] loop prompt"
│   → 钩子返回 false（不拦截执行，只拦截显示）
│
├─ Pi 继续原有逻辑
│   → _queueFollowUp("loop prompt")
│   → 和用户消息在同一个 FIFO 队列
│
└─ 排序正确 ✅
```

**关键设计决策**：对于 `deliverAs: "followUp"` 的插件消息，拦截钩子**不阻止执行**（返回 false），只负责**通知 pager 显示**。这样 FIFO 排序由 Pi 内部保证，pager 只做展示。

对于 `deliverAs: "steer"` 的插件消息，拦截钩子**阻止执行**（返回 true），改由 pager 通过 `x.ai/interject` 路径重新发送。这样 pager 可以在 scrollback 显示 interjection block。

### 7.3 兼容性保证

- `_externalQueueHandler` 默认为 `None`，只有 adapter 设置后才生效
- Fallback 路径完全保留原有逻辑
- 插件无需任何修改
- 不经过 adapter 的 Pi 直接使用场景不受影响

---

## 8. 删除/保留清单

### 8.1 删除清单

| 文件 | 删除内容 | 原因 |
|------|----------|------|
| `pi-grok-adapter/src/queue_bridge.rs` | 整个文件 (~280 行) | QueueMirror 不再需要 |
| `pi-grok-adapter/src/pi_adapter.rs` | `apply_pi_queue_update()`, `publish_queue_snapshot()`, `rebroadcast_queue_mirror()` | 不再 mirror Pi 队列 |
| `pi-grok-adapter/src/pi_adapter.rs` | `queue_mirror` 字段及所有 `reserve()` 调用 | 同上 |
| `xai-grok-pager/src/app/effects/mod.rs` | `Effect::SendPromptNow` 变体及其处理 (~40 行) | Send Now 改走 interject |
| `xai-grok-pager/src/app/dispatch/interject.rs` | `dispatch_send_prompt_now()` 整个函数 (~80 行) | 被 `dispatch_interject()` 替代 |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `push_server_queue_echo()` (~20 行) | 不再有乐观回显 |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `retire_optimistic_echo()` (~30 行) | 同上 |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `immediate_server_send_eligible()` 中的 server-busy 判断 | 不再判断 server-busy |
| `xai-grok-pager/src/app/app_view.rs` | `optimistic_prompt_echoes` 字段 | 不再有 mirror |
| `xai-grok-pager/src/app/app_view.rs` | `shared_prompt_queues` 字段 | 同上 |
| `xai-grok-pager/src/app/app_view.rs` | `apply_queue_changed()` (~60 行) | 不再 reconcile |
| `xai-grok-pager/src/app/app_view.rs` | `push_optimistic_prompt_echo()` (~30 行) | 不再有乐观回显 |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `send_now_awaiting_confirm` 字段 | 不再有 park 机制 |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `resolve_send_now_awaiting_confirm()` (~30 行) | 同上 |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `optimistic_queue_ids` 字段 | 同上 |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `force_interject_queue_row()` 中的 server row 分支 (~20 行) | server row 不再存在 |
| `xai-grok-pager/src/views/queue_pane.rs` | `visible_held_server_row()` (~10 行) | 不再有 server 行 |
| `xai-grok-pager/src/views/queue_pane.rs` | `QueueRowOrigin::Server` 变体及相关逻辑 | 同上 |
| `xai-grok-pager/src/app/dispatch/task_result.rs` | `TaskResult::SendPromptNowFailed` 处理 (~30 行) | 不再有 send-now 失败 |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `maybe_drain_queue()` 中的 `shared_queue` 检查 (~5 行) | 不再有 shared_queue |

**总计删除：~700 行**

### 8.2 保留清单

| 文件 | 保留内容 | 原因 |
|------|----------|------|
| `xai-grok-pager/src/app/agent/session.rs` | `pending_prompts`, `enqueue_prompt()`, `dequeue_prompt()` | 本地队列（idle 时使用） |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `maybe_drain_queue()` | idle 时自动 drain |
| `xai-grok-pager/src/app/dispatch/interject.rs` | `dispatch_interject()` | steering 路径（核心） |
| `xai-grok-pager/src/app/dispatch/interject.rs` | `record_interject_prompt_history()` | Ctrl+R 历史 |
| `xai-grok-pager/src/app/dispatch/queue.rs` | `push_send_now_user_block()` | scrollback 渲染 |
| `xai-grok-pager/src/app/effects/mod.rs` | `Effect::SendInterject` 及其处理 | steering 发送 |
| `xai-grok-pager/src/app/effects/mod.rs` | `Effect::SendPrompt` 及其处理 | follow-up 发送 |
| `pi-grok-adapter/src/pi_adapter.rs` | `handle_steer_message()` (去掉 reserve) | Pi steer RPC |
| `pi-grok-adapter/src/prompt_bridge.rs` | `prompt_streaming_behavior()` | 判断 steer/followUp |
| `xai-grok-shell/src/session/acp_session_impl/interjection.rs` | 全部 | Shell-native interject |
| `xai-grok-shell/src/extensions/interject.rs` | 全部 | Shell ext handler |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `sync_queue_pane()` (简化版) | 队列面板渲染 |
| `xai-grok-pager/src/app/agent_view/queue.rs` | `try_send_now_queued_from_prompt()` | 空 composer 时发送队列行 |
| `pi-main/.../core/agent-session.ts` | `_queueSteer()`, `_queueFollowUp()` | Pi 内部执行 |
| `pi-main/.../core/agent-session.ts` | `_steeringMessages`, `_followUpMessages` | Pi 内部 FIFO |

### 8.3 新增清单

| 文件 | 新增内容 | 原因 |
|------|----------|------|
| `pi-main/.../core/agent-session.ts` | `_externalQueueHandler` 字段 + `setExternalQueueHandler()` | 插件拦截钩子 |
| `pi-main/.../core/agent-session.ts` | `sendCustomMessage` / `sendUserMessage` 中的拦截逻辑 | 拦截插件消息 |
| `pi-grok-adapter/src/pi_adapter.rs` | 拦截钩子注入 + `x.ai/plugin_interject` 通知 | 转发插件消息给 pager |
| `xai-grok-pager/src/app/acp_handler/` | `x.ai/plugin_interject` handler | 处理插件 steer |
| `xai-grok-pager/src/app/acp_handler/` | `x.ai/plugin_followup_queued` handler | 显示插件 followUp |

---

## 9. 迁移策略

### Phase 1: Pager 路由修改（最小 MVP，5 行改动）

```rust
// router.rs — 将 SendPromptNow 路由到 interject
Action::SendPromptNow { text, images } => {
    // 改前: super::interject::dispatch_send_prompt_now(app, text, images)
    // 改后:
    super::interject::dispatch_interject(app, text, images)
}
```

**效果**：Send Now 立即变为 steering 语义（不打断），连按不再产生重复。

### Phase 2: 删除乐观回显 + reconcile（~200 行删除）

- 删除 `push_server_queue_echo()` 调用
- 删除 `optimistic_prompt_echoes` / `shared_prompt_queues`
- 删除 `apply_queue_changed()` 的 id 匹配逻辑
- 删除 `send_now_awaiting_confirm` / `resolve_send_now_awaiting_confirm`
- 简化 `sync_queue_pane()`：只渲染本地队列 + 发送记录

### Phase 3: 删除 adapter QueueMirror（~180 行删除）

- 删除 `queue_bridge.rs` 整个文件
- 删除 `pi_adapter.rs` 中 `queue_mirror` 字段及所有 `reserve()` / `apply_queue_update()` / `publish_queue_snapshot()` 调用
- 保留 `handle_steer_message()`（去掉 `reserve` 调用）
- 可选：`queue_update` 事件改为只读通知 pager（不做 reconcile）

### Phase 4: Pi 插件拦截（~100 行新增）

- `agent-session.ts`：新增 `_externalQueueHandler` + 拦截逻辑
- `pi_adapter.rs`：bootstrap 时注入钩子 + 转发通知
- Pager：新增 `x.ai/plugin_interject` / `x.ai/plugin_followup_queued` handler

### Phase 5: 清理（~100 行删除）

- 删除 `Effect::SendPromptNow` 变体
- 删除 `dispatch_send_prompt_now()` 函数
- 删除 `TaskResult::SendPromptNowFailed`
- 删除 `immediate_server_send_eligible()` 中的 server-busy 判断
- 删除 `visible_held_server_row()` / `QueueRowOrigin::Server`

---

## 10. 测试矩阵

### 10.1 基本功能

| 场景 | 期望行为 | 验证点 |
|------|----------|--------|
| Idle 时按 Enter | 立即发送，新 turn 开始 | `maybe_drain_queue` 立即 drain |
| Turn running 时按 Enter | 消息发到 Pi followUp 队列 | 队列面板显示，turn 结束后 Pi 自动处理 |
| Turn running 时按 Send Now | 消息注入当前 turn（steering） | 不打断 AI，scrollback 显示 interjection |
| 快速连按 Send Now (×5) | 5 条 interjection 注入 | 无重复，无队列追加 |
| Turn 结束后 followUp 自动处理 | 按 FIFO 顺序逐条处理 | 用户消息和插件消息排序正确 |
| 空 composer + 队列有行 + 按 Send Now | 发送队列第一行（interject） | 保持现有 UX |

### 10.2 插件消息

| 场景 | 期望行为 | 验证点 |
|------|----------|--------|
| 插件 `sendUserMessage({ deliverAs: "followUp" })` | Pi 内部处理 + pager 显示 | FIFO 排序正确，队列面板有只读行 |
| 插件 `sendMessage({ deliverAs: "steer" })` | 拦截 → interject 路径 | scrollback 显示 "[Plugin]" interjection |
| 插件在 idle 时 `sendMessage({ triggerTurn: true })` | 直接触发新 turn | 不经过队列 |
| 无 adapter 时插件 `sendMessage` | Fallback 到 Pi 内部 | 向后兼容 |
| `loop.ts` 的 `updateStatus` | 不进入队列 | 状态更新 ≠ 消息 |
| `loop.ts` 的 `sendUserMessage(prompt, followUp)` | Pi followUp + pager 显示 | FIFO 正确 |

### 10.3 边界条件

| 场景 | 期望行为 | 验证点 |
|------|----------|--------|
| Reconnect 期间按 Send Now | 本地 requeue | toast "Reconnecting" |
| 无 session 时按 Send Now | toast "No active session" | 不 crash |
| Goal turn 期间按 Send Now | steering 注入（不 cancel） | goal 不被打断 |
| 多 pane 查看同一 session | 所有 pane 显示 interjection | `broadcast_interjection` |
| 队列编辑（e 键） | 编辑本地队列行 | 不再有 server row 编辑 |
| 模型切换 | 队列 drain 暂停 | `model_switch_pending` 保留 |
| Ctrl+C rewind | 正常回退 | `in_flight_prompt` 机制保留 |

### 10.4 回归测试

| 场景 | 期望行为 | 验证点 |
|------|----------|--------|
| Subagent 完成后的 auto-wake | 正常触发新 turn | synthetic prompt 路径不变 |
| Shell-native 会话的 interject | 正常工作 | `SessionCommand::Interject` 路径不变 |
| Shell-native 会话的 follow-up | 正常工作 | shell `pending_inputs` 路径不变 |
| `/loop` 插件完整生命周期 | 正常 | `sendUserMessage` 被拦截但 FIFO 正确 |
| `pi-grok-subagents` 插件 | 正常 | `sendMessage` 被拦截 |
| `pi-grok-recap` 插件 | 正常 | `sendMessage` 被拦截 |

---

## 11. 风险评估

### 11.1 多客户端一致性

**现状**：Shell 的 `pending_inputs` 是 server-authoritative，多客户端通过 `x.ai/queue/changed` 同步。

**改造后**：
- Steering（interject）通过 `broadcast_interjection` 仍然多 pane 同步 ✅
- Follow-up 队列是 per-client 的（每个客户端有自己的输入框）⚠️

**严重度**：⭐ 极低
- 单用户场景（绝大多数）：无影响
- 多客户端：follow-up 是 per-client 的，这是合理的
- 如果需要跨客户端可见性，可以后续加轻量级 "queue hint" 通知

### 11.2 Pi 插件兼容性

**风险**：修改 `sendCustomMessage` / `sendUserMessage` 可能影响不经过 adapter 的 Pi 直接使用场景。

**严重度**：⭐ 极低
- `_externalQueueHandler` 默认为 `None`，只有 adapter 设置后才生效
- Fallback 路径完全保留原有逻辑
- 插件无需任何修改

### 11.3 FIFO 排序

**风险**：如果拦截钩子对 followUp 消息返回 true（阻止执行），会打破 FIFO。

**严重度**：⭐⭐ 中等（设计时需注意）
- **设计决策**：对 `deliverAs: "followUp"` 的插件消息，拦截钩子**不阻止执行**（返回 false），只负责通知 pager 显示
- 对 `deliverAs: "steer"` 的插件消息，拦截钩子**阻止执行**（返回 true），改由 pager 通过 interject 路径重新发送

### 11.4 Shell-native 会话

**风险**：改造是否影响 shell-native 会话？

**严重度**：⭐ 零
- Shell-native 会话的 interject 路径（`SessionCommand::Interject` → `pending_interjections`）完全不变
- Shell-native 会话的 follow-up 路径（`pending_inputs`）完全不变
- Shell 源码不需要任何修改

---

## 12. 工作量估算

| Phase | 内容 | 估算 |
|-------|------|------|
| Phase 1 | Pager 路由修改（5 行） | 0.5 天 |
| Phase 2 | 删除乐观回显 + reconcile（~200 行删除） | 1-2 天 |
| Phase 3 | 删除 adapter QueueMirror（~180 行删除） | 1 天 |
| Phase 4 | Pi 插件拦截（~100 行新增） | 1-2 天 |
| Phase 5 | 清理残留代码（~100 行删除） | 0.5 天 |
| 测试 | 全量回归 + 新增测试 | 2-3 天 |
| **总计** | | **6-9 天** |

---

## 13. 附录：关键代码位置索引

### Pager (`xai-grok-pager`)

| 概念 | 文件 | 行号 |
|------|------|------|
| Send Now 入口 (InterjectPrompt) | `src/app/agent_view/prompt.rs` | L621 |
| dispatch_send_prompt_now (删除) | `src/app/dispatch/interject.rs` | L88 |
| dispatch_interject (保留，核心) | `src/app/dispatch/interject.rs` | L18 |
| record_interject_prompt_history | `src/app/dispatch/interject.rs` | L165 |
| Effect::SendPromptNow 处理 (删除) | `src/app/effects/mod.rs` | L1621 |
| Effect::SendInterject 处理 (保留) | `src/app/effects/mod.rs` | L4045 |
| push_server_queue_echo (删除) | `src/app/dispatch/queue.rs` | L72 |
| retire_optimistic_echo (删除) | `src/app/dispatch/queue.rs` | L108 |
| immediate_server_send_eligible | `src/app/dispatch/queue.rs` | L50 |
| maybe_drain_queue (保留) | `src/app/dispatch/queue.rs` | L200 |
| apply_queue_changed (删除) | `src/app/app_view.rs` | L2343 |
| push_optimistic_prompt_echo (删除) | `src/app/app_view.rs` | L2403 |
| sync_queue_pane (简化) | `src/app/agent_view/queue.rs` | L364 |
| force_interject_queue_row (简化) | `src/app/agent_view/queue.rs` | L456 |
| try_send_now_queued_from_prompt (保留) | `src/app/agent_view/queue.rs` | L49 |
| resolve_send_now_awaiting_confirm (删除) | `src/app/agent_view/queue.rs` | L520 |
| visible_held_server_row (删除) | `src/views/queue_pane.rs` | L23 |
| sync_from_merged (简化) | `src/views/queue_pane.rs` | L475 |
| router SendPromptNow 路由 (修改) | `src/app/dispatch/router.rs` | L359 |
| TaskResult::SendPromptNowFailed (删除) | `src/app/dispatch/task_result.rs` | L478 |

### Pi Adapter (`pi-grok-adapter`)

| 概念 | 文件 | 行号 |
|------|------|------|
| QueueMirror (整个删除) | `src/queue_bridge.rs` | L1-280 |
| apply_pi_queue_update (删除) | `src/pi_adapter.rs` | L1120 |
| publish_queue_snapshot (删除) | `src/pi_adapter.rs` | L1110 |
| rebroadcast_queue_mirror (删除) | `src/pi_adapter.rs` | L1131 |
| queue_mirror 字段 (删除) | `src/pi_adapter.rs` | L192 |
| handle_steer_message (保留，去掉 reserve) | `src/pi_adapter.rs` | L2984 |
| prompt() 中的 reserve 调用 (删除) | `src/pi_adapter.rs` | L2390 |
| prompt_streaming_behavior (保留) | `src/prompt_bridge.rs` | L35 |
| handle_event "queue_update" (删除/改为只读) | `src/pi_adapter.rs` | L1397 |
| ext_method "x.ai/interject" (保留) | `src/pi_adapter.rs` | L2556 |

### Shell (`xai-grok-shell`)

| 概念 | 文件 | 行号 |
|------|------|------|
| extensions/interject.rs (保留，不改) | `src/extensions/interject.rs` | L1-123 |
| SessionCommand::Interject 处理 (保留) | `src/session/acp_session_impl/run_loop.rs` | L862 |
| pending_interjections (保留) | `src/session/acp_session.rs` | L686 |
| drain_pending_interjections (保留) | `src/session/acp_session_impl/interjection.rs` | L288 |
| broadcast_interjection (保留) | `src/session/acp_session_impl/interjection.rs` | L160 |
| queue_input (保留，不改) | `src/session/acp_session_impl/prompt_queue.rs` | L10 |
| broadcast_queue_changed (保留，不改) | `src/session/acp_session_impl/prompt_queue.rs` | L270 |
| handle_interject_queued_prompt (保留) | `src/session/acp_session_impl/prompt_queue.rs` | L546 |

### Pi (`pi-main/packages/coding-agent`)

| 概念 | 文件 | 行号 |
|------|------|------|
| sendCustomMessage (修改：加拦截) | `src/core/agent-session.ts` | L1410 |
| sendUserMessage (修改：加拦截) | `src/core/agent-session.ts` | L1464 |
| prompt() streaming 分支 | `src/core/agent-session.ts` | L1139 |
| _queueSteer | `src/core/agent-session.ts` | L1359 |
| _queueFollowUp | `src/core/agent-session.ts` | L1376 |
| steer() 公开方法 | `src/core/agent-session.ts` | L1316 |
| followUp() 公开方法 | `src/core/agent-session.ts` | L1336 |
| _emitQueueUpdate | `src/core/agent-session.ts` | L580 |
| _handleAgentEvent (queue dequeue) | `src/core/agent-session.ts` | L578 |
| sendMessage binding (runner) | `src/core/agent-session.ts` | L2343 |
| sendUserMessage binding (runner) | `src/core/agent-session.ts` | L2352 |

### Pi 插件示例

| 插件 | 文件 | 调用 |
|------|------|------|
| loop.ts | `~/.pi/agent/extensions/loop.ts` | `pi.sendUserMessage(loopState.prompt, { deliverAs: "followUp" })` |
| pi-grok-bash | `extensions/pi-grok-bash/index.ts:185` | `pi.sendMessage(...)` |
| pi-grok-recap | `extensions/pi-grok-recap/index.ts:243` | `pi.sendMessage(...)` |
| pi-grok-subagents | `extensions/pi-grok-subagents/index.ts:237` | `pi.sendMessage(...)` |

---

## 14. 模拟器验证

已构建交互式模拟器（`~/.pi/widgets/2026-07-22T02-34-47_queue_redesign_simulator.html`），对比 Current vs Redesigned 行为：

| 操作 | Current (broken) | Redesigned (fixed) |
|------|-----------------|-------------------|
| Send Now | cancel turn + 双重广播 + duplicate echo | `agent.steer()` 注入，不取消，无重复 |
| Send Now x5 | 队列无限增长，5 条 duplicate | 5 条独立 interjection，队列不增长 |
| Plugin msg | 混入 mirror，产生幽灵行 | 拦截 → 通知 pager 显示，FIFO 正确 |
| Enter (follow-up) | 走 shell + mirror reconcile | 直接到 Pi `_queueFollowUp`，FIFO 正确 |

---

## 15. 设计决策记录

| # | 决策 | 选项 | 最终选择 | 理由 |
|---|------|------|----------|------|
| 1 | Send Now 的 scrollback 渲染 | A: `interjection_prompt` 样式 / B: `user_prompt` 样式 | A | 区分于普通消息，用户知道这是 steering |
| 2 | 空 composer + 队列有行时按 Send Now | A: 发送队列第一行（interject） / B: no-op | A | 保持现有 UX |
| 3 | Pi `queue_update` 事件 | A: 忽略 / B: 只读刷新面板 | B | 插件消息可显示（只读） |
| 4 | Shell-native 会话是否也改 | A: 一起改 / B: 只改 Pi 路径 | B | Shell 路径语义正确，不需要改 |
| 5 | Follow-up 走哪里 | A: pager 本地 / B: 仍发到 Pi | B | 保持与插件消息的 FIFO 排序 |
| 6 | 插件 followUp 拦截后是否阻止执行 | A: 阻止 / B: 只通知不阻止 | B | 保持 FIFO，Pi 内部执行 |
| 7 | 插件 steer 拦截后是否阻止执行 | A: 阻止 / B: 只通知不阻止 | A | 改由 pager interject 路径发送，可在 scrollback 显示 |
| 8 | 是否修改 Pi 源码 | A: 是 / B: 否 | A | 拦截插件消息需要修改 `agent-session.ts` |
| 9 | 是否修改 Shell 源码 | A: 是 / B: 否 | B | Pi 会话不经过 shell，shell-native 路径正确 |
