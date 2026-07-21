---
id: "2026-07-20-grok-pi-recap-mermaid"
title: "grok-pi Recap Mermaid 注入"
status: "completed"
created: "2026-07-20"
updated: "2026-07-20"
category: "adapter"
tags: ["workhub", "recap", "mermaid", "settings"]
---

# Issue: grok-pi Recap Mermaid 注入

## Goal

增加一个默认关闭的 F2 `recap_mermaid` 开关。开启后，Recap 生成提示词允许模型在有结构价值时输出 Markdown Mermaid 图；关闭时保持现有纯文本 Recap 行为。

## Boundaries

- Pager 继续使用原生 Markdown/Mermaid 渲染，不新增第二套 renderer。
- Pi 继续通过注入的 `__pi_grok_recap` extension 生成 display-only 内容，不修改 Pi 源码。
- adapter 只透传 `recapMermaid` 参数，不渲染内容。

## Implementation

- `[ui].recap_mermaid`：默认 `false`，F2 可动态修改并持久化。
- `Action::SendRecap` → `Effect::SendRecap` → `x.ai/recap` 携带 `recapMermaid`。
- adapter 转发到 Pi extension；extension 在提示词中允许最多 8 节点的 Mermaid 图。
- Recap 展开态改用现有 `MarkdownContent`，因此 ```` ```mermaid ```` 进入既有 Mermaid 渲染链；折叠态仍显示首行预览。

## Acceptance

- [x] 默认关闭，不影响原有 Recap 纯文本提示词。
- [x] F2 设置实时生效并持久化。
- [x] Pager → adapter → Pi extension 参数链路完整。
- [x] 展开 Recap 使用原生 Markdown/Mermaid 渲染。
- [x] `cargo check -p xai-grok-pager-bin --bin grok-pi` PASS。
- [x] `cargo test -p pi-grok-adapter recap_bridge` PASS（4）。
- [x] `git diff --check` PASS。

## Known limits

- Mermaid 是否输出由模型根据提示词判断；开关只允许生成，不强制每次生成图。
- 未进行真实终端手测，需后续确认折叠/展开交互与复杂 Mermaid 图在目标终端的视觉效果。
