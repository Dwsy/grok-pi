---
id: "2026-07-30-拆分 pi-grok-adapter 长文件"
title: "拆分 pi-grok-adapter 长文件"
status: "done"
created: "2026-07-30"
updated: "2026-07-30"
category: "adapter"
tags: ["workhub", "拆分 pi-grok-adapter 长文件"]
---

# Issue: 拆分 pi-grok-adapter 长文件

## Goal

在不改变行为的前提下，将 `pi_adapter.rs` 的会话、事件/工具、ACP 协议与测试拆成职责清晰的内部模块，并同步拆出 `model.rs`、`tool_projection.rs` 的测试，降低单文件维护成本。

## 背景/问题

`pi_adapter.rs` 原有 6291 行，同时承担状态、会话、事件、工具、扩展协议与测试；`model.rs` 原有 2148 行，`tool_projection.rs` 原有 1244 行。逻辑已按领域形成簇，但仍集中在长文件中，阅读、审查和上游窄接缝维护成本过高。用户要求先提交现有工作，再做纯结构拆分；本阶段禁止子代理和 Cargo。

## 验收标准 (Acceptance Criteria)

- [x] WHEN 阅读适配器入口时，系统 SHALL 将会话、事件/工具、ACP 协议和测试放在独立内部模块中。
- [x] WHERE `pi-grok-adapter` 公共边界，系统 SHALL 保持 `PiAgent`、`PiBootstrap` 与现有导出不变。
- [x] IF 不运行 Cargo，THEN 系统 SHALL 至少通过 `rustfmt --check`、`git diff --check`、源码结构检查与完整 diff 审查。
- [x] 所有拆分变更 SHALL 只移动代码、格式化长签名或调整模块内可见性，不改变协议字段、分支逻辑或用户可见行为。
- [x] 拆分结果 SHALL 以聚焦提交保存到本地，且不 push。

## 实施阶段

### Phase 1: 规划和准备
- [x] 统计文件行数并确认工作树现状
- [x] 分组提交拆分前的既有代码
- [x] 设计仅移动代码的模块边界

### Phase 2: 执行
- [x] 提取会话与计划生命周期模块
- [x] 提取通知、队列、回放、事件与工具/UI 模块
- [x] 提取 ACP Agent 协议与扩展请求模块
- [x] 提取 `pi_adapter`、`model` 与 `tool_projection` 测试模块

### Phase 3: 验证
- [x] `rustfmt --edition 2024 --check`（仅相关模块入口）
- [x] `git diff --check`
- [x] 源码结构与行数检查
- [x] 函数体与类型体对比确认无逻辑改动
- [ ] Cargo 验证留待用户后续指定（按用户要求未运行）

### Phase 4: 交付
- [x] 更新 Issue 实施记录
- [x] 创建本地聚焦提交 `b8e352f`
- [x] 创建本地 PR 记录
- [x] 确认未 push

## 关键决策

| 决策 | 理由 |
|------|------|
| 使用 `pi_adapter/` 子模块并保留根模块中的共享状态与纯辅助函数 | 子模块可访问父模块内部项，避免公开状态、复制依赖或改变公共边界 |
| 跨子模块方法仅提升为 `pub(super)` | 将可见性限制在 `pi_adapter` 父模块树内 |
| 按通知、队列、回放、运行时事件、工具/UI、会话、ACP Agent 拆分 | 每个文件对应稳定职责，最大运行时子模块降到约 1190 行 |
| 测试使用 Rust 文件模块拆出 | 不改变测试函数、断言或覆盖范围 |
| 不处理生成的 session HTML | 与本任务无关，保持未跟踪状态 |

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-07-30 | `rg` 结构表达式在当前输出过滤下无结果 | 改用 Python 逐行结构扫描与受控文件读取 |
| 2026-07-30 | 初次事件模块切分把方法文档注释放在前一文件末尾 | 将切分标记前移到文档注释起点并重新运行 `rustfmt` |
| 2026-07-30 | `rustfmt` 为换行后的函数参数补尾逗号，初版文本比较误报 5 项 | 等价性比较规范化仅由格式器引入的参数尾逗号后重新验证 |

## 结果

- `pi_adapter.rs`: 6291 → 1521 行。
- 新增职责模块：`agent.rs` 1190、`session.rs` 999、`tools.rs` 759、`events.rs` 416、`notifications.rs` 400、`queue_runtime.rs` 321、`replay.rs` 191、`tests.rs` 539 行。
- `model.rs`: 2148 → 1600 行，测试移至 `model/tests.rs`（547 行）。
- `tool_projection.rs`: 1244 → 856 行，测试移至 `tool_projection/tests.rs`（386 行）。
- 结构等价检查：`pi_adapter` 198 个函数/8 个类型、`model` 78 个函数/12 个类型、`tool_projection` 44 个函数/3 个类型，拆分前后数量与规范化代码体一致。

## 相关资源

- [x] 项目约束: `AGENTS.md`
- [x] Adapter 边界: `crates/codegen/pi-grok-adapter/src/lib.rs`
- [x] 相关 Issue: `docs/issues/adapter/20260718-提取 Pi 上下文投影模块.md`、`docs/issues/adapter/20260718-提取 Pi 工具卡片投影模块.md`
- [x] PR 记录: `docs/pr/adapter/20260730-拆分 pi-grok-adapter 长文件.md`

## Notes

拆分前既有工作已分为 4 个本地提交；拆分代码提交为 `b8e352f`。全程未使用子代理、未运行 Cargo、未 push。生成的 session HTML 未纳入版本控制。

---

## Status 更新日志

- **[2026-07-30 00:32]**: 状态变更 → in_progress，备注: 完成现状盘点与拆分前分组提交
- **[2026-07-30 00:44]**: 状态变更 → done，备注: 完成职责拆分、静态等价验证与本地代码提交
