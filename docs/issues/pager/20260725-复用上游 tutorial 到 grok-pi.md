---
id: "2026-07-25-复用上游 tutorial 到 grok-pi"
title: "复用上游 tutorial 框架实现 grok-pi 产品导览"
status: "done"
created: "2026-07-25"
updated: "2026-07-25"
category: "pager"
tags: ["workhub", "tutorial", "grok-pi", "pager-native", "pi-core"]
---

# Issue: 复用上游 tutorial 框架实现 grok-pi 产品导览

## Goal

保留上游 Pager tutorial 的模态框、状态机、Picker、Markdown、键鼠输入和 minimal 门控，为 grok-pi 提供完整产品导览。内容必须同时覆盖：

1. grok-pi 保留并复用的 Grok Pager 原生终端能力；
2. grok-pi 在 Pi RPC/extension 边界上实现的产品能力；
3. Pi Core 自带的多 Provider、多模型、会话树、扩展、Skills、Prompt Templates、Packages 与自定义 Provider/Tool 能力。

教程只描述仓库和当前 Pi 官方文档可证明的行为，并区分默认开启、F2 可选、实验性和明确边界。

## 背景

第一版仅把上游 `/tutorial` 加入 external profile，错误沿用了 stock Grok 文案。第二版改成九篇 grok-pi 专属内容，但 Git 历史、双语 Changelog、README、功能矩阵、命令表、F2 设置和 Pi 官方文档显示：九篇把模型/Provider、工具、上下文、扩展生态、资源安全、Review/Timeline/Rollback、后台任务与可选自动化过度压缩，产品能力仍有明显遗漏。

## 深度审计结论

### 能力来源

| 来源 | 代表能力 | 教程策略 |
|---|---|---|
| Grok Pager 原生 | terminal lifecycle、Welcome、PromptWidget、Markdown、tool/diff、scrollback、modal/picker、theme、voice | 说明“复用原生表面”，不把它写成 Pi 内核能力 |
| grok-pi 产品集成 | queue、context/cache、Plan、Pi session/tree/fork/clone、review、rollback、background Bash、subagent/dashboard、Remote TUI、export/update/isolation | 按真实默认和门控单列 |
| Pi Core/生态 | 多 Provider/模型、本地模型、自定义 Provider、extension events/tools/commands/shortcuts、Skills、Prompt Templates、Packages、AGENTS/CLAUDE context files | 说明 Pi 是能力所有者，grok-pi 负责原生展示或启动参数透传 |

### 九篇遗漏

- Provider 登录、多 Provider、自托管/本地模型与 `models.json`；
- 自定义 Provider、Extension lifecycle、动态工具/命令/快捷键；
- Skills、Prompt Templates、Packages、AGENTS/CLAUDE context files；
- CLI tool allowlist/denylist 与 F2 built-in tool selection；
- cache graph、recap/compaction、Review、Timeline、Tree file rollback；
- Remote TUI/extension UI 映射及其实验边界；
- HTML export/private share、GitHub update、diagnostics；
- 默认开启、默认关闭、重启生效与冲突包策略。

### 新结构：18 篇

1. 产品身份与边界
2. 原生终端与输入
3. Provider、模型与思考等级
4. 工具、流式事件与 Diff
5. Context、Cache、Compaction 与 Recap
6. Queue 与 Turn 控制
7. Session 生命周期与 Resume
8. Session Tree 与分支
9. Review、Timeline 与文件回滚
10. Plan Mode 与 Todo
11. Extensions 与动态命令
12. Extension UI、Remote TUI 与快捷键
13. Skills、Prompt Templates 与 Context Files
14. Themes、Packages、Resources 与 Trust
15. 后台 Bash 与 Tasks
16. Subagents 与 Dashboard
17. 可选交互与自动化
18. Export、Update、隔离与 Diagnostics

## 验收标准

- [x] `/tutorial`、`/tour`、`/onboarding` 在 fullscreen 可达，minimal 继续 fail-closed。
- [x] stock Grok 默认教程完全保留；grok-pi 只安装产品内容 profile。
- [x] grok-pi 教程包含 18 篇、标题唯一、每篇不超过 50 行、全部 `go_deeper=None`。
- [x] 多 Provider/本地模型、自定义 Provider、Extensions、Skills、Templates、Packages、会话树等 Pi 能力有独立介绍。
- [x] 默认开启、F2 默认关闭/重启、实验性 Remote TUI、资源冲突策略与边界均准确。
- [x] 不宣传 stock Grok-only 云会话、worktree、rewind、feedback、import-claude。
- [x] README、功能矩阵、架构说明、Issue 与 verifier 精确路径同步。
- [x] tutorial 定向测试、grok-pi 单测、cargo check、rustfmt、diff check 通过或记录既有 blocker。

## 实施计划

- [x] Phase 1：Git/Changelog/源码/Pi 官方文档能力考古。
- [x] Phase 2：确定 18 篇能力分类与门控边界。
- [x] Phase 3：重组 Markdown、profile 与覆盖测试。
- [x] Phase 4：更新 README/矩阵/架构说明/verifier。
- [x] Phase 5：运行定向与编译验证，回写结果。

## 验证结果

| 检查 | 结果 |
|---|---|
| tutorial profile tests | 3/3 PASS：18 篇结构、安装、30 个能力锚点、默认/可选/实验边界与 stock-only 禁词 |
| grok-pi bin tests（单线程） | 59/59 PASS |
| `cargo check -p xai-grok-pager --lib` | PASS，0 error；仅既有 warning |
| `cargo check -p xai-grok-pager-bin --bin grok-pi` | PASS，0 error；仅既有 warning |
| targeted `rustfmt --check` / `git diff --check` | PASS |
| 教程篇幅 | 18 篇；最长 19 行，全部低于 50 行限制 |
| 命令清单审计 | 源码 `PI_GROK_NATIVE_COMMANDS` 41/41 均出现在中英文功能矩阵 |
| stock tutorial 正文 | `docs/tutorial/*` 零 diff |
| source-identity exact seams | PASS：4 个 renderer/state seam、57 个精确 modified seam；18 个 composition Markdown 均精确 allowlist |
| full architecture verifier | BLOCKED：仓库既有旧 SHA baseline、历史新增/删除文件及既有 `fork`/`voice`/`debug` policy 差异；本次 tutorial seam 检查均 PASS |

## 关键边界

- Pi Core 有意保持精简；Plan、Subagent、后台 Bash、Todo 等是 grok-pi 内置桥接或 Pi 扩展能力，不写成固定 Pi 内核功能。
- `/pi-config` 管理发现、预览、启停、Trust/Policy；安装、移除、更新 Pi Packages 仍使用 Pi CLI。
- Remote TUI 默认启用但仍是实验兼容层；raw terminal hook 仍不支持。
- Tree navigation 是 Pi 非破坏性分支导航；文件回滚是可选的 file-only preimage 机制；两者都不是 stock Grok rewind。

## Status 更新日志

- **2026-07-25**：第一版仅放行命令，错误沿用 stock Grok 文案。
- **2026-07-25**：第二版完成九篇 grok-pi 专属教程与 profile seam。
- **2026-07-25**：用户要求重新深度审计；Git/Changelog/Pi 文档确认九篇覆盖不足，状态 → in_progress，计划重组为 18 篇。
- **2026-07-25**：18 篇重组、双语能力矩阵纠偏、Provider/扩展生态补全、精确 verifier 路径与验证完成，状态 → done。
