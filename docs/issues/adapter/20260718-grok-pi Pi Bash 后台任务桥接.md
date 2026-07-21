---
id: "2026-07-18-grok-pi-background-bash"
title: "grok-pi Pi Bash 后台任务桥接"
status: "completed"
created: "2026-07-18"
updated: "2026-07-18"
category: "adapter"
tags: ["grok-pi", "pi-extension", "bash", "background-tasks"]
---

# Issue: grok-pi Pi Bash 后台任务桥接

## Goal

让 `grok-pi` 通过私有 Pi Extension 增强内置 `bash`：extension 持有 Bash 子进程，并通过 Pi `createBashToolDefinition` 保持前台的标准输出/渲染语义；`is_background: true` 与运行中的前台转后台均投影到现有 Grok Pager 原生后台任务卡片与生命周期。

## 背景/问题

Pi 内置 Bash 没有 Grok `run_terminal_command` 的后台任务协议。Pager 已有 `is_background`、`x.ai/task_backgrounded` 和 `x.ai/task_completed` 的原生承载面，但 Pi adapter 尚未把 Pi Extension 生命周期事件投影到这些通知。

## 验收标准

- [ ] 前台 `bash` SHALL 复用 Pi `createBashToolDefinition` 的参数、流式 `onUpdate`、取消、超时、退出错误、截断与完整输出路径语义；extension 保留子进程所有权以支持转后台。
- [ ] `is_background: true` SHALL 返回任务 ID，并使用既有 `x.ai/task_backgrounded` / `x.ai/task_completed` 使 Pager 渲染原生任务状态。
- [ ] `get_task_output`、`wait_tasks`、`kill_task` SHALL 使用与 Pager tracker 兼容的参数名和任务 ID。
- [ ] Extension SHALL 只由 `grok-pi` 的 NamedTempFile 注入；独立 Pi 不加载它。
- [ ] 实现 SHALL 不引入外部 extension 源码、PTY、终端 UI 或 adapter terminal ownership。

## 实施阶段

### Phase 1: 规划和准备
- [x] 核对 Pi 同名 `bash` override 和 `createBashToolDefinition` 的官方行为。
- [x] 核对 Pager 已有后台任务生命周期协议和 schema。
- [x] 确定前台透传、后台 extension 自管状态、adapter 仅投影通知的边界。

### Phase 2: 执行
- [x] 新建私有 Bash override extension 与任务查询/等待/终止工具。
- [x] 通过 `grok-pi` NamedTempFile 注入 extension。
- [x] 将 extension custom message 投影为 `x.ai/task_backgrounded` / `x.ai/task_completed`。

### Phase 3: 验证
- [x] Extension source/injection 测试。
- [x] Adapter bridge projection 单元测试。
- [x] `cargo test -p pi-grok-adapter` 与 `cargo check -p xai-grok-pager-bin --bin grok-pi`。

## 关键决策

| 决策 | 理由 |
|------|------|
| 同名 `bash` 覆盖 | Pi 官方注册表支持 extension 覆盖 built-in，能保留模型可见工具名。 |
| 前台复用 `createBashToolDefinition`，由 extension 托管 child | 保持 Pi 标准输出/渲染语义，同时让 Pager 可将同一运行中进程转后台；不重跑命令。 |
| 后台使用 versioned custom message | Pi RPC 已回传 custom message；adapter 可投影到 Pager 已有原生任务通知。 |
| 不复用外部 extension 代码 | 外部路径仅作 API/机制参考；实现必须独立编写。 |

## Notes

- 2026-07-20：后台任务完成消息此前固定 `triggerTurn:false` 且正文为空。Pager 可收到 task completion，但 Pi agent 不会被唤起或获得失败上下文。失败（非显式 kill）现在会把命令、输出、exit code/signal 作为 custom message 注入 Pi；idle 时立即触发下一轮，streaming 时按 follow-up 队列投递。成功仍静默，只投影 Pager。
- 2026-07-21：Pager 任务卡 kill 点击此前无效。根因是 adapter `ext_method` 对 `x.ai/task/kill` 走默认空成功分支，未触达 Pi Bash 子进程。现经同一 control 通道追加 `op:"kill"` + `taskId`，extension 发布 `runningTaskIds` 供校验，并返回 `ExtMethodResult<KillTaskResponse>`（`killed` / `not_found`）；进程结束后仍投影 `x.ai/task_completed`。
- `pi-grok-cli` 的 `Shell` shim 仅展示 `createBashToolDefinition` 委托方式，不含后台任务实现，且不会被复制。
- `pi-interactive-shell` 依赖 PTY 与 `ctx.ui.custom()`，不适用于 Pi RPC，明确排除。
- 后台启动生命周期从同步 tool result details 投影，以避免在 Pi streaming 时 custom message 排队导致任务卡延迟；完成 custom message 会先补入累计 stdout，再发送完成通知。
- 工作树已有无关 Remote-TUI 实验改动；本 Issue 不修改其文件。

---

## Status 更新日志

- **[2026-07-18]**: 状态变更 → in_progress，备注: 已完成协议与 API 研究，开始实现。
- **[2026-07-18]**: Phase 2 完成，备注: 私有 Bash extension、NamedTempFile 注入和原生 task lifecycle 投影已落盘。
- **[2026-07-18]**: 状态变更 → completed，备注: adapter 68 项测试、grok-pi bin 10 项测试、cargo check，以及 Pi runtime/Bun smoke 均通过。
- **[2026-07-18]**: 后续 `Send to Background` 工作调整前台所有权：不再由 Pi 内建 Bash 直接 spawn，改为 extension 托管 child 并复用 `createBashToolDefinition`；详见 `docs/issues/pager/20260718-补齐 grok-pi 工具卡 Send to Background.md`。
- **[2026-07-20]**: 修复后台 Bash 失败的 agent 通知：失败任务的 custom message 进入 Pi 上下文并触发后续 turn；成功与显式 kill 继续静默。
