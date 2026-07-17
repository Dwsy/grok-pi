# Grok Native TUI × Pi 功能矩阵

状态定义：**原生**＝由 Grok Pager 组件实现；**适配**＝Pi 语义转换后进入 Grok 原生组件；**边界**＝Pi RPC 未暴露或与 Grok 产品后端绑定，刻意不实现。

## 终端与显示

| 功能 | 状态 | 实现 |
|---|---|---|
| Terminal init/restore | 原生 | Grok `init_terminal` / `restore_terminal` |
| Fullscreen / alternate screen | 原生 | Grok screen mode；启动时选择 |
| Minimal / scrollback-native | 原生 | `xai-grok-pager-minimal`；启动时选择 |
| Welcome / minimal logo | 原生+适配 | 默认进 Welcome（与 stock `grok` 一致）；`ExternalUiProfile.logo` 注入 π block art（行宽 pad 防居中错位）；仅 `grok-pi -c/--continue` 跳过 Welcome 直接 Resume |
| Welcome 菜单（Pi） | 原生+适配 | Resume/Ctrl+S ≡ `/resume`（Pi catalog）；隐藏 New worktree；Changelog 打开 `https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD` |
| Welcome session 预热（Pi） | 适配 | 进入 Welcome 即后台 `new_session`；首字输入 attach 预热 agent，避免冷启动 “Starting session…” |
| 更新检查/安装 | 适配 | 源：① GitHub `Dwsy/grok-pi` releases JSON ② scoped npm `@dwsy/grok-pi`（**不用**无 scope 的 `grok-pi`，那是别人的 1.0.x 包）。路径：`grok-pi update` / `--check` / Welcome **Ctrl+U**；`GROK_PI_NO_AUTO_UPDATE=1` 关后台检查 |
| Agent Dashboard | 原生+适配 | 原生 `/dashboard` · Ctrl+\\ · 列表/peek/dispatch；idle 行经 `pi/session/list` → `pi/ui/session_catalog` 投影到 dormant roster；不接 Grok leader FleetView |
| Prompt editing | 原生 | PromptWidget |
| Multiline / Vim mode | 原生 | Grok slash/settings |
| Theme / timestamps / mouse | 原生+适配 | Grok appearance/input；Pi 主题 JSON 经 `theme::pi` 映射为 Grok `Theme`，`/theme` 可选 `pi:<name>` |
| Markdown / code blocks | 原生+适配 | Pi text/reasoning → ACP chunks → `xai-grok-markdown` |
| Tool cards | 原生+适配 | Pi tool events → ACP ToolCall；`read`/`bash`/`edit`/`write`/`grep`/`find`/`ls` 投影到原生卡 |
| Todo / plan list | 原生+适配 | Pi `@juicesharp/rpiv-todo` 的 `todo` tool `details.tasks` → ACP `Plan` → 原生 TodoPane/badge；scrollback 抑制 `todo` 卡 |
| Diff rendering | 原生+适配 | edit-like tool metadata进入 Grok tool/diff pipeline |
| Images | 原生+适配 | Pi image blocks → ACP ImageContent；具体终端显示取决于 Grok/terminal 能力 |
| Scroll / find / copy / transcript / export | 原生 | Grok Pager |

## Agent 与流式语义

| Pi 功能 | 状态 | 映射 |
|---|---|---|
| Prompt | 适配 | ACP prompt → Pi `prompt` |
| Mid-turn send now | 适配 | Grok `sendNow` → Pi `steer`；队列行 send-now → `x.ai/queue/interject` → steer |
| Follow-up queue | 适配 | 默认 active-turn prompt → Pi `followUp`（`sendNow`/`followUp:false` 才走 steer） |
| Abort | 适配 | ACP cancel → Pi `abort`；Bash 时用 `abort_bash` |
| Text stream | 适配 | `message_update` → AgentMessageChunk |
| Thinking/reasoning stream | 适配 | `message_update` → AgentThoughtChunk |
| Tool start/update/end | 适配 | ACP ToolCall/ToolCallUpdate |
| Prompt completion | 适配 | 以 Pi `agent_settled` 为完成屏障，不错误使用 `agent_end` |
| Retry | 适配 | Grok native sticky status/toast |
| Compaction | 适配 | `/compact [instructions]` → Pi `compact` |
| Queue pane / count | 适配 | Pi `queue_update` 全文数组 → `x.ai/queue/changed`（稳定 id + 出队）+ status；`/queue` 面板镜像 Pi steering/follow-up。Pi RPC 无 clear/remove/edit，对应操作 rebroadcast + toast |
| Context bar used tokens | 适配 | Pi `contextUsage` / message usage → ACP `_meta.totalTokens` → 右上角 bar |
| Context click / `/context` | 适配 | Grok `x.ai/session/info` → Pi `get_session_stats` + `get_messages` 估算 breakdown → 原生 `ContextInfoBlock` |

## Model、session 与命令

| 功能 | 状态 | 说明 |
|---|---|---|
| Model catalog | 适配 | `get_available_models` → Grok native model selector |
| Thinking effort | 适配 | Pi levels → Grok effort selector；xhigh/max 做能力归一化 |
| New session | 适配 | Grok `/new` → Pi `new_session` |
| Rename | 适配 | Grok `/rename` → Pi `set_session_name` |
| Session info / context snapshot | 适配 | Grok `x.ai/session/info` ← Pi stats（used/window/counts）+ message 字符估算；system/tool-defs 因 RPC 未暴露记 0 |
| Session history replay | 适配 | `get_messages` → ACP replay，使用 Grok scrollback |
| 启动时继续上一会话 | 适配 | `grok-pi --continue` / `-c` → Pi `--continue` |
| 启动资源、提示词与会话选项 | 适配 | `--system-prompt`、`--append-system-prompt`、`--no-skills`、`--no-context-files`、`--extension`、`--no-extensions`、`--no-tools`、`--no-session` 与 `--name` 由 `grok-pi` 转发给 Pi |
| Pi extension/prompt/skill commands | 原生+适配 | `get_commands` → Grok slash registry |
| Grok cloud/session history picker | 边界 | 依赖 Grok session store，Pi profile 不暴露 `/history` |
| Pi tree/fork/switch UI | 边界 | 当前不新增自定义命令/UI；可由 Pi 动态扩展命令自行提供 |
| Pi HTML export RPC | 边界 | 保留 Grok 原生 transcript `/export`，不另造重复命令 |

## Extension UI

| 方法 | 状态 | Grok 组件 |
|---|---|---|
| `notify` | 原生+适配 | toast |
| `setStatus` | 原生+适配 | sticky banner/status |
| `setWidget` | 原生+适配 | persistent native banner surface |
| `setTitle` | 原生+适配 | terminal title |
| `set_editor_text` | 原生+适配 | PromptWidget |
| `select` | 原生+适配 | QuestionView option list |
| `confirm` | 原生+适配 | QuestionView Yes/No |
| `input` | 原生+适配 | QuestionView freeform PromptWidget |
| `editor` | 原生+适配 | QuestionView multiline PromptWidget |
| timeout/cancel | 适配 | Pi timeout 撤销对应 QuestionView，返回 `cancelled:true` |
| raw terminal hook | 边界 | Pi RPC 明确不支持 |
| custom header/footer/component | 边界 | Pi RPC 明确不支持 component factory |
| `rpiv-ask-user-question` (`custom` 问卷) | 边界 | 依赖不可序列化的 `ctx.ui.custom(factory)`；RPC stub 恒 decline；不改插件无法桥接 |
| `rpiv-btw` | 边界 | 进程内 side model + TUI overlay；应走原生 `/btw` + adapter `x.ai/btw`（尚未实现），不映射 juicesharp 包 |

## 斜杠命令

### 保留的 Grok 原生命令

`exit`、`help`、`new`、`compact`、`model`、`effort`、`rename`、`resume`、`dashboard`、`copy`、`find`、`transcript`、`export`、`expand`、`queue`、`multiline`、`compact-mode`、`vim-mode`、`theme`、`timestamps`、`toggle-mouse-reporting`。

### 动态 Pi 命令

Pi 返回的 extension、prompt 和 skill 命令不硬编码在 Rust 中。它们通过 ACP command catalog 进入 Grok 原生 slash suggestion/dropdown；名称冲突由 Grok registry 去重。

### 刻意排除

Grok 产品或本地 session-store 命令，包括 `history`、`login`、`logout`、`usage`、`plugins`、`mcp`、`memory`、`workspace`、`share`、`voice`、`debug`。同时不暴露原版 `/minimal`、`/fullscreen` re-exec 命令：renderer 仍是 Grok 原生，但切换应使用启动参数，避免丢失 Pi 进程参数。
