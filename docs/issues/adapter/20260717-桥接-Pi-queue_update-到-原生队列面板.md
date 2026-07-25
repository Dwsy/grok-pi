---
id: "2026-07-17-桥接-pi-queue-update-到-原生队列面板"
title: "Pi / Grok 原生队列隔离与生命周期对齐"
status: "completed"
created: "2026-07-17"
updated: "2026-07-25"
category: "adapter"
tags: ["workhub", "queue", "follow-up", "steer", "isolation", "extension"]
---

# Issue: Pi / Grok 原生队列隔离与生命周期对齐

## Goal

同时提供两种真实语义：

- **Send now / steering**：注入当前 turn；已有待执行行可原子移除后立即发送，不重复执行。
- **Append / follow-up**：先留在 Grok 可管理队列，当前 turn 完成后才真正发送给 Pi。

并修复 extension（尤其 `~/.pi/agent/extensions/loop.ts`）通过 `sendUserMessage(..., { deliverAs: "followUp" })` 产生的消息被吞到末尾、响应已结束但 UI 仍等待、Esc 长期停在 Cancelling 的问题。

## 根因

1. Pager 有稳定 id/version 与完整 CRUD 协议，但 stock Pi 0.81.1 RPC 队列只有文本数组，没有单行 id、编辑、删除、重排或 `clear_queue`。
2. 直接把 Pager 行复制成 Pi `steer` 无法原子删除原 follow-up，会造成二次执行。
3. extension `sendUserMessage` 使用 `source="extension"`，过去会直接进入 Pi 私有队列，绕过 Pager 行生命周期。
4. extension 驱动的新 turn 没有 ACP prompt waiter；只依赖客户端 PromptResponse 会让 Pager 在 Pi 已空闲时仍显示 Waiting。
5. 取消过程中 `loop.ts` 的终止处理器仍可能再次发 follow-up，若接收该消息，取消完成后循环会“复活”。

## 最终架构

### A. Adapter-owned isolated lane

客户端 active-turn follow-up 与被拦截的 extension 消息先进入 `QueueMirror.local_entries`，此时**尚未发送给 Pi**。

每行保存：

- 稳定 `id` 与 `version`
- `execution_text` / `display_text`
- images
- lane（steering / follow-up）
- origin（client / extension）

因此待执行行可以真实执行：

| ACP 扩展方法 | 行为 |
|---|---|
| `x.ai/queue/remove` | 按 id + expectedVersion 原子移除 |
| `x.ai/queue/clear` | 清空本地待执行行，并完成客户端 waiter 为 Cancelled |
| `x.ai/queue/edit` | 修改文本并递增 version |
| `x.ai/queue/reorder` | 重排本地行；未列出的行保持相对顺序并追加 |
| `x.ai/queue/interject` | 原子取出本地行；运行中发送 Pi `steer`，空闲时作为正常新 turn 执行 |

### B. Extension interception

最先注入的 `pi-grok-rpc-compat` extension 监听 Pi 官方 `input` 事件：

- 仅拦截 `event.source === "extension"`
- 通过内部 `setStatus(__pi_grok_queue_enqueue__)` 把 text/images/streamingBehavior 交给 Rust adapter
- 返回 `{ action: "handled" }`，阻止消息进入 Pi 私有 follow-up 队列
- adapter 真正调度时用 RPC source 重新发送，因此不会再次被拦截

`loop.ts` 当前调用路径已核对：`pi.sendUserMessage(loopState.prompt, { deliverAs: "followUp" })`，会进入上述隔离层。

### C. Pi external lane

绕过兼容扩展、已存在于 Pi 私有队列的消息仍由 `queue_update` 全量数组镜像为 `pi_entries`。

该通道保持只读边界：stock Pi RPC 无法按行原子删除/编辑/重排。外部 follow-up 出队时可成为 running；steering 出队只注入当前 turn，不覆盖 primary `runningPromptId`。

### D. Dispatch and completion

- adapter 空闲时取本地首行，设置 running，再发送普通 Pi RPC prompt。
- 客户端行把 queued completion sender 提升到 `active_prompts`，直到 turn 完成才回复 ACP PromptResponse。
- extension/Pi-owned running 行在完成时发送 `x.ai/session/prompt_complete`。
- 正常完成屏障是 Pi `agent_settled`，不是 `agent_end`。
- 每次 adapter-owned dispatch 都启动短延迟 idle probe；若其他 input handler 消费了 RPC prompt、没有产生 `agent_start`/`agent_settled`，探针会清理 ghost running、完成通知并继续下一行。

### E. Cancel barrier

ACP cancel：

1. 同步标记 cancelling。
2. 清空 adapter-owned pending rows，完成所有客户端 waiter 为 Cancelled。
3. 清除当前 running 展示并立即广播空队列，Pager 不再卡 Cancelling。
4. fire-and-forget 发送 Pi `abort` / `abort_bash`，避免 Pi 内部等待导致 ACP cancel 阻塞。
5. `get_state` 探针等待 Pi 真正 idle 后再允许新调度。
6. cancelling 期间 extension 新产生的 continuation 直接丢弃，防止 `loop.ts` 在 Esc 后重启循环。

用户在取消期间新提交的客户端消息仍可排入本地队列，并在 Pi idle 后执行；只屏蔽自动 extension continuation。

## 验收

1. active turn 追加多条消息：按 Pager 顺序显示，可编辑、删除、重排、清空。
2. 队列行 Send now：只执行一次；运行中走 steer，空闲时走新 turn。
3. `loop.ts` follow-up 出现在原生队列正确位置，不被吞到最后。
4. extension-driven turn 结束后 Waiting 状态收敛；被 input handler handled 的 prompt 也由 idle probe 收敛。
5. Esc 清空待执行行并立即完成 ACP 请求；取消期间 loop continuation 不会复活。
6. Pi 外部队列行保持只读并给出边界提示，不伪造成功。
7. Pi 文本经 input handler / skill / template / plan reminder 改写后，外部镜像仍尽量按 lane FIFO 保留 reservation id 与原始展示文本。

## 验证记录（2026-07-25）

- `cargo test -p pi-grok-adapter`：127 passed。
- `cargo check -p xai-grok-pager-bin --bin grok-pi`：通过，仅既有 warnings。
- `queue_bridge` 覆盖稳定 id、文本变换、外部出队、CRUD、重复文本 FIFO 与 running preservation。
- `rpc_compat_extension` embedding test 校验 extension-source 拦截与 handled 短路。

## 边界

- 不修改 Pi 源码，不给 stock RPC 私加命令。
- Pi 外部私有队列仍无单行 CRUD / guaranteed clear；只有 adapter-owned isolated lane 具备完整功能。
- 单进程、单 Pi session；不实现多客户端共享队列共识。
- 未执行真实模型驱动的长循环端到端手测，仍建议用 `loop.ts` 做一次人工验收。
