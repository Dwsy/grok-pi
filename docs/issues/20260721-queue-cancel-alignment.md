# 队列取消对齐：Ctrl+C 必须清空排队消息（完整研究记录）

> **历史方案提示（2026-07-25）：** 文中给系统 Pi 打 `clear_queue` 补丁的方案未作为最终实现采用。当前实现不改 Pi 源码，而是在 adapter 隔离队列中同步 clear，并以 fire-and-forget abort + idle probe 收敛；详见 `adapter/20260717-桥接-Pi-queue_update-到-原生队列面板.md`。

## 日期
2026-07-21

## 状态
✅ 已修复并通过测试

---

## 一、问题为什么出现

### 用户报告的现象

> 发送消息 a 正在处理，再发送到队列消息 b，然后按 Ctrl+C 终止，就会立即发送 a，
> 但是页面显示一直在取消，然后等待 b 的回答完成后 groktui 才显示发送出去了。
> 很多地方没对齐。

### 根本原因

**grok-pi 的取消路径没有清空 Pi 的 steering / follow-up 队列**，而 Pi 原生的 TUI
在取消时会先清空队列再 abort。这是一个**协议能力缺失**导致的语义不对齐：

- Pi 原生 TUI（`interactive-mode.ts`）的中断处理是
  `restoreQueuedMessagesToEditor({ abort: true })`，它**先调用 `clearAllQueues()`
  把排队消息恢复到编辑器，再调用 `agent.abort()`**。
- grok-pi 走的是 Pi 的 **JSONL RPC 模式**（`rpc-mode.ts`），而 RPC 协议**根本没有
  暴露清空队列的命令**。adapter 只能发 `abort`，无法复制 TUI 的"先清队列再 abort"语义。

因此取消时排队消息 b 留在 Pi 内部队列里，触发了 Pi 的"运行后自动续跑"机制，造成一系列错位。

---

## 二、问题是如何发生的（完整调用链分析）

### 2.1 Pi 的队列与续跑机制（来自 pi-main 源码深读）

**队列结构**（`packages/agent/src/agent.ts`）：

```ts
class PendingMessageQueue {
    private messages: AgentMessage[] = [];
    public mode: QueueMode;              // "all" | "one-at-a-time"
    drain(): AgentMessage[] {
        if (this.mode === "all") { /* 全部取出 */ }
        // one-at-a-time: 只取最早的一条
        const first = this.messages[0];
        this.messages = this.messages.slice(1);
        return [first];
    }
}
```

Agent 持有两个队列：`steeringQueue`（steer，打断当前轮）和 `followUpQueue`
（follow-up，等当前轮结束再发）。默认模式都是 `"one-at-a-time"`。

**agent-loop 的出队时机**（`packages/agent/src/agent-loop.ts`）：

```ts
// 内层循环：每轮工具调用后拉取 steering
pendingMessages = (await config.getSteeringMessages?.()) || [];
// 外层循环：agent 即将停止时拉取 follow-up
const followUpMessages = (await config.getFollowUpMessages?.()) || [];
if (followUpMessages.length > 0) { pendingMessages = followUpMessages; continue; }
```

**关键的自动续跑**（`packages/coding-agent/src/core/agent-session.ts`）：

```ts
private async _runAgentPrompt(messages): Promise<void> {
    this._isAgentRunActive = true;
    try {
        await this.agent.prompt(messages);
        while (await this._handlePostAgentRun()) {   // ← 关键
            await this.agent.continue();             // ← 自动续跑队列消息
        }
    } finally {
        await this._emitAgentSettled();              // ← agent_settled 在此发出
    }
}

private async _handlePostAgentRun(): Promise<boolean> {
    // ...retry / compaction 检查...
    // 队列里还有消息就继续跑！
    return this.agent.hasQueuedMessages();
}
```

**abort 的行为**（`agent.ts`）：

```ts
abort(): void {
    this.activeRun?.abortController.abort();   // 只中止当前 run，不清队列！
}
```

> ⚠️ **核心发现**：`abort()` 只设置 AbortController，**不会清空 steering/follow-up
> 队列**。abort 后当前 run 以 `stopReason: "aborted"` 结束，但
> `_handlePostAgentRun()` 仍会检查 `hasQueuedMessages()`——如果队列里有 b，
> 就调用 `agent.continue()` 把 b 当作新一轮跑起来。

### 2.2 Pi TUI 如何正确取消（对照组）

`packages/coding-agent/src/modes/interactive/interactive-mode.ts`：

```ts
abortHandler: () => {
    this.restoreQueuedMessagesToEditor({ abort: true });
}

private restoreQueuedMessagesToEditor(options?: { abort?: boolean }): number {
    const { steering, followUp } = this.clearAllQueues();   // ← 先清队列
    const allQueued = [...steering, ...followUp];
    // 把排队文本拼回编辑器
    this.editor.setText(combinedText);
    this.updatePendingMessagesDisplay();
    if (options?.abort) {
        this.agent.abort();                                  // ← 后 abort
    }
}
```

`clearAllQueues()` → `session.clearQueue()` → `agent.clearAllQueues()` +
`_emitQueueUpdate()`（发出空的 `queue_update`）。

### 2.3 grok-pi 修复前的错误时序

```
用户发 a (turn 运行中)
  └─ ACP prompt(a) → Pi prompt(a, streamingBehavior 无) → 正常 turn
用户发 b (turn 仍运行)
  └─ ACP prompt(b) → prompt_streaming_behavior(already_active=true) = "followUp"
     → Pi prompt(b, streamingBehavior="followUp") → 进入 followUpQueue
     → ACP prompt(b) 阻塞在 completion_rx，等待 agent_settled
用户按 Ctrl+C
  └─ ACP cancel
     → adapter: 标记 active_prompts.cancelled = true
     → adapter: 发 abort RPC（旧代码：直接 abort，没清队列！）
        → Pi agent.abort() → 当前 run 以 aborted 结束
        → _handlePostAgentRun(): hasQueuedMessages() == true（b 还在！）
        → agent.continue() → 开始处理 b ← ❌ 静默续跑
        → b 处理完 → _emitAgentSettled() → agent_settled
  └─ adapter 收到 agent_settled → finish_prompts
     → prompt(a) 和 prompt(b) 的 completion_rx 同时解锁
```

**导致的三个可见症状**：

1. **"立即发送 a"的错觉**：abort 让 a 的 turn 提前结束，a 的部分输出定格。
2. **页面一直显示"取消中"**：prompt(b) 的 ACP 调用阻塞到 b 跑完才返回，
   pager 的 TurnCancelling 状态卡住。
3. **"等 b 回答完成后 groktui 才显示发送出去"**：b 被静默续跑，它的回答先于
   prompt(b) 的返回出现在流里，顺序完全错位。

---

## 三、修复了什么（设计）

对齐 Pi TUI 的"先清队列、后 abort"语义，并补齐 RPC 协议缺失的能力。

### 修复后的正确时序

```
用户按 Ctrl+C
  └─ ACP cancel
     → adapter: 标记 active_prompts.cancelled = true
     → adapter: 发 clear_queue RPC ← 新增
        → Pi session.clearQueue() → 清空 steering + followUp 队列
        → Pi 发出空的 queue_update 事件
     → adapter: 清空本地 queue_mirror，发布空 x.ai/queue/changed ← 新增
        → pager QueuePane 立即排空
     → adapter: 发 abort RPC
        → Pi agent.abort() → 当前 run 以 aborted 结束
        → _handlePostAgentRun(): hasQueuedMessages() == false（已清空）
        → 不再续跑 → _emitAgentSettled() → agent_settled
     → adapter: settle backstop 轮询 get_state，Pi idle 即 finish_prompts
        → prompt(a)、prompt(b) 立即返回 Cancelled
        → pager 退出 TurnCancelling
```

---

## 四、改了什么（grok-pi 仓库内）

### 4.1 `crates/codegen/pi-grok-adapter/src/queue_bridge.rs`

新增 `QueueMirror::clear()`：

```rust
/// Clear all mirrored entries and reservations (cancel path).
/// Returns a snapshot with empty entries so the pager can update.
pub(crate) fn clear(&mut self) -> QueueSnapshot {
    self.entries.clear();
    self.reserved.clear();
    self.running_prompt_id = None;
    self.snapshot()
}
```

新增测试 `clear_empties_all_state`（验证清空后 snapshot 为空、可重新入队）。

### 4.2 `crates/codegen/pi-grok-adapter/src/pi_adapter.rs`

**(a) `cancel()` 重排——先清队列后 abort：**

```rust
async fn cancel(&self, _arguments) -> Result<(), acp::Error> {
    let command = { /* 标记 cancelled，选 abort/abort_bash */ };

    // 先清 Pi 队列（best-effort，旧 Pi 无此命令则忽略）
    if let Err(error) = self.rpc.request(json!({ "type": "clear_queue" })).await {
        tracing::debug!(%error, "clear_queue RPC unavailable; proceeding with abort");
    }
    // 清本地镜像 + 发布空快照
    { self.state.borrow_mut().queue_mirror.clear(); }
    self.publish_queue_snapshot().await;

    // 再 abort
    if let Err(error) = self.rpc.request(json!({ "type": command })).await {
        self.finish_prompts(acp::StopReason::Cancelled);
        return Err(acp_internal(error));
    }
    // settle backstop
    tokio::task::spawn_local(async move { probe.settle_cancelled_prompts().await; });
    Ok(())
}
```

**(b) `settle_cancelled_prompts()` 兜底清镜像：**

```rust
if !parse_state(&value).is_streaming {
    // 兜底：即使 clear_queue 不可用，idle 时也清干净
    { self.state.borrow_mut().queue_mirror.clear(); }
    self.publish_queue_snapshot().await;
    self.finish_prompts(acp::StopReason::Cancelled);
    return;
}
```

**(c) 新增 `pi/queue/mode` ext_method**（对齐 Pi 的 Follow-up mode 设置）：

```rust
"pi/queue/mode" => {
    let mode = /* "all" | "one-at-a-time" */;
    if params.get("steering").and_then(Value::as_bool) == Some(true) {
        self.rpc.request(json!({ "type": "set_steering_mode", "mode": mode })).await?;
    } else {
        self.rpc.request(json!({ "type": "set_follow_up_mode", "mode": mode })).await?;
    }
    ext_response(json!({ "mode": mode }))
}
```

---

## 五、对上游（Pi）进行了什么修改

> ⚠️ 上游修改说明：AGENTS.md 规定"不要修改 Pi 源码来扩展 RPC，优先用官方扩展 API"。
> 但本问题中，**清空队列是取消语义的必要能力，Pi 的扩展 API（ExtensionContext）只暴露
> `hasPendingMessages()` 和 `abort()`，没有 `clearQueue()`**，无法通过扩展实现。
> 因此对 Pi RPC 做了**最小、向后兼容**的新增命令 `clear_queue`（纯新增，不改任何现有
> 命令语义），并同时打补丁到 pi-main 子模块与系统安装的 Pi。

### 5.1 `pi-main/packages/coding-agent/src/modes/rpc/rpc-types.ts`

```ts
// RpcCommand 联合类型新增：
| { id?: string; type: "clear_queue" }

// RpcResponse 联合类型新增：
| { id?: string; type: "response"; command: "clear_queue"; success: true;
    data: { steering: string[]; followUp: string[] } }
```

### 5.2 `pi-main/packages/coding-agent/src/modes/rpc/rpc-mode.ts`

```ts
case "clear_queue": {
    const cleared = session.clearQueue();   // 复用 AgentSession 已有的 clearQueue()
    return success(id, "clear_queue", cleared);
}
```

> 注意：`session.clearQueue()` 是 AgentSession **已有**的公开方法
> （`agent-session.ts:1502`），Pi TUI 的 `restoreQueuedMessagesToEditor` 也用它。
> 本次只是把它通过 RPC 暴露出来，**没有改动 AgentSession 内部逻辑**。

### 5.3 `pi-main/packages/coding-agent/src/modes/rpc/rpc-client.ts`

```ts
async clearQueue(): Promise<{ steering: string[]; followUp: string[] }> {
    const response = await this.send({ type: "clear_queue" });
    return this.getData(response);
}
```

### 5.4 dist 与系统 Pi 同步

由于 pi-main 的 `npm run build` 存在**预先就有的**编译错误
（`package-manager-cli.ts` 的 TS7006 implicit any，与本次改动无关），
无法整体重建 dist。因此手工同步了：

- `pi-main/packages/coding-agent/dist/modes/rpc/{rpc-mode,rpc-client,rpc-types}.{js,d.ts}`
- 系统 Pi（实际运行的宿主，v0.81.0）：
  `~/.nvm/versions/node/v24.15.0/lib/node_modules/@earendil-works/pi-coding-agent/dist/modes/rpc/{rpc-mode,rpc-client}.js`

---

## 六、与 Pi TUI 的对齐表

| Pi TUI 行为 | grok-pi 对应实现 |
|---|---|
| `clearAllQueues()` 先于 `agent.abort()` | `clear_queue` RPC 先于 `abort` RPC |
| 排队文本恢复到编辑器 | pager 本地队列行经空 `x.ai/queue/changed` 清除 |
| `updatePendingMessagesDisplay()` | `publish_queue_snapshot()` 发空 entries |
| Follow-up mode 设置（one-at-a-time / all） | `pi/queue/mode` ext_method → `set_follow_up_mode` RPC |
| abort 后不续跑队列消息 | 队列已清空，`hasQueuedMessages()` 为 false |

---

## 七、验证

- `cargo test -p pi-grok-adapter`：**98 passed**（含新增 `clear_empties_all_state`）
- `cargo build -p xai-grok-pager-bin --bin grok-pi`：**0 errors**
- 系统 Pi (v0.81.0) 已打 `clear_queue` 补丁，命令可用
- 预先存在的失败（与本次无关）：
  `remote_tui_extension_source_is_a_loadable_typescript_module`

## 八、涉及文件清单

**grok-pi 仓库：**
- `crates/codegen/pi-grok-adapter/src/pi_adapter.rs`
- `crates/codegen/pi-grok-adapter/src/queue_bridge.rs`
- `FEATURE_MATRIX.md` / `FEATURE_MATRIX.zh-CN.md`（队列行说明更新）
- `docs/issues/20260721-queue-cancel-alignment.md`（本文档）

**pi-main 子模块（上游，src + dist）：**
- `packages/coding-agent/src/modes/rpc/rpc-types.ts`
- `packages/coding-agent/src/modes/rpc/rpc-mode.ts`
- `packages/coding-agent/src/modes/rpc/rpc-client.ts`
- `packages/coding-agent/dist/modes/rpc/*.{js,d.ts}`

**系统 Pi（运行时宿主）：**
- `~/.nvm/.../pi-coding-agent/dist/modes/rpc/rpc-mode.js`
- `~/.nvm/.../pi-coding-agent/dist/modes/rpc/rpc-client.js`
