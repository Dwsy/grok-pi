---
id: "2026-07-31-EditTool 并排 Diff 渲染"
title: "EditTool 并排 Diff 渲染"
status: "in_progress"
created: "2026-07-31"
updated: "2026-07-31"
category: "pager"
tags: ["pager", "edit", "diff", "grok-pi"]
---

# Issue: EditTool 并排 Diff 渲染

## Goal

为 grok-pi 增加一个 external-only F2 开关 **Side-by-side edit diffs**。开关默认关闭；开启且可用宽度足够时，展开 EditTool 与普通全屏 Edit viewer 用 old/new 两列显示 diff，并保留窄布局的原生 unified 回退和现有 patch 复制语义。

## 边界

- Pi/ACP/adapter 已提供 edit old/new 数据，本改动只处理 Pager 显示与设置接缝。
- 不把并排实现继续堆进上游 `EditToolCallBlock`；使用 sibling renderer 模块。
- 不新增第二套 TUI、事件循环、scrollback pipeline 或 adapter renderer。
- F2 值为进程内 `AtomicBool`，默认关闭，不扩展 shell `UiConfig`/TOML schema，也不产生持久化 effect。
- code-review viewer 始终保留 unified 双行号布局。

## 验收标准

- [x] F2 中仅 external agent 显示 `side_by_side_edit`，默认关闭，可即时切换和 reset。
- [x] 独立 `side_by_side_edit.rs` 负责 old/new 配对与双列组合；`edit.rs` 只保留窄分派、共享 helper 可见性和输出 source 元数据。
- [x] 开启且两侧内容区均达到最小宽度时显示并排；否则自动调用原生 unified renderer。
- [x] 删除侧显示 `-`，新增侧显示 `+`；Equal 行双侧显示；不等长 Delete/Insert run 保持稳定配对。
- [x] 普通 fullscreen Edit viewer 使用实际内容宽度，并在宽度、主题或 F2 值变化时重建；review viewer 强制 unified 双 gutter。
- [x] 两侧复用现有语法高亮；FileScoped 高亮 map 只覆盖 new side；hunk 分隔、wrap 与 span 背景继续走 Pager 类型。
- [x] `DiffLineOutput.source` 与 paired viewer metadata 保留 patch 顺序，对 Equal 去重。
- [x] verifier 精确区分修改 seam 与新增 sibling renderer，并将新文件加入 focused renderer manifest。
- [ ] Cargo build/test：按本轮用户要求未运行。

## 实现

### Renderer

`crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/side_by_side_edit.rs`

- 按 hunk 扫描 Equal 与 Delete/Insert run。
- run 内按索引配对，不等长侧用空 cell 补齐。
- 独立维护 old/new syntect highlighter；new side 可应用 FileScoped spans。
- 计算行号、marker、divider 和每侧内容宽度；任一内容列小于最小值即返回 `None`，由调用方 unified fallback。
- 输出仍为原生 `DiffLineOutput`，未创建新的 Block/Widget 类型。

### Native hook

`edit.rs` 恢复上游主体后只增加：

- sibling renderer 分派；
- `render_unified_diff_lines()` 显式原生路径；
- 少量 helper 的 `pub(super)` 可见性；
- 每个输出行的 old/new source pair，供 fullscreen patch copy 使用。

### F2

- `SettingOwner::Pager`、`external_only: true`、默认常量 `false`。
- `Action::SetSideBySideEdit` → setter → process-local cache。
- 不返回 `Effect::PersistSetting`。
- registry、bool action、reset/rollback 和 Pager-owned 默认契约测试均有对应 arm。

### Fullscreen viewer

- 普通 Edit viewer 调 `render_diff_lines()`，传入当前 `content_area.width`。
- review viewer 调 `render_unified_diff_lines()`，保留 dual line numbers。
- paired metadata 展平为 Delete → Insert → Equal patch 顺序；wrapped continuation 不重复 source。

## 静态验证

- [x] 直接 `rustfmt --edition 2024` 格式化本任务 Rust 文件。
- [x] `rustfmt --check` 通过。
- [x] `git diff --check` 通过。
- [x] `python3 -m py_compile verify_native_grok.py` 通过。
- [x] 两个 verifier JSON manifest 可被 Python `json` 解析。
- [x] `message_animation_renderer_seams_are_declared`：7 个修改 seam + 1 个新增 renderer seam，PASS。
- [x] `modified_surface_is_exact_and_semantic` 与 `no_fallback_or_old_custom_tui`：PASS。
- [ ] 全量 verifier：仓库既有上传基线缺失/哈希漂移、renderer 非本任务文件漂移及 slash/ACP contract stale 检查仍失败；未把这些无关 blocker 伪装为本功能通过。
- [ ] Rust 编译、单元测试和二进制检查未运行，遵守“不要使用 Cargo”的要求。

## 工作树说明

仓库包含其他既有 WIP。这里只修改本 Issue 所需的 renderer、F2 设置链路、viewer metadata、精确 verifier 声明和中英文文档；未恢复或重写其他 WIP。

## 相关文件

- `crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/side_by_side_edit.rs`
- `crates/codegen/xai-grok-pager/src/scrollback/blocks/tool/edit.rs`
- `crates/codegen/xai-grok-pager/src/views/block_viewer.rs`
- `crates/codegen/xai-grok-pager-render/src/appearance/cache.rs`
- `crates/codegen/xai-grok-pager/src/settings/defs.rs`
- `crates/codegen/pi-grok-adapter/scripts/verify_native_grok.py`
- `FEATURE_MATRIX.md`
- `NATIVE_GROK_TUI_ALIGNMENT.md`

---

## Status 更新日志

- **2026-07-31**: 状态变更 → in_progress；完成调用链与原生 unified renderer 分析。
- **2026-07-31**: 按新约束重构为独立 sibling renderer；新增 external-only、默认关闭的 F2 开关，并完成纯静态检查。
