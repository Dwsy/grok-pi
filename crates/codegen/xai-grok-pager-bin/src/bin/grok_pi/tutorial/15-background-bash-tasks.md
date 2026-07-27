# Background Bash & Tasks

grok-pi's built-in Bash bridge keeps Pi's tool semantics while adding native
background-task control.

- Start Bash normally, then press `Ctrl+B` to send the same subprocess to the
  background instead of launching a replacement command.
- `Ctrl+G` opens the native Tasks pane; task cards show status and output.
- The extension also exposes `get_task_output`, `wait_tasks` and `kill_task` to
  Pi when long-running work needs programmatic control.
- Native task-card kill and tool-level kill share a process-scoped control
  channel keyed by the tool call id.
- A failed background command can deliver its command, output and exit status as
  a follow-up so Pi can recover; successful completion stays quiet.

Output limits, timeouts and process-tree cleanup remain enforced. Backgrounding
changes ownership of display and control, not the underlying process.
