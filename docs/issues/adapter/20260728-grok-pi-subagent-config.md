---
id: "2026-07-28-grok-pi-subagent-config"
title: "grok-pi 子代理配置（内置代理、资源选择、消息、工具、模型与回合上限）"
status: "implemented"
created: "2026-07-28"
updated: "2026-07-28"
category: "adapter"
tags: ["subagents", "pi-grok", "extension", "configuration"]
---

# Issue: grok-pi 子代理配置

## Goal

让 grok-pi 的 Pi 子代理拥有可配置的内置、项目级与全局 Markdown 定义，并在**既有原生 Grok Pager** 内完成编辑：可从 Pi 取得工具目录、分组勾选内置/插件工具、最多选三个已可用模型、在既有 Pi 资源管理器中选择扩展与技能、控制可见性与最大回合数，并可向运行中的子代理发送 follow-up 或 steer 消息。

语义参考本机 `@tintinweb/pi-subagents`，但不引入它的字符 UI 或改动 Pi 源码。

## 边界

| 层 | 职责 |
|---|---|
| Grok 原有子代理代码 | **只读**；继续负责既有原生卡片、面板与活动投影 |
| Pager 的 grok-pi 外部 profile seam | 仅挂接独立的原生配置入口/回调；资源选择复用既有 Pi Resource Manager 的选择模式，不改变原有 Grok 子代理实现 |
| Pi Core | 唯一的模型、工具、会话与 agent loop 所有者 |
| `pi-grok-adapter` | 只把 Pi 已暴露的工具/模型 catalog 和配置回调投影给 Pager；不绘制 UI |
| `extensions/pi-grok-subagents` | 配置扫描、派生子会话、工具过滤与软回合中断；不绘制 UI |

禁止：复制 pi-subagents 的 TUI、修改 Pi 源码/RPC、修改 Grok 原有子代理实现、在 adapter 中使用终端组件。

## Configuration contract

### Discovery

| Scope | Directory | Priority |
|---|---|---|
| Project | `<cwd>/.grok-pi/agents/*.md` | 1 |
| Global | `$GROK_HOME/agents/*.md` | 2 |
| Built-in | `general-purpose`、`explore`、`plan` | 3 |

同名项目定义覆盖全局定义；项目层的禁用条目可遮蔽同名全局条目。内置条目始终显示；选择内置条目可创建项目或全局覆盖。全局内置覆盖可恢复内置默认值，项目内置覆盖可移除并恢复其继承定义；两者都只删除覆盖 Markdown，不修改 Pi 或 Grok 原有子代理代码。

### Markdown frontmatter

`description`, `tools`, `models`, `extensions`, `skills`, `max_turns` 和 `enabled` 均为可选。`enabled: false` 是唯一的关闭开关：全局文件关闭全局条目，项目文件关闭项目条目或同名全局条目。未写 `enabled` 默认启用。

`tools` 是由 Pi 运行时工具目录验证的选择列表：Pager 必须分为 Pi built-in 与 extension/plugin 两组显示。显式 `tools: []` 禁用全部工具，未配置时沿用 profile 默认。`models` 为按 Pi catalog 回调选择的已可用模型，最多三个；未配置时继承父会话模型。`extensions` 与 `skills` 通过既有 `/pi-config` Pi 资源管理器的选择模式挑选：沿用其发现、信任、全局/项目页、搜索、预览和折叠，选择不会改写 Pi 的资源启用设置。`extensions` 也包含 grok-pi 注入扩展；`max_turns: 0`/缺省为不限。

运行期间可用 `/subagent-message [subagent-id] [message]` 或 `send_message_to_subagent`。`follow_up` 在当前回合后排队，`steer` 立即打断当前回合；两者仅面向仍在运行的 Pi 子会话。

## Slices

| ID | 内容 | 验收 |
|---|---|---|
| S0 | 本 Issue、引用实现与现有 seam 盘点 | 本文 |
| S1 | 扩展配置扫描、frontmatter、项目/全局优先级和禁用遮蔽 | 扩展单测/静态测试 |
| S2 | Pi tool/model/extension/skill catalog bridge；Pager 原生配置表单分组与多选（模型≤3） | adapter + Pager 窄测 |
| S3 | 新建/编辑项目与全局定义，安全原子写入 Markdown | 配置 round-trip 测试 |
| S4 | 模型调度与工具过滤；超回合发送一次“结束并总结”软提醒 | extension 测试 |
| S5 | grok-pi 注入、Feature Matrix/中英文 README 与手测清单 | binary check + 文档复核 |
| S6 | 内置代理覆盖/恢复、Pi 资源管理器选择模式、运行中消息 | narrow tests + binary check |

## Acceptance

- [x] A1 不修改 Grok 原有子代理源文件；继续复用其卡片、活动面板和生命周期。
- [x] A2 项目/全局 `.md` 条目可发现；项目同名定义优先，项目 `enabled: false` 可关闭同名全局条目。
- [x] A3 全局 `enabled: false` 只关闭该全局条目；项目 `enabled: false` 只关闭该项目条目或遮蔽同名全局。
- [x] A4 TUI 从 Pi 实时 catalog 获取工具，按内置与插件两组勾选。
- [x] A5 TUI 从 Pi 模型 callback 获取已可用模型，最多三个；仅提供 grok-pi 兼容/可用模型。
- [x] A6 TUI 复用既有 Pi 资源管理器选择要加载的 Pi/grok-pi extensions 与 Pi 发现到的 skills；子代理实际按配置加载，且不改 Pi 资源启用设置。
- [x] A7 子代理工具实际受配置约束；未配置时沿用默认行为。
- [x] A8 `max_turns` 到限时 extension 向子代理注入一次结束/总结提醒，不粗暴中止正在执行的工具。
- [x] A9 配置文件写入产品隔离路径，不读取或写入 stock `.grok` / `~/.grok`。
- [x] A10 窄测、编译检查、diff 审查通过；文档描述与行为一致。
- [x] A11 内置 `general-purpose`、`explore`、`plan` 均可见；可建立项目/全局覆盖且可恢复内置默认值。
- [x] A12 可向运行中的子代理发送 follow-up 或 steer 消息；已完成的子代理会被拒绝。

## Progress

- [x] S0 Issue + 参考扩展能力盘点
- [x] S1 项目/全局配置扫描与禁用语义
- [x] S2 catalog bridge + 原生配置 UI
- [x] S3 Markdown 写入/编辑
- [x] S4 运行时工具/模型/软回合上限
- [x] S5 注入、文档与验证
- [x] S6 内置覆盖、资源管理器选择模式、运行中消息

## Validation

- `pi --version` → `0.82.1`; a real RPC `get_commands` probe loaded the
  extension and returned both `subagents` and `subagent-message`.
- `cargo test -p pi-grok-adapter product_multi_select_envelope_uses_native_checkbox_answer_shape` → pass.
- `cargo test -p pi-grok-adapter product_resource_picker_envelope_round_trips_selected_paths` → pass.
- `cargo test -p xai-grok-pager-bin --bin grok-pi subagent_extension_source_is_a_loadable_typescript_module` → pass.
- `cargo check -p xai-grok-pager -p pi-grok-adapter`, the embedded-extension test above, and `git diff --check` → pass (existing warnings remain).
- The local `tsgo` config reports no error from `extensions/pi-grok-subagents`.
  The full command still exits nonzero on three unrelated `pi-main` diagnostics:
  stale NVIDIA/Vercel model-catalog assertions and a missing `highlight.js`
  declaration. A real Pi RPC extension-load probe succeeds despite these.
- A real-model native Pager interaction (select/save/spawn/turn-limit) remains
  a manual acceptance item; it was not represented as an automated pass.
