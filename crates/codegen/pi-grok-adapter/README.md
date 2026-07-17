# pi-grok-adapter 0.4

`pi-grok-adapter` 是一个 **headless、library-only** 的 Pi JSONL RPC ↔ ACP 适配器。它不创建 terminal，不读取键盘，不调用 Ratatui/Crossterm，也不渲染任何字符界面。

实际可执行程序位于：

```text
crates/codegen/xai-grok-pager-bin/src/bin/grok-pi.rs
```

该二进制在 Grok Build 的生产 composition package 内启动 Pi RPC，然后把 ACP channel 交给：

```rust
xai_grok_pager::app::run_external(...)
```

因此 UI 由 `xai-grok-pager`、`xai-grok-pager-minimal` 和 `xai-grok-markdown` 原生实现。

## 适配职责

- 启动并监管 `pi --mode rpc`；
- `get_state`、`get_available_models`、`get_commands`、`get_messages` bootstrap；
- Pi model/thinking level ↔ Grok model/effort；
- Pi prompt/steer/follow-up/Bash ↔ ACP prompt；
- Pi text/reasoning/tool/history/image ↔ ACP SessionUpdate；
- Pi queue/compaction/retry/session-name 状态 ↔ Grok 原生状态/标题；
- Pi Extension UI ↔ Grok toast/banner/PromptWidget/QuestionView；
- Pi `agent_settled` ↔ ACP prompt completion barrier。

## 明确不负责

- terminal 初始化与恢复；
- alternate screen/minimal/fullscreen renderer；
- input event loop；
- PromptWidget；
- slash dropdown/palette；
- Markdown、代码、diff、tool card、image 与 scrollback renderer；
- theme、鼠标、Vim/multiline 模式；
-任何 adapter-specific slash UI。

## 验证

```bash
python3 scripts/verify_native_grok.py \
  --workspace ../../.. \
  --pi-source ../../../../pi-main
```

完整交付请从仓库上层运行 `./verify.sh`。
