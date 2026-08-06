---
id: "2026-08-06-为 pi-grok-bash 添加 Python JavaScript Eval"
title: "为 pi-grok-bash 添加 Python/JavaScript Eval"
status: "completed"
created: "2026-08-06"
updated: "2026-08-06"
category: "adapter"
tags: ["workhub", "grok-pi", "bash", "eval", "python", "javascript"]
---

# Issue: 为 pi-grok-bash 添加 Python/JavaScript Eval

## Goal

在不修改 Pi 源码的前提下，由 grok-pi 私有 `pi-grok-bash` 扩展提供 Python/JavaScript 持久 Eval 工具，并通过 F2 的 Pi built-in tools 设置控制，默认关闭。

## 背景/问题

当前 grok-pi 的私有 Bash 扩展只提供前台/后台 shell 执行。用户希望参考 `~/Dev/AI/oh-my-pi` 的 Eval 使用方式，在同一会话中增量执行 Python 或 JavaScript，并复用此前定义的变量。该能力必须保持 Grok Pager 仅负责设置与展示、Pi 继续负责 agent/tool 生命周期的架构边界。

## 验收标准 (Acceptance Criteria)

- [x] F2 → Pi built-in tools 中出现 `Eval`，默认值为关闭，修改后对下一个 grok-pi 会话生效。
- [x] Eval 开启时，模型可调用独立 `eval` 工具，并使用 `py` 或 `js`、`code`、可选 `title`、`timeout`、`reset` 参数。
- [x] 同一语言的变量和导入在后续 Eval 调用中保持；Python 与 JavaScript 状态彼此隔离。
- [x] `reset=true`、超时、中止或内核异常会销毁对应语言内核，后续调用获得干净内核。
- [x] Eval 关闭时，`pi-grok-tools` 从 active tools 中移除 `eval`；显式 CLI 工具限制仍然优先。
- [x] 不修改 `pi-main` 或安装的 Pi 包，不引入 Ruby/Julia、图片显示、Agent bridge 等超出需求的能力。

## 实施阶段

### Phase 1: 规划和准备
- [x] 核对 F2 设置、配置持久化、扩展注入和 active tools 链路。
- [x] 阅读 oh-my-pi Eval 参数与持久内核语义。
- [x] 用独立原型验证 Node REPL 的状态/顶层 await 与 Python AST 的持久命名空间。

### Phase 2: 执行
- [x] 补齐 `pi_builtin_tools.eval` 的设置定义、读取、重置、动作和启动选择。
- [x] 在 `pi-grok-bash` 中实现 Python/JavaScript 持久内核与 `eval` 工具。
- [x] 补齐扩展源码静态断言及默认关闭测试。

### Phase 3: 验证
- [x] 运行扩展格式/类型检查。
- [x] 运行目标 Rust 单元测试与 grok-pi binary check。
- [x] 执行 Python/JavaScript 双语言持久状态、reset、超时回归。
- [x] 审查 `git diff --check` 与最终差异，确认未覆盖既存工作树改动。

### Phase 4: 交付
- [x] 更新验证结果与最终状态。

## 关键决策

| 决策 | 理由 |
|------|------|
| Eval 作为独立工具注册，但源码与生命周期归 `pi-grok-bash` 扩展 | 保持工具参数清晰，同时复用 grok-pi 私有执行扩展边界，不修改 Pi。 |
| JavaScript 使用隔离子进程中的 Node REPL；Python 使用隔离子进程中的 AST/asyncio runner | 两者都支持跨调用状态与顶层 await；超时或中止可通过销毁子进程可靠恢复。 |
| F2 默认关闭，并将 `eval` 纳入 `PI_GROK_BUILTIN_TOOLS` 选择集合 | 与现有 built-in tools 行为一致，且不会改变默认工具面。 |
| 仅实现 Python/JavaScript 核心语义 | 避免复制 oh-my-pi 的 Ruby/Julia、富媒体与 agent bridge 大型子系统。 |

## 验证结果

- `eval loader harness`: PASS。覆盖 JavaScript/Python 持久状态、输出、`reset`、超时、中止以及销毁后的干净内核。
- `biome check --formatter-enabled=false`: PASS；新增改动行与 Biome formatter 的重叠为空。
- `git diff --check`: PASS。
- `./scripts/cargo-shared.sh test -p xai-grok-pager-bin --bin grok-pi`: PASS，72/72。
- `./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi`: PASS。
- Cargo 仅报告仓库已有的 dead-code/unused warnings，本次未扩大处理范围。

## 遇到的错误

| 日期 | 错误 | 解决方案 |
|------|------|---------|
| 2026-08-06 | Python 原型的 await 示例未在持久命名空间导入 `asyncio` | 将测试改为先导入再复用；内核设计本身不受影响。 |

## 相关资源

- `extensions/pi-grok-bash/index.ts`
- `extensions/pi-grok-tools/index.ts`
- `crates/codegen/xai-grok-pager/src/settings/defs.rs`
- `~/Dev/AI/oh-my-pi/packages/coding-agent/src/tools/eval.ts`

## Notes

当前工作树另有 README、CLI 和 `grok-pi.rs` 改动；本 Issue 只追加 Eval 所需局部差异，不能回退或覆盖这些既存修改。

---

## Status 更新日志

- **2026-08-06 22:18**: 状态变更 → in-progress，备注: 研究与原型验证完成，开始实现。
- **2026-08-06 22:57**: 状态变更 → completed，备注: Eval、F2 开关与工具过滤链路完成；功能回归、Biome、72 项 Rust 测试及 binary check 通过。
