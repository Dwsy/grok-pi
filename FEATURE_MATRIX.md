# Grok Native TUI × Pi Feature Matrix


**Minimum Pi version: 0.80.10** (system `pi` / `@earendil-works/pi-coding-agent`). `pi-main` is an optional git submodule, not required at runtime.

Status definitions: **Native** = implemented by a Grok Pager component; **Adapted** = Pi semantics converted and projected into a Grok native component; **Boundary** = Pi RPC does not expose it or it is bound to a Grok product backend, deliberately not implemented.

## Terminal and Display

| Feature | Status | Implementation |
|---|---|---|
| Terminal init/restore | Native | Grok `init_terminal` / `restore_terminal` |
| Fullscreen / alternate screen | Native | Grok screen mode; selected at startup |
| Minimal / scrollback-native | Native | `xai-grok-pager-minimal`; selected at startup |
| Welcome / minimal logo | Native+Adapted | Defaults to Welcome (consistent with stock `grok`); `ExternalUiProfile.logo` injects π block art (line-width pad prevents centering drift); only `grok-pi -c/--continue` skips Welcome and goes straight to Resume |
| Welcome menu (Pi) | Native+Adapted | Resume/Ctrl+S ≡ `/resume` (Pi catalog); hides New worktree; Changelog opens `https://github.com/Dwsy/grok-pi/blob/main/CHANGELOG.MD` |
| Welcome session prewarm (Pi) | Adapted | Entering Welcome starts `new_session` in the background; first keystroke attaches the prewarmed agent, avoiding cold-start "Starting session…" |
| Update check/install | Adapted | **GitHub only** `Dwsy/grok-pi` releases JSON + install.sh/ps1; `grok-pi update` / `--check` / Welcome **Ctrl+U**; `GROK_PI_NO_AUTO_UPDATE=1` disables background check |
| Agent Dashboard | Native+Adapted | Native `/dashboard` · Ctrl+\\ · list/peek/dispatch; idle rows projected via `pi/session/list` → `pi/ui/session_catalog` into the dormant roster; not wired to Grok leader FleetView |
| Prompt editing | Native | PromptWidget |
| Multiline / Vim mode | Native | Grok slash/settings |
| Theme / timestamps / mouse | Native+Adapted | Grok appearance/input; Pi theme JSON mapped to Grok `Theme` via `theme::pi`, `/theme` accepts `pi:<name>`; built-in experimental `pi:transparent` (dark) and `pi:transparent-light` themes leave the main canvas to the terminal default background (for terminal opacity/blur) while retaining opaque selection, code, diff, and tool surfaces; F2 controls OSC 9;4 terminal-tab progress, off by default |
| Voice dictation | Native+Adapted | Pager-native `/voice` / Ctrl+Space/F8 writes xAI STT text into the PromptWidget; grok-pi explicitly enables this narrow Pager-owned surface. It uses the local Grok login/API-key credential and does not affect Pi model, session, or prompt ownership. |
| Markdown / code blocks | Native+Adapted | Pi text/reasoning → ACP chunks → `xai-grok-markdown` |
| Tool cards | Native+Adapted | Pi tool events → ACP `ToolCall`; `read`/`bash`/`edit`/`write`/`grep`/`find`/`ls` projected to native cards |
| Todo / plan list | Native+Adapted | Pi `@juicesharp/rpiv-todo` `todo` tool `details.tasks` → ACP `Plan` → native TodoPane/badge; `todo` card suppressed in scrollback |
| Diff rendering | Native+Adapted | edit-like tool metadata enters the Grok tool/diff pipeline |
| Images | Native+Adapted | Pi image blocks → ACP `ImageContent`; actual terminal display depends on Grok/terminal capability |
| Scroll / find / copy / transcript / export | Native | Grok Pager |

## Agent and Streaming Semantics

| Pi feature | Status | Mapping |
|---|---|---|
| Prompt | Adapted | ACP prompt → Pi `prompt` |
| Mid-turn send now | Adapted | Grok `sendNow` → Pi `steer`; queue-line send-now → `x.ai/queue/interject` → steer |
| Follow-up queue | Adapted | default active-turn prompt → Pi `followUp` (only `sendNow`/`followUp:false` goes to steer) |
| Abort | Adapted | ACP cancel → Pi `abort`; uses `abort_bash` for Bash |
| Text stream | Adapted | `message_update` → AgentMessageChunk |
| Thinking/reasoning stream | Adapted | `message_update` → AgentThoughtChunk |
| Tool start/update/end | Adapted | ACP `ToolCall`/`ToolCallUpdate` |
| Pi Bash background task / Send to Background | Native+Adapted | `grok-pi` private Bash extension holds foreground and initial background Bash subprocesses; foreground reuses Pi `createBashToolDefinition` output/rendering semantics. Pager native Send to Background transfers control via `x.ai/terminal/background` using a controlled temp file keyed by `toolCallId` to the **same** subprocess, then projects to the existing `x.ai/task_*` card; `is_background` + `description`, `get_task_output` / `wait_tasks` / `kill_task` remain usable |
| Pi subagents | Native+Adapted | Built-in `pi-grok-subagents` extension owns a Pi child `AgentSession`; versioned bridge projects to native `SubagentBlock`, Tasks Pane, child `AgentView`, and `x.ai/subagent/cancel`. Model-driven end-to-end acceptance is pending |
| Prompt completion | Adapted | uses Pi `agent_settled` as the completion barrier; does not misuse `agent_end` |
| Retry | Adapted | Grok native sticky status/toast |
| Compaction | Native+Adapted | `/compact [instructions]` → Pi `compact`; Pi `compaction_*` → native CompactionStarted/Completed/Failed/Cancelled scrollback blocks + sticky status |
| Session recap (`/recap` + auto away) | Adapted | initialize `meta.sessionRecap`; `x.ai/recap` → inject extension `__pi_grok_recap` (called on `complete` side, does not write session history) → custom `pi-grok-recap/v1` → `SessionRecap`. Uses only the `recap_model` explicitly configured via F2, never falls back to the current session model; auto: ≥3 turns, last completed turn ≥3 minutes old, generated in background while terminal unfocused, not repeated if no new turn after success; manual: any user turn qualifies; input limited to last 6 turns/12k chars; body language prefers macOS `AppleLanguages`, then locale |
| Queue pane / count | Adapted | Pi `queue_update` full-array → `x.ai/queue/changed` (stable id + dequeue) + status; `/queue` panel mirrors Pi steering/follow-up. Pi RPC has no clear/remove/edit, so those ops rebroadcast + toast |
| Context bar used tokens | Adapted | Pi `contextUsage` / message usage → ACP `_meta.totalTokens` → top-right bar |
| Context click / `/context` | Native+Adapted | Grok `x.ai/session/info` → Pi stats + messages + `__pi_context_breakdown` extension (system/tools/AGENTS/append/skills) → native `ModalWindow` reusing `ContextInfoBlock` chart; live refresh while running, not written to scrollback |

## Model, Session, and Commands

| Feature | Status | Notes |
|---|---|---|
| Model catalog | Adapted | `get_available_models` → Grok native model selector; bare `/model` opens the picker directly, active model pinned to top |
| Thinking effort | Adapted | Pi levels → Grok effort selector; xhigh/max normalized for capability |
| New session | Adapted | Grok `/new` → Pi `new_session` |
| Rename | Adapted | Grok `/rename` → Pi `set_session_name` |
| Resume session catalog | Adapted | `/resume` reads Pi JSONL metadata through the headless adapter. Named sessions receive a native `named` badge; expanded Pi rows show CWD/session path, start/update time, model, message count, persisted token total and cost when recorded. Catalog remains ordered by latest activity. |
| Session info / context snapshot | Adapted | Grok `x.ai/session/info` ← Pi stats (used/window/counts) + message estimate + injected extension reading system/tool-defs/AGENTS; on bridge failure system/tools fall back to 0 |
| Session history replay | Adapted | `get_messages` → ACP replay, using Grok scrollback |
| Continue previous session at startup | Adapted | `grok-pi --continue` / `-c` → Pi `--continue` |
| Startup resources, prompts, and session options | Adapted | First-class Pi flags forwarded by `grok-pi`: model (`--provider`/`--model`/`--models`/`--thinking`), session (`--session`/`--session-id`/`--session-dir`/`--fork`/`--no-session`/`--name`), prompts (`--system-prompt`/`--append-system-prompt`), resources (`--extension`/`--no-extensions`/`--no-skills`/`--no-context-files`), tools (`--tools`/`--exclude-tools`/`--no-tools`/`--no-builtin-tools`), trust/network (`--approve`/`--no-approve`/`--offline`); remaining args after `--` still passthrough. `--resume` not exposed (Welcome/`/resume`) |
| Pi extension/prompt/skill commands | Native+Adapted | `get_commands` → Grok slash registry; `source=extension` reaches the Pi command handler directly via private ACP metadata, not the Pager-local or Pi steering/follow-up queue; prompt/skill keep prompt semantics |
| Pi Config resource management | Native+Rust compatible | F2 or `/pi-config` (alias `/pi-resources`) → Pi resources; Rust reads Pi `settings.json`/`trust.json`, managing extensions/skills/prompts/themes across global and trusted-project overrides. Discovers resources by Pi's auto-expansion entry rules; source tree collapsed by default, GitHub/npm/local identities clearly visible, search expands only matching sources. Native two-pane supports tree expand/collapse, search, keyboard paging/scroll, click and wheel; right pane previews package.json key fields and README; after switching prompts restart or Pi `/reload`; does not include `install/remove/update` |
| Grok cloud/session history picker | Boundary | depends on Grok session store; Pi profile does not expose `/history` |
| Pi session tree (`/tree`) | Adapted | Native `SessionTree` modal: filter/search/collapse/detail/copy/tags; Enter/`Shift+Enter` calls `ctx.navigateTree` (can summarize) via injected extension; `session/load` replays; TreeX-style detail panel; does not modify Pi source |
| Pi HTML export / share | Adapted | Grok `/export` stays Markdown transcript; experimental `/pi-export` (HTML or `.jsonl`) and `/pi-share` (private gh gist + pi.dev viewer) hand off to Pi host export-html / share paths via injected extension, no second TUI |

## Extension UI

| Method | Status | Grok component |
|---|---|---|
| `notify` | Native+Adapted | native toast; explicit `info` also appends a native `SystemMessage` scrollback; `/notify` uses a native searchable modal to view all in-process, Pi-session-isolated info/warning/error events (not persisted) |
| `setStatus` | Native+Adapted | sticky banner/status |
| `setWidget` | Native+Adapted | persistent native banner surface |
| `setTitle` | Native+Adapted | terminal title |
| `set_editor_text` | Native+Adapted | PromptWidget |
| `select` | Native+Adapted | QuestionView option list |
| `confirm` | Native+Adapted | QuestionView Yes/No |
| `input` | Native+Adapted | QuestionView freeform PromptWidget |
| `editor` | Native+Adapted | QuestionView multiline PromptWidget |
| timeout/cancel | Adapted | Pi timeout revokes the corresponding QuestionView, returning `cancelled:true` |
| raw terminal hook | Boundary | Pi RPC explicitly does not support it |
| custom header/footer/component | Boundary | Pi RPC explicitly does not support component factory |
| Remote TUI (experimental) | Experimental | `PI_GROK_REMOTE_TUI` default-on: **does not modify Pi source**; injects extension monkey-patch `ctx.ui.custom` + `setWidget` frame projection; keys via temp keyfile; Pager ANSI parsing. Default-on bare `/login`/`/logout` via `pi-grok-auth` (resume-x style); broader `/pi-*` selectors remain opt-in (`PI_GROK_NATIVE_COMMANDS`) |
| `rpiv-ask-user-question` (`custom` questionnaire) | Boundary | depends on non-serializable `ctx.ui.custom(factory)`; RPC stub always declines; experimental Remote TUI may attempt it, but adapting the plugin without changes is still not a stable mapping |
| `rpiv-btw` | Boundary | in-process side model + TUI overlay; should go through native `/btw` + adapter `x.ai/btw` (not yet implemented), not map the juicesharp package |

## Slash Commands

### Retained Grok Native Commands

`exit`, `help`, `new`, `compact`, `model`, `effort`, `rename`, `resume`, `dashboard`, `copy`, `find`, `transcript`, `export`, `expand`, `queue`, `notify`, `multiline`, `compact-mode`, `vim-mode`, `theme`, `timestamps`, `toggle-mouse-reporting`.

### Dynamic Pi Commands

Extension, prompt, and skill commands returned by Pi are not hard-coded in Rust. They enter the Grok native slash suggestion/dropdown through the ACP command catalog; name conflicts are de-duplicated by the Grok registry.

### Deliberately Excluded

Grok product or local session-store commands, including `history`, `login`, `logout`, `usage`, `plugins`, `mcp`, `memory`, `workspace`, `share`, `voice`, `debug`. The original `/minimal` and `/fullscreen` re-exec commands are also not exposed: the renderer remains Grok native, but switching should use startup arguments to avoid losing Pi process arguments.
