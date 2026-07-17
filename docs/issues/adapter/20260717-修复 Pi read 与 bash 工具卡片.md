---
id: "2026-07-17-修复-pi-read-与-bash-工具卡片"
title: "修复 Pi read 与 bash 工具卡片显示与弹窗"
status: "completed"
created: "2026-07-17"
updated: "2026-07-17"
category: "adapter"
tags: ["workhub", "pi-rpc", "read", "bash", "tool-card"]
---

# Issue: 修复 Pi read 与 bash 工具卡片显示与弹窗

## Goal

让 Pi 的 `read` / `bash` 工具在 Grok 原生工具卡与 Block Viewer 中正确显示：read 展示路径与行数并可打开弹窗；bash 弹窗同时展示命令输出。

## 背景/问题

1. `read` 卡片只显示工具名 `read`，没有文件路径和行数；展开只折叠，无法打开弹窗。
2. `bash` 打开 Block Viewer 后只有入参（命令），没有 stdout 出参。

## 已确认根因

- 适配器曾将 `read` 映射为 `ToolKind::Other`，以绕过原生 Read 卡对 Grok `ToolOutput::ReadFile` 的依赖；Other 卡无路径摘要，且不支持 fullscreen viewer。
- Pi 的 `tool_execution_*` / direct `bash` 结果是 `{ content: [{type,text}], details }` 或 `{ output, exitCode }`，Pager Execute 卡从 `raw_output` 反序列化 `ToolOutput::Bash`，匹配失败时只渲染命令。

## 验收标准 (Acceptance Criteria)

- [ ] WHEN Pi `read` 完成，系统 SHALL 在原生 Read 卡显示文件路径，并在可知时显示行范围/总数。
- [ ] WHEN 用户对成功的 read 打开 Block Viewer，系统 SHALL 显示文件内容而非仅折叠标题。
- [ ] WHEN Pi `bash` 完成且有 stdout，系统 SHALL 在 Execute 卡与弹窗中显示输出。
- [ ] 适配器保持 headless，不引入 Pager UI 依赖。

## 实施阶段

### Phase 1: 规划和准备
- [x] 定位 read/bash 从 Pi RPC → ACP → Pager 的投影路径。
- [x] 确认原生卡所需的 `raw_output` 形状。

### Phase 2: 执行
- [x] 将 `read` 映射回 `ToolKind::Read`。
- [x] 将 Pi read/bash 结果规范化为 `ToolOutput::ReadFile` / `ToolOutput::Bash`。
- [x] 缓存 tool args，供 end 事件补全 path/command。
- [x] 补充适配器单元测试。

### Phase 3: 验证
- [x] 运行 `cargo test -p pi-grok-adapter`（21 passed）。
- [x] 审查 diff 范围（仅 `pi-grok-adapter` + issue 文档）。

### Phase 4: 交付
- [x] 更新本 Issue 验证证据与状态。

## Notes

- Edit 工具已有独立 issue（diff 内容投影）；本 issue 只覆盖 read/bash 的 typed raw_output。
- Bash 实时流式输出仍受 Pi RPC 无中间事件限制（见 bash 流式输出 issue）。
