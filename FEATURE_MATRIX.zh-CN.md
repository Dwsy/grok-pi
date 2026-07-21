# Grok Native TUI × Pi 功能矩阵


**最小 Pi 版本：0.80.10**（系统 `pi` / `@earendil-works/pi-coding-agent`）。`pi-main` 为可选 git 子模块，非运行时必需。

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
| 更新检查/安装 | 适配 | **仅 GitHub** `Dwsy/grok-pi` releases JSON + install.sh/ps1；`grok-pi update` / `--check` / Welcome **Ctrl+U**；`GROK_PI_NO_AUTO_UPDATE=1` 关后台检查 |
| Agent Dashboard | 原生+适配 | 原生 `/dashboard` · Ctrl+\\ · 列表/peek/dispatch；idle 行经 `pi/session/list` → `pi/ui/session_catalog` 投影到 dormant roster；不接 Grok leader FleetView |
| Prompt editing | 原生 | PromptWidget |
| Multiline / Vim mode | 原生 | Grok slash/settings |
| Theme / timestamps / mouse | 原生+适配 | Grok appearance/input；Pi 主题 JSON 经 `theme::pi` 映射为 Grok `Theme`，`/theme` 可选 `pi:<name>`；内置实验性 `pi:transparent`（暗色）与 `pi:transparent-light`（浅色）将主画布交给终端默认背景（用于终端透明度/毛玻璃），同时保留选中态、代码、diff 与工具表面的实色；F2 可控制 OSC 9;4 terminal-tab progress，默认关闭 |
| Markdown / code blocks | 原生+适配 | Pi text/reasoning → ACP chunks → `xai-grok-markdown` |
| Tool cards | 原生+适配 | Pi tool events → ACP ToolCall；`read`/`bash`/`edit`/`write`/`grep`/`find`/`ls` 投影到原生卡 |
| Todo / plan list | 原生+适配 | Pi `@juicesharp/rpiv-todo` 的 `todo` tool `details.tasks` → ACP `Plan` → 原生 TodoPane/badge；scrollback 抑制 `todo` 卡 |
| Plan mode | 原生+适配 | Pager 原生 Plan 开关 → adapter 负责的 `Inactive/Pending/Active/ExitPending` 状态机；full/sparse system-reminder 前缀；session 私有 `.plan.md` sidecar；注入 Pi `tool_call` gate 阻止 `edit`/`write`/`bash`（仅放行计划文件）；Pi `exit_plan_mode` 打开原生 `x.ai/exit_plan_mode` 审批，并持久化 `.plan-mode.json` 状态 |
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
| Pi Bash 后台任务 / Send to Background | 原生+适配 | `grok-pi` 私有 Bash extension 持有前台与初始后台 Bash 子进程；前台仍复用 Pi `createBashToolDefinition` 的输出/渲染语义。Pager 原生 Send to Background 经 `x.ai/terminal/background` 以受控临时控制文件按 `toolCallId` 转交**同一**子进程，随后投影到既有 `x.ai/task_*` 卡片；原生任务卡 kill 经同一控制通道走 `x.ai/task/kill`（`op:kill` + 已发布 `runningTaskIds`）；`is_background` + `description`、`get_task_output` / `wait_tasks` / `kill_task` 保持可用。 |
| Pi 子代理 | 原生+适配 | 内置 `pi-grok-subagents` extension 拥有 Pi child `AgentSession`；版本化 bridge 投影到原生 `SubagentBlock`、Tasks Pane、child `AgentView` 与 `x.ai/subagent/cancel`。模型驱动的手工端到端验收待执行。 |
| Prompt completion | 适配 | 以 Pi `agent_settled` 为完成屏障，不错误使用 `agent_end` |
| Retry | 适配 | Grok native sticky status/toast |
| Compaction | 原生+适配 | `/compact [instructions]` → Pi `compact`；Pi `compaction_*` → 原生 CompactionStarted/Completed/Failed/Cancelled scrollback blocks + sticky status |
| Session recap (`/recap` + auto away) | 适配 | initialize `meta.sessionRecap`；`x.ai/recap` → 注入 extension `__pi_grok_recap`（`complete` 侧调用，不写会话历史）→ custom `pi-grok-recap/v1` → `SessionRecap`。仅使用 F2 显式配置的 `recap_model`，不回退当前会话模型；auto：≥3 turn、最后完成 turn ≥3 分钟、终端失焦期间后台生成、成功后无新 turn 不重复；manual：有 user turn即可；输入限最近 6 turn/12k 字符；正文语言优先 macOS `AppleLanguages`，再回退 locale |
| Queue pane / count | 适配 | Pi `queue_update` 全文数组 → `x.ai/queue/changed`（稳定 id + 出队）+ status；`/queue` 面板镜像 Pi steering/follow-up。Pi RPC 无 clear/remove/edit，对应操作 rebroadcast + toast |
| Context bar used tokens | 适配 | Pi `contextUsage` / message usage → ACP `_meta.totalTokens` → 右上角 bar |
| Context click / `/context` | 原生+适配 | Grok `x.ai/session/info` → Pi stats + messages + `__pi_context_breakdown` extension（system/tools/AGENTS/append/skills）→ 原生 `ModalWindow` 中复用 `ContextInfoBlock` 图表；运行中即时刷新、不写 scrollback |

## Model、session 与命令

| 功能 | 状态 | 说明 |
|---|---|---|
| Model catalog | 适配 | `get_available_models` → Grok native model selector；裸 `/model` 直接打开 picker，当前激活模型置顶 |
| Thinking effort | 适配 | Pi levels → Grok effort selector；xhigh/max 做能力归一化 |
| New session | 适配 | Grok `/new` → Pi `new_session` |
| Rename | 适配 | Grok `/rename` → Pi `set_session_name` |
| Resume session catalog | 适配 | `/resume` 经无界面 adapter 读取 Pi JSONL 元数据。已命名会话显示原生 `named` 标记；展开 Pi 行可显示 CWD/会话路径、开始/更新时间、模型、消息数、已持久化的 token 总数与成本（仅在记录存在时）。目录继续按最近活动时间排序。 |
| Session info / context snapshot | 适配 | 原生 `/session-info`（别名 `/session`，对齐 Pi 命名）→ Grok `x.ai/session/info` ← Pi stats（file/used/window/counts）+ message 估算 + 注入 extension 读 system/tool-defs/AGENTS；bridge 失败时 system/tools 回退 0。展示为 system scrollback（Pi interactive 写 chat）；图表仍用 `/context` modal。 |
| Session history replay | 适配 | `get_messages` → ACP replay，使用 Grok scrollback |
| 启动时继续上一会话 | 适配 | `grok-pi --continue` / `-c` → Pi `--continue` |
| 启动资源、提示词与会话选项 | 适配 | `grok-pi` 一等转发：模型（`--provider`/`--model`/`--models`/`--thinking`）、会话（`--session`/`--session-id`/`--session-dir`/`--fork`/`--no-session`/`--name`）、提示词（`--system-prompt`/`--append-system-prompt`）、资源（`--extension`/`--no-extensions`/`--no-skills`/`--no-context-files`）、工具（`--tools`/`--exclude-tools`/`--no-tools`/`--no-builtin-tools`）、trust/网络（`--approve`/`--no-approve`/`--offline`）；`--` 后参数仍透传。不暴露 `--resume`（Welcome/`/resume`） |
| Pi extension/prompt/skill commands | 原生+适配 | `get_commands` → Grok slash registry；`source=extension` 经私有 ACP metadata 直达 Pi command handler，不进入 Pager 本地或 Pi steering/follow-up 队列；prompt/skill 保持 prompt 语义 |
| Pi Config 资源管理 | 原生+Rust 兼容 | F2 或 `/pi-config`（别名 `/pi-resources`）→ Pi resources；Rust 读取 Pi `settings.json`/`trust.json`，管理 extensions/skills/prompts/themes 的 global 与 trusted-project 覆盖。按 Pi 自动扩展入口规则发现资源；来源树默认折叠，GitHub/npm/local 身份清晰可见，搜索仅展开命中来源。原生双栏支持树展开/折叠、搜索、键盘分页/滚动、点击与滚轮；右栏预览 package.json 关键字段与 README；切换后提示重启或 Pi `/reload`；不含 `install/remove/update`。 |
| Grok cloud/session history picker | 边界 | 依赖 Grok session store，Pi profile 不暴露 `/history` |
| Pi session tree (`/tree`) | 适配 | 原生 `SessionTree` modal：筛选/搜索/折叠/详情/复制/标签；Enter/`Shift+Enter` 经注入 extension 调 `ctx.navigateTree`（可 summarize）；`session/load` 回放；TreeX 风格详情面板；不改 Pi 源码 |
| Pi session fork (`/fork`) | 适配 | External：与 `/jump` 同款 prompt 区 `ListOverlay`（RPC `get_fork_messages`）；选择后 RPC `fork` 生成分支 session 文件，同 agent 换绑新 `sessionId`，`session/load` 回放并把选中文案预填 prompt；非 external 仍走 Grok peer-agent `/fork` |
| Pi session clone (`/clone`) | 适配 | External：RPC `clone` 在当前 leaf 复制新 session 文件；同 agent 换绑新 `sessionId`，`session/load` 回放并清空 prompt（对齐 Pi） |
| Pi 资源重载 (`/reload`) | 适配 | External：`__pi_reload` → `ctx.reload()`；流式 **与** compaction 中禁止（对齐 Pi）；adapter 刷新命令/模型目录；Pager 重扫 Pi theme（`rediscover`）并重应用当前 `pi:*` 主题；loading/成功 toast 文案对齐 Pi；不分支 session 文件 |
| Pi HTML export / share | 适配 | Grok `/export` 仍为 Markdown transcript；实验性 `/pi-export`（HTML 或 `.jsonl`）与 `/pi-share`（私有 gh gist + pi.dev 预览）经注入 extension 交给 Pi host export-html / share 路径，不另造 TUI |

## Extension UI

| 方法 | 状态 | Grok 组件 |
|---|---|---|
| `notify` | 原生+适配 | 原生 toast；显式 `info` 同时追加原生 SystemMessage scrollback；`/notify` 用原生可搜索 modal 查看当前进程内、按 Pi session 隔离的全部 info/warning/error 事件（不持久化） |
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
| Remote TUI（实验） | 实验 | `PI_GROK_REMOTE_TUI` 默认开：**不改 Pi 源码**；npm/Node Pi 通过官方 `rpc-entry.js` 启动，因此仅检查 argv 的第三方 RPC guard 看不到外层 `--mode rpc`；最先注入的兼容扩展仅在 Remote TUI host 活跃时将 `ExtensionRunner` 暴露给扩展的 `ctx.mode` 从 `rpc` 投影为 `tui`。Pi core 与 JSONL transport 仍是真实 RPC。注入 `ctx.ui.custom` host + `setWidget` 帧投影；键经 tmp keyfile；Pager ANSI 解析。裸 `/login`/`/logout` 由 `pi-grok-auth` 默认开启（resume-x 风格）；更广的 `/pi-*` 选择器仍需 `PI_GROK_NATIVE_COMMANDS` |
| `rpiv-ask-user-question` (`custom` 问卷) | 边界 | 依赖不可序列化的 `ctx.ui.custom(factory)`；RPC stub 恒 decline；实验 Remote TUI 可尝试，不改插件仍非稳定适配 |
| `rpiv-btw` | 边界 | 进程内 side model + TUI overlay；应走原生 `/btw` + adapter `x.ai/btw`（尚未实现），不映射 juicesharp 包 |

## 斜杠命令

### 保留的 Grok 原生命令

`exit`、`help`、`hotkeys`（别名 `shortcuts`/`keys`）、`new`、`compact`、`model`、`effort`、`rename`、`resume`、`session-info`（别名 `session`）、`dashboard`、`copy`、`find`、`transcript`、`export`、`expand`、`queue`、`notify`、`multiline`、`compact-mode`、`vim-mode`、`theme`、`timestamps`、`toggle-mouse-reporting`。

### 动态 Pi 命令

Pi 返回的 extension、prompt 和 skill 命令不硬编码在 Rust 中。它们通过 ACP command catalog 进入 Grok 原生 slash suggestion/dropdown；名称冲突由 Grok registry 去重。

### 刻意排除

Grok 产品或本地 session-store 命令，包括 `history`、`login`、`logout`、`usage`、`plugins`、`mcp`、`memory`、`workspace`、`share`、`voice`、`debug`。同时不暴露原版 `/minimal`、`/fullscreen` re-exec 命令：renderer 仍是 Grok 原生，但切换应使用启动参数，避免丢失 Pi 进程参数。
