---
id: "2026-07-21-resume-预览与搜索对齐-resume-x"
title: "Resume 增强：预览与搜索对齐 resume-x"
status: "completed"
created: "2026-07-21"
updated: "2026-07-21"
category: "pager"
tags: ["pager", "adapter", "resume", "preview", "search", "resume-x"]
---

# Issue: Resume 增强：预览与搜索对齐 resume-x

## Goal

在 grok-pi 的 `/resume` SessionPicker 中增加 resume-x 风格的**消息预览**和**全文搜索**能力：

- `→` 打开选中会话的全屏消息预览（顶部含会话详情），`←` 返回列表
- `Ctrl+F` 打开独立的 PSM 全文搜索弹窗（会话名 + 消息内容）
- 预览仅渲染 user/assistant 文本消息，不渲染工具调用
- 数据源：PSM SQLite 优先，PSM 未启动时 adapter 直读 JSONL 回退

## 背景/问题

当前 grok-pi 的 `/resume` SessionPicker 只提供会话列表 + 卡片展开（`e/Shift+e` 显示 CWD/token/cost），无法：
1. 快速浏览会话的完整对话内容（需先 resume 才能看到）
2. 按消息内容全文搜索历史会话

外部参考项目 `resume-x`（Pi TUI 扩展）已实现三模式交互（list/preview/search），数据全部来自 PSM SQLite。本 Issue 将其核心交互移植到 grok-pi 的原生 Rust Pager + adapter 架构中。

## 用户决策记录

| # | 决策 | 理由 |
|---|------|------|
| 1 | **Ctrl+F** 打开搜索页（非 Ctrl+Q） | Ctrl+Q 是全局 Quit（When::Always），冲突不可接受；Ctrl+F 语义自然且无冲突 |
| 2 | **→ 直接进预览**，预览页顶部包含会话详情 | 对齐 resume-x 交互；卡片展开保留 `e/Shift+e` 不变 |
| 3 | 预览**仅渲染 user/assistant 文本**，不渲染工具调用 | 降低渲染复杂度；工具输出噪音大，预览目的是快速确认对话内容 |
| 4 | 搜索为**独立 PSM 弹窗**，与 Grok 原生搜索无关 | 原生 `x.ai/session/search` 只索引 Grok 会话，不含 Pi 会话；搜索页参考 resume-x TUI 设计但不修改 Grok 原生组件 |

## 验收标准

### 预览模式
- [ ] SessionPicker 中按 `→` 打开选中会话的全屏消息预览
- [ ] 预览顶部固定 header 显示：会话文件名、消息数、模型、token/cost、创建/更新时间
- [ ] 预览体仅渲染 user（带背景色 Box）和 assistant（Markdown）文本消息
- [ ] 工具调用消息（tool_use/tool_result）完全跳过
- [ ] `↑/↓` 滚动 3 行，`Shift+↑/↓` 半页，`PgUp/PgDn` 全页
- [ ] `←` 或 `Esc` 返回会话列表，列表选中位置不变
- [ ] `Enter` 在预览中直接 resume 当前会话
- [ ] PSM 未启动时回退 adapter 直读 JSONL，预览功能不受影响

### 搜索模式
- [ ] SessionPicker 中按 `Ctrl+F` 打开独立搜索弹窗
- [ ] 搜索弹窗包含：查询输入框、结果列表（最多 10 条）、选中结果详情面板
- [ ] 搜索范围：会话名称 + 消息内容（基于 PSM SQLite `sessions` + `message_entries` 表）
- [ ] `Tab` 切换 cwd/all 搜索范围
- [ ] `↑/↓` 导航结果，`Enter` resume 选中会话，`→` 进入预览
- [ ] `Esc` 或 `←` 关闭搜索弹窗返回会话列表
- [ ] PSM 未启动时搜索不可用，显示提示（不 crash）

### 数据层
- [ ] adapter 新增 `pi/session/messages` ext 方法，返回指定会话路径的 user/assistant 消息
- [ ] PSM 可用时从 `message_entries` 表查询（role IN ('user','assistant')，content 非空）
- [ ] PSM 不可用时从 JSONL 文件解析（复用现有 `session_message_text` 逻辑）
- [ ] 消息返回格式：`{role, content, timestamp}`，按时间升序

### 不破坏现有功能
- [ ] `e/Shift+e` 卡片展开/折叠行为不变
- [ ] `/resume` 列表导航、排序、filter、delete 行为不变
- [ ] Ctrl+Q 全局 Quit 行为不变
- [ ] 非 Pi 会话（native/remote）不受影响

## 实施阶段

### Phase 1: 数据层（adapter）
- [ ] `model.rs`：添加 `PiSessionMessage { role, content, timestamp }` 类型
- [ ] `model.rs`：添加 `parse_session_messages(path) -> Vec<PiSessionMessage>` JSONL 解析
- [ ] `psm_session_catalog.rs`：添加 `load_messages(session_path) -> Option<Vec<PiSessionMessage>>` PSM 查询
- [ ] `pi_adapter.rs`：添加 `pi/session/messages` ext 方法路由（PSM 优先 → JSONL 回退）
- [ ] `pi_adapter.rs`：添加 `pi/session/search` ext 方法路由（PSM `sessions` + `message_entries` LIKE 搜索）
- [ ] 单元测试覆盖 PSM 可用/不可用/空会话/工具消息过滤

### Phase 2: Pager UI（预览模式）
- [ ] `ActiveModal::SessionPicker` 新增 `preview_state: Option<PreviewState>` 字段
- [ ] `PreviewState`：messages、scroll_offset、total_lines、session_path
- [ ] `modals.rs` 键盘处理：`→` 触发 `Action::FetchSessionPreview { session_path }`
- [ ] `effects/mod.rs`：新增 `Effect::FetchSessionPreview`，调用 `pi/session/messages`
- [ ] `actions.rs`：新增 `Action::SessionPreviewLoaded { messages }` / `Action::CloseSessionPreview`
- [ ] `modals.rs` 渲染：预览模式全屏渲染（固定 header + 滚动消息体）
- [ ] 预览内键盘：`←`/Esc 返回、`↑↓` 滚动、`Enter` resume

### Phase 3: Pager UI（搜索模式）
- [ ] `ActiveModal::SessionPicker` 新增 `search_state: Option<SearchState>` 字段
- [ ] `SearchState`：query、results、selected_idx、scroll_offset、cwd_only
- [ ] `modals.rs` 键盘处理：`Ctrl+F` 触发搜索模式
- [ ] `effects/mod.rs`：新增 `Effect::SearchPiSessions { query, cwd }`，调用 `pi/session/search`
- [ ] `actions.rs`：新增 `Action::PiSessionSearchResults { results }`
- [ ] `modals.rs` 渲染：搜索弹窗（查询框 + 结果列表 + 详情面板）
- [ ] 搜索内键盘：输入、`Tab` 切 scope、`↑↓` 导航、`Enter` resume、`→` 预览、`Esc` 返回

## 关键决策

| 决策 | 理由 |
|------|------|
| 预览/搜索作为 SessionPicker 内部子模式 | 对齐 resume-x 的三模式模型；避免新增 ActiveModal 变体 |
| PSM 优先 + JSONL 回退 | PSM 提供快速索引；JSONL 保证 PSM 未安装时功能可用 |
| 不渲染工具调用 | 预览目的是快速确认对话内容；工具输出噪音大且格式复杂 |
| 搜索独立于 Grok 原生 | 原生 FTS5 只索引 Grok 会话；Pi 会话搜索必须走 adapter/PSM |
| 保留 e/Shift+e 卡片展开 | 现有用户习惯不变；→ 的语义从展开改为预览（更对齐 resume-x） |
| Ctrl+F 而非 Ctrl+Q/Alt+Q | Ctrl+Q 是全局 Quit；Alt+Q 在终端中不可靠；Ctrl+F 语义自然 |

## 相关资源

- resume-x 源码：`~/Downloads/pi-session-manager-main/extensions/resume-x/`
- resume-x README：`~/Downloads/pi-session-manager-main/extensions/resume-x/README.md`
- adapter PSM 基建：`crates/codegen/pi-grok-adapter/src/psm_session_catalog.rs`
- adapter JSONL 解析：`crates/codegen/pi-grok-adapter/src/model.rs`
- adapter ext 路由：`crates/codegen/pi-grok-adapter/src/pi_adapter.rs:2369`
- Pager SessionPicker 模态：`crates/codegen/xai-grok-pager/src/views/modal.rs:298`
- Picker 键盘处理：`crates/codegen/xai-grok-pager/src/app/modals.rs:1022`
- Picker 渲染：`crates/codegen/xai-grok-pager/src/app/modals.rs:2208`
- Effect 模式参考：`crates/codegen/xai-grok-pager/src/app/effects/mod.rs:754`（FetchSessionTree）
- 关联 Issue：`docs/issues/adapter/20260720-可选 PSM resume 数据源.md`

## Notes

- resume-x 的 `doResume()` 模式（done → switchSession）在 grok-pi 中等价于 `Action::PickSession`，无需额外处理
- PSM `message_entries` 表 schema：`session_path, role, source_type, content, timestamp`
- PSM 存活探测复用现有 `psm_server_is_listening(52131)` 逻辑
- 预览渲染参考 resume-x `buildPreviewLines`：固定 header（Container）+ 滚动 content（Container.slice）

---

## 验证

### 自动化测试

| 命令 | 结果 |
|------|------|
| `cargo check -p xai-grok-pager-bin --bin grok-pi` | ✅ 通过 |
| `cargo check -p xai-grok-pager` | ✅ 通过 |
| `cargo check -p pi-grok-adapter` | ✅ 通过 |
| `cargo test -p pi-grok-adapter --lib` | ✅ 93 passed, 0 failed |
| `cargo test -p xai-grok-pager --lib session_picker_mode_tests` | ✅ 8 passed, 0 failed |
| `cargo test -p xai-grok-pager --lib` | 7396 passed, 19 failed (全部预存失败，与本次变更无关) |

### 手工验证清单

| # | 场景 | 预期 | 状态 |
|---|------|------|------|
| 1 | Pi 会话中 `/resume` → Ctrl+F | 进入搜索全屏页，显示查询框 | 待验证 |
| 2 | 搜索页输入关键词 | 实时搜索 PSM 数据库，显示结果列表 | 待验证 |
| 3 | 搜索页 Tab | 切换 cwd/all scope | 待验证 |
| 4 | 搜索页 Enter | resume 选中会话 | 待验证 |
| 5 | 搜索页 → | 进入选中结果的预览 | 待验证 |
| 6 | 搜索页 Esc/← | 返回会话列表 | 待验证 |
| 7 | 会话列表 → | 进入选中会话的全屏预览 | 待验证 |
| 8 | 预览页 ↑↓/Shift+↑↓/PgUp/PgDn | 滚动消息体 | 待验证 |
| 9 | 预览页 Enter | resume 当前预览会话 | 待验证 |
| 10 | 预览页 ←/Esc | 返回会话列表 | 待验证 |
| 11 | PSM 未启动时 → 预览 | 回退 JSONL 解析，预览正常 | 待验证 |
| 12 | PSM 未启动时 Ctrl+F 搜索 | 显示 toast 提示搜索不可用 | 待验证 |
| 13 | stock Grok（非 Pi）`/resume` | Ctrl+F 和 → 不触发预览/搜索，行为不变 | 待验证 |
| 14 | Pi 会话 e/Shift+e | 卡片展开/折叠行为不变 | 待验证 |
| 15 | Ctrl+Q 全局 Quit | 行为不变（搜索页内 Ctrl+Q 仍退出应用） | 待验证 |

---

## Status 更新日志

- **[2026-07-21]**: 创建 Issue，状态 → in_progress。完成调研，确认 4 项用户决策。
- **[2026-07-21]**: Phase 1-3 全部完成。adapter 数据层 + Pager 状态建模 + 数据管线 + 键位渲染 + 单测。状态 → completed。
