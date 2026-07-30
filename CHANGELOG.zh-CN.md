# 更新日志（中文）

**grok-pi**（在 Grok Build 生产级 TUI 中运行 Pi Agent Core）的版本说明。

- 英文完整版（含历史版本）：[CHANGELOG.MD](CHANGELOG.MD)
- 格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)

---

## [0.0.13] - 2026-07-30

范围：`v0.0.12` → `v0.0.13`（2026-07-28 → 2026-07-30）。

### 新增

- **Q&A 桌面通知** — 已启用的原生 `ask_user_question` 在 grok-pi 失焦时抵达，Pager 会尽力发送原生桌面通知。F2 → Agent → **Q&A desktop notifications** 可即时控制，默认开启，且不影响 Q&A 工具准入开关。

### 修复

- **外部 ACP 启动噪声** — Pager 所有的认证管理器不再为 grok-pi 产品隔离的 external profile 记录预期缺失的 Grok 认证文件诊断。

---

## [0.0.12] - 2026-07-28

范围：`v0.0.11` → `v0.0.12`（2026-07-25 → 2026-07-28）。

### 亮点

- **原生 Pi 模型管理中心** — 在 Pager 弹窗内管理 `models.json`，保存后热更新 Pi，无需重启会话。
- **产品导览与 Herdr** — grok-pi 专属 18 篇导览，以及可选启用的原生 Herdr 生命周期桥接。
- **更安全的产品边界** — 仅当 recap 桥接实际加载时才声明该能力；未加载的桥接命令会明确报错。

### 新增

- **`/pi-models`**（别名：`/model-config`、`/models-config`）：原生 Provider → Model → Detail 三栏管理 Pi `models.json`，支持搜索、新建/克隆/编辑/删除、校验、外部修改冲突检测、备份与恢复。保存复用 Pi 官方 reload；激活模型仍走 typed ACP `session/set_model`。
- **grok-pi 教程 profile**：`/tutorial`、`/tour`、`/onboarding` 现在提供 18 篇产品专属内容，覆盖 Pager 原生表面、Pi 能力、可选桥接及边界，不再复用 stock Grok 文案。
- **Herdr 生命周期集成**：F2 中可控制、需重启的 **Pi Herdr integration** 注入宿主拥有的扩展，上报根 Pi 会话身份及 working/blocked/idle 状态；在 Herdr 外无副作用，`[ui].pi_herdr = false` 可关闭。
- **子代理会话隔离**：子代理 session 文件创建在父 session 目录下的 `subagent/` 树中。

### 修复

- **Recap 与桥接命令** — 仅当注入扩展存在时声明 session recap；拒绝调用未加载的桥接命令，并阻止并发 recap 请求。
- **Thinking 流式渲染** — 剥离完整 ANSI 控制序列，并跨 chunk 保留未完成序列，避免终端转义码泄漏到 Thinking 文本或 Rust fence 中。
- **启动噪声** — 不再向 stderr 打印成功的 Pi host 版本检查。

### 变更

- 依照“先 changelog、后隔离同步”的流程整合 Grok Build `47348d1`；保留 Pi-Grok 窄接缝，并为 linked worktree 复用 Cargo target。
- README、功能矩阵、架构记录及中英文 Herdr 使用指南同步说明新产品表面与可选启用策略。

### 说明

- 模型管理中心刻意不伪造 enabled/disabled 状态：模型可用性和认证仍归 Pi 所有。
- Herdr 与 recap 桥接的扩展准入设置变更后，需要完全重启才能生效。

---

## [0.0.11] - 2026-07-25

范围：`v0.0.10` → `v0.0.11`（2026-07-25）。

### 修复

- **发布完整性** — 纳入 bash run-display 集成所需的本地 Pager appearance、settings、router 与 renderer 源码，确保所有发布目标能从 tag checkout 完整编译。

## [0.0.10] - 2026-07-25

范围：`v0.0.9` → `v0.0.10`（2026-07-24 → 2026-07-25）。

### 修复

- **会话替换崩溃** — shortcut-manager 不再在会话重载、fork 或切换后，通过延时回调保留失效的 Pi extension context。
- **Pi RPC 诊断** — 完整子进程 stderr 追加写入 `$GROK_HOME/logs/pi-rpc-stderr.log`，终端错误表面窄时仍保留未裁切的 Node stack trace。

## [0.0.9] - 2026-07-24

范围：`v0.0.8` → `v0.0.9`（2026-07-22 → 2026-07-24）。

### 亮点

- **透明主题波浪 accent 恢复** — 工具运行 / Thinking 左侧 `┃` 呼吸动画在 `pi:transparent` 等主题下不再冻成静态色
- **会话表面** — Context 缓存图、`/review-session` / `/review-message`、会话树地图
- **原生桥接（F2，多数默认关）** — 原生问答 QuestionView、`/btw`、`/loop` 调度
- **Adapter 对齐** — 每条 ACP 通知打 `promptId`；bash/Execute 中途 `output_delta` 流式输出
- **上游** — 合并 Grok Build `a5727c5` 并保留 Pi-Grok 窄接缝；合并后丢失接缝已回补
- **Windows / 多架构安装** — 可靠解析 Pi host shim；安装与 Release 覆盖 macOS / Linux / Windows 的 x86_64 + aarch64

### 新增

#### Context、Review、树

- Context 弹窗 **缓存图**（F2 `[ui].pi_cache_graph`，默认 **开**）：adapter 从 Pi `get_entries` 投影 `cacheMetrics`；视图 `0/1/2/3`，`s` 排序，`e` 导出，`r` 刷新 — 不走 `ctx.ui.custom`
- **`/review-session`**、**`/review-message`**：原生 Pager 审查弹窗（文件列表 + BlockViewer diff）；F2 `review_file_tree` 默认 **关**；弹窗内 `t` 切换树形
- 会话 **树地图** 表面，便于分支方位（与既有 Session Tree 导航并存）

#### 扩展桥接（F2 / 注入，多为可选）

- **原生问答** — F2 `[ui].pi_ask_user_question`（默认 **关**，需重启）：`ask_user_question` → `x.ai/ask_user_question` → 原生 QuestionView；控制目录回写答案。冲突包见 `assets/native_feature_conflicts.toml`（可用 `$GROK_HOME` / 项目目录覆盖）
- **`/btw`** — F2 `pi_btw`（默认 **关**）：旁路提问经 adapter `x.ai/btw` + `pi-grok-btw`（不映射 juicesharp 覆盖层）
- **`/loop` 调度** — F2 `[ui].pi_loop`（默认 **关**，需重启）：`scheduler_create` / `delete` / `list` → 原生 `ScheduledTask*` / tasks pane；仅会话内（无持久 loop 子代理）
- Slash **`getArgumentCompletions`** 桥接：扩展命令（如 `/gapp`）可填充 Grok 参数下拉；`/model` 补全与 Pi `provider/id` 对齐
- 实验性 **rust-tui bridge**（本 tag 仅注释清理）；shortcut-manager / remote-tui 快照归档至 `extensions/_archived/`

#### Adapter / 队列 / 工具流式

- 每条 live ACP **`SessionNotification._meta` 打上客户端 `promptId`**，Pager 的 prompt-id gate 与 turn 铬条与 stock Grok shell 一致
- 主 `session/prompt` 时 **固定 `runningPromptId`**（`QueueMirror::set_running`）；在首个 Pi 事件前再广播，便于队列 adoption
- Pi 递增全文 **`partialResult` → `BashOutput.output_delta`**，Run/bash 卡片中途流式刷新，而非仅结束时跳变

#### 资源、遥测、网站

- 项目级 **resource policy** 与崩溃自愈报告路径
- **`tools/ext-crash-telemetry`**：扩展崩溃上报 CLI + Cloudflare Worker + dashboard（可选运维工具）
- 网站：**静态导出** 部署 GitHub Pages；`basePath` 下 `/docs` 链接可用；中英文档字典扩充

#### 平台

- Windows：将裸 `pi` / `pi.cmd` 解析为绝对路径（PATH + pi-node/npm）；经 `cmd.exe` 拉起 `.cmd`；版本探测后回写 `args.pi_bin`
- 安装与 Release：macOS / Linux / Windows × x86_64 + aarch64

#### 上游

- 合并 Grok Build **`a5727c5`**；写入 `docs/upstream/UPSTREAM_CHANGELOG.md`；验证后更新 AGENTS `base`
- 合并后 **窄接缝回补**（render / effects / shortcuts / shell ops 等）

### 修复

#### 透明主题波浪 accent（用户可见回归）

- **根因：** 透明 / 终端原生主题将 `Theme.bg_base` 设为 `Color::Reset`。运行中 accent 调用 `blend_color(bg, accent, wave_brightness)`；旧实现对 `Reset` 返回 `None`，调用方 `unwrap_or(accent)` → **每帧同一实色**（主观「完全没有呼吸」）
- **修复：** `blend_color` 仅在插值时将 `Reset` 映射为合成深色 canvas `(0x12, 0x12, 0x18)`（页面仍透明，不强制铺不透明底）。命名 ANSI 色仍不可 blend
- **回归测试：** `test_blend_color_reset_base_keeps_wave`
- **附带：** `EntryRenderer` 在 `entry.is_running` 时，即使 block `accent()` 为 `None`（Collapsed 默认）也强制 `accent_running` 动画

#### 其他

- Resume：全文搜索、fork 树、预览模式、快捷键提示
- `a5727c5` 整合后的接缝回补
- GH Pages `basePath` 下文档链接
- rust-tui-bridge 注释噪声清理

### 变更

- FEATURE_MATRIX / README（中英）与 session tree、review、queue、问答、btw、loop、cache graph、notify 行为对齐
- 多行 info 通知优先 **scrollback `SystemMessage`**（对齐 Pi `showStatus`，避免仅 toast 丢失）
- 文档启动路径简化为 **`grok-pi` / `pi-grok`**
- `.gitignore`：本地 fabric mesh 运行态
- 上游流程：先 changelog，再隔离 merge + 窄接缝 reapply

### 说明

- 依赖注入扩展的 F2（**ask-user / btw / loop / workflows / goal**）开关后需 **完全退出并重启**
- 透明主题：波浪仅用合成 canvas 做明度调制，UI 仍保持宿主透明
- 排查笔记（可选）：`docs/investigation/breathing-animation-debug.md`
- 自 **0.0.8** 升级：无额外迁移；透明主题用户无需换主题即可恢复呼吸
- GitHub Release 说明默认仍从 **0.0.6** 起累计章节（`scripts/extract-changelog-section.py`）

---

## 更早版本

`0.0.8` 及更早的完整英文条目见 [CHANGELOG.MD](CHANGELOG.MD)。
