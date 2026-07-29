---
id: "2026-07-30-拆分 pi-grok-adapter 长文件"
title: "拆分 pi-grok-adapter 运行时职责"
status: "ready"
created: "2026-07-30"
updated: "2026-07-30"
category: "adapter"
tags: ["workhub", "pr", "refactor", "pi-grok-adapter"]
---

# 拆分 pi-grok-adapter 运行时职责

> 将 6291 行的适配器入口按职责拆成内部模块，并移出两个长文件中的测试；公共边界和运行行为保持不变。

## 背景与目的 (Why)

`pi_adapter.rs` 同时承载 ACP Agent、会话/计划生命周期、通知、队列、历史回放、事件、工具与扩展 UI，导致阅读和审查成本过高。`model.rs` 与 `tool_projection.rs` 的内联测试也显著抬高主体文件行数。本次变更仅做结构拆分，便于后续窄接缝维护和上游同步。

## 变更内容概述 (What)

- 保留 `pi_adapter.rs` 中的公共类型、共享状态、构造/事件循环与纯辅助函数。
- 新增 `pi_adapter/agent.rs`、`session.rs`、`notifications.rs`、`queue_runtime.rs`、`replay.rs`、`events.rs`、`tools.rs`。
- 将 `pi_adapter`、`model`、`tool_projection` 的内联测试分别移到子模块 `tests.rs`。
- 跨子模块协作方法仅提升为 `pub(super)`，不扩大 crate 外可见性。
- 未改变协议字段、控制流、断言、依赖或公共导出。

## 关联 Issue

- **Issue:** `docs/issues/adapter/20260730-拆分 pi-grok-adapter 长文件.md`

## 测试与验证结果 (Test Result)

- [x] `rustfmt --edition 2024 --check` 通过
- [x] `git diff --check` 通过
- [x] 源码结构与行数检查通过
- [x] 拆分前后函数体/类型体规范化等价检查通过
- [ ] Cargo 单元与集成测试：按用户要求暂未运行，等待后续指定

等价检查结果：

| 区域 | 函数 | 类型 | 结果 |
|------|------|------|------|
| `pi_adapter` | 198 → 198 | 8 → 8 | 无缺失、无新增代码体 |
| `model` | 78 → 78 | 12 → 12 | 无缺失、无新增代码体 |
| `tool_projection` | 44 → 44 | 3 → 3 | 无缺失、无新增代码体 |

## 风险与影响评估 (Risk Assessment)

- 风险集中在 Rust 模块可见性和文件路径解析；通过 `pub(super)` 限制、`rustfmt` 解析和结构等价检查降低风险。
- `rustfmt` 仅对 5 个换行后的长函数签名补了参数尾逗号，无语义变化。
- 未运行 Cargo，因此依赖解析与完整编译验证明确留待后续。
- 不影响公共 API、协议负载、配置、持久化格式或运行时依赖。

## 回滚方案 (Rollback Plan)

执行 `git revert b8e352f` 可仅回退本次结构拆分；拆分前的 4 个既有工作提交保持独立，不受影响。

---

## 变更类型

- [ ] 🐛 Bug Fix
- [ ] ✨ New Feature
- [x] 📝 Documentation
- [x] 🚀 Refactoring
- [ ] ⚡ Performance
- [ ] 🔒 Security
- [x] 🧪 Testing（仅测试代码移动）

## 文件变更列表

| 文件 | 变更类型 | 描述 |
|------|---------|------|
| `crates/codegen/pi-grok-adapter/src/pi_adapter.rs` | 修改 | 保留共享状态、入口和纯辅助函数，声明职责子模块；6291 → 1521 行 |
| `crates/codegen/pi-grok-adapter/src/pi_adapter/*.rs` | 新增 | ACP Agent、会话、通知、队列、回放、事件、工具/UI 与测试模块 |
| `crates/codegen/pi-grok-adapter/src/model.rs` | 修改 | 移出内联测试；2148 → 1600 行 |
| `crates/codegen/pi-grok-adapter/src/model/tests.rs` | 新增 | 原 `model.rs` 测试，断言保持不变 |
| `crates/codegen/pi-grok-adapter/src/tool_projection.rs` | 修改 | 移出内联测试；1244 → 856 行 |
| `crates/codegen/pi-grok-adapter/src/tool_projection/tests.rs` | 新增 | 原工具投影测试，断言保持不变 |

## 详细变更说明

### 1. 拆分适配器运行时

**问题：** 单文件混合多个生命周期和协议职责，定位代码与审查变更困难。

**方案：** 按 ACP Agent、会话、通知、队列、回放、运行时事件、工具/UI 划分私有子模块；根模块继续拥有共享状态和公共入口。

**影响范围：** 仅 `pi-grok-adapter` 内部源码组织。

### 2. 拆出内联测试

**问题：** `model.rs` 和 `tool_projection.rs` 主体代码被大量测试拉长。

**方案：** 使用 Rust 文件模块 `#[cfg(test)] mod tests;` 移动原测试体。

**影响范围：** 测试路径变化，测试函数和断言不变。

## 测试命令

```bash
rustfmt --edition 2024 --check \
  crates/codegen/pi-grok-adapter/src/pi_adapter.rs \
  crates/codegen/pi-grok-adapter/src/model.rs \
  crates/codegen/pi-grok-adapter/src/tool_projection.rs

git diff --check
```

Cargo 命令按用户要求未执行。

## 破坏性变更

- [x] 否
- [ ] 是

## 性能影响

- [x] 无影响
- [ ] 提升
- [ ] 下降

## 依赖变更

- [x] 否
- [ ] 是

## 安全考虑

- [x] 无安全影响
- [ ] 有安全影响

## 文档变更

- [ ] 否
- [x] 是 — 更新本 Issue 与 PR 记录

## 代码审查检查清单

### 功能性
- [x] 公共边界保持不变
- [x] 协议字段与控制流未改变
- [x] 原测试函数与断言完整保留

### 代码质量
- [x] 模块按稳定职责命名
- [x] 内部可见性限制为 `pub(super)`
- [x] 最大适配器运行时文件降至约 1521 行，最大职责子模块约 1190 行

### 测试
- [x] 格式与 diff 静态检查通过
- [x] 函数体/类型体结构等价检查通过
- [ ] Cargo 测试等待用户指定

## 审查日志

- **[2026-07-30 00:44] 本地审查**: 完成格式、diff、行数和结构等价检查。
  - [x] 文档注释与方法保持同一模块
  - [x] 生成的 session HTML 保持未跟踪
  - [x] 未使用子代理，未运行 Cargo，未 push

## 最终状态

- **代码 Commit Hash:** `b8e352f`
- **合并状态:** 仅本地提交，未 push / 未创建远端 PR
- **部署状态:** 不适用
