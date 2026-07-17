---
id: "2026-07-17-Pi-rpiv-btw-与原生-btw-适配策略"
title: "Pi rpiv-btw 与原生 /btw 适配策略"
status: "todo"
created: "2026-07-17"
updated: "2026-07-17"
category: "adapter"
tags: ["workhub", "adapter", "btw", "rpiv-btw", "rpc", "side-question"]
---

# Issue: Pi rpiv-btw 与原生 /btw 适配策略

## Goal

在 **不修改 `@juicesharp/rpiv-btw` 源码**、运行形态为 **Pi JSONL RPC + grok-pi** 的前提下，评估如何处理侧问 `/btw`：是桥接 juicesharp 插件，还是直接复用 Grok 原生 `/btw` surface。

## 约束（用户确认）

1. 不改 juicesharp 插件源码。
2. Pi RPC + pi-grok adapter + Grok Pager。
3. 只复用原生 Grok surface；adapter headless。

## 两边事实

### juicesharp `rpiv-btw`

| 项 | 值 |
|---|---|
| 入口 | slash `/btw <question>`（**不是 tool**） |
| UI | `showBtwOverlay` → Pi TUI bottom panel（`ctx.ui` + pi-tui） |
| 模型调用 | 在 **Pi 进程内** `completeSimple`（tool-less side agent） |
| 上下文 | branch snapshot + 本 session `/btw` history（`globalThis`） |
| 持久化 | 进程内；主 transcript 不写 |
| RPC 行为 | README 明确：`/btw requires interactive mode`；`custom`/overlay 依赖 TUI |

RPC 下：`ctx.hasUI` 可能为 true，但 overlay 工厂同样需要真实 TUI；插件会尝试 `showBtwOverlay`，在 RPC 无 TUI 时失败或不可用。

### Grok 原生 `/btw`

| 层 | 路径 |
|---|---|
| Slash | `xai-grok-pager/.../slash/commands/btw.rs` → `Action::SendBtw` |
| Effect | `Effect::SendBtw` → ACP `ext_method` **`x.ai/btw`** |
| Shell | `xai-grok-shell/.../extensions/feedback.rs::handle_btw` → `SessionCommand::SideQuestion` |
| UI | `views/btw_overlay.rs` + scrollback `BtwBlock` |

### pi-grok adapter 现状

`ext_method` 已实现：`x.ai/interject`、`x.ai/compact_conversation`、`pi/session/list`、`x.ai/session/rename`。  
**未实现 `x.ai/btw`** → pager 发 SendBtw 会得到 MethodNotFound。

Pi RPC **没有** side-question / completeSimple 代理命令；`rpiv-btw` 的模型调用发生在 extension 进程内，不经过 adapter。

## 结论

### 直接「映射 rpiv-btw 插件」到原生 overlay

**不可行（不改插件）。**

原因：

1. 插件在 Pi 内自己画 overlay + 自己 `completeSimple`；
2. RPC 不导出 side-call 事件流；
3. adapter 看不到中间态，只能看到 slash 是否通过 prompt/command 转发；
4. Pi 的 `/btw` 若作为 AvailableCommand 被 pager 以「普通 slash → prompt」方式送回 Pi，仍会进插件 handler，然后撞上无 TUI / 坏 overlay。

### 正确产品路径：**原生 Grok `/btw` + adapter 实现 `x.ai/btw`**

不碰 juicesharp 包。让 grok-pi 用户走 **Pager 已有的** `/btw`：

```
用户 /btw q
  → pager BtwCommand / SendBtw
  → ACP ext_method x.ai/btw { sessionId, question }
  → pi-grok-adapter 实现：
       用 Pi 当前 session 消息做 read-only 上下文
       + 侧问 system prompt
       + 对当前 model 做一次无工具补全
       → 返回 { answer }
  → pager 原生 btw_overlay / BtwBlock
```

| 点 | 说明 |
|---|---|
| UI | 100% 原生，零第二 TUI |
| 不改插件 | 是；甚至可 **禁用/不装** rpiv-btw，避免 slash 名冲突 |
| 上下文 | adapter 从 Pi `get_messages` / history 取 branch 近似；follow-up history 可进程内缓存（对齐 rpiv 语义） |
| 模型 | 用 Pi 当前 model + 已有 RPC auth 路径；若 Pi 无「裸 complete」RPC，需评估：`prompt` 会污染主会话 → **不能直接 prompt** |

**关键风险：Pi RPC 是否提供「不写主 transcript 的 side completion」？**

当前 `rpc-types` 只有 `prompt/steer/follow_up/bash/...`，**没有** `complete_side`。因此 adapter 实现 `x.ai/btw` 有两条子路径：

| 子路径 | 做法 | 主 transcript | 评价 |
|---|---|---|---|
| A. 仅 adapter 内直接调 provider | 复制 model id/api key 从 Pi state | 不污染 | 可能重复 auth/provider 逻辑；架构上 adapter 变厚 |
| B. 扩展 Pi RPC（改 pi-main）加 `side_complete` | Pi 内 completeSimple | 不污染 | 改 Pi 源码，违反「尽量不改 Pi」但 **不是改 juicesharp** |
| C. 用 `prompt` + 事后回滚 | 污染后 fork/清理 | 差 | 否决 |
| D. 不实现，文档说明 grok-pi 暂无 /btw | — | — | 诚实 fallback |

**在不改 Pi RPC 的前提下，A 是唯一能严格不污染主会话的路径，但实现成本与安全边界要评估（密钥是否从 Pi 进程可拿）。**  
更干净的是 **B：官方 extension 或 pi-main RPC 增加 side_complete**——仍不改 juicesharp。

### slash 名冲突

- 若同时装 rpiv-btw：Pi `get_commands` 会暴露 `btw`；pager 可能把它当 ACP command 转发，与本地 `BtwCommand` 打架。
- **策略**：grok-pi 文档要求 **不装 rpiv-btw**；pager 本地 `/btw` 优先；或 adapter 从 AvailableCommands 过滤掉 Pi 的 `btw`。

## 推荐决策

| 优先级 | 动作 |
|---|---|
| P0 产品 | **不桥接 rpiv-btw**；用原生 `/btw` UI |
| P0 协议 | adapter 实现 `x.ai/btw` **或** 明确 MethodNotFound + UI 提示「Pi 后端暂不支持侧问」 |
| P1 | 若要完整语义：pi-main 加 side completion RPC / 或 adapter 安全侧调模型 |
| 文档 | FEATURE_MATRIX：`rpiv-btw` = 不适用；`/btw` = 原生 + adapter（状态：未实现/进行中） |

## 验收标准（若实现原生路径）

- [ ] WHEN 用户在 grok-pi 输入 `/btw <q>`，系统 SHALL 打开原生 btw overlay，而非无响应或 MethodNotFound 静默失败。
- [ ] WHEN 侧问完成，系统 SHALL 显示 answer，且主 Pi session transcript 不增加该轮问答。
- [ ] WHEN 用户 Esc，系统 SHALL 取消/关闭 panel。
- [ ] WHERE 已装 rpiv-btw，系统 SHALL 不因双重 `/btw` 产生错误路由（过滤或文档禁用）。
- [ ] 不修改 `@juicesharp/rpiv-btw` 源码。

## 与 todo / ask 对比

| 插件 | 交互类型 | 不改插件 + RPC only |
|---|---|---|
| rpiv-todo | tool result 单向投影 | **可行** → ACP Plan |
| rpiv-ask-user-question | 阻塞 custom UI | **不可行** → 需自有 tool 替换 |
| rpiv-btw | slash + 进程内 side model + TUI overlay | **不可行映射** → 用原生 `/btw` + 实现 `x.ai/btw` |

## Notes

- 评估日期：2026-07-17
- 用户约束：不改插件；Pi RPC + pi-grok

## Status 更新日志

- **2026-07-17**: 状态 → `todo`，备注: 完成评估；推荐原生 /btw + adapter，不桥接 rpiv-btw
