# Tools, Streaming & Diffs

Pi executes tools while grok-pi projects their lifecycle into native Pager
cards, status updates and diffs.

- The default built-in set is `read`, `bash`, `edit` and `write`.
- F2 can additionally enable Pi's `grep`, `find` and `ls` built-ins.
- CLI policy is authoritative: `--tools`, `--exclude-tools`, `--no-tools` and
  `--no-builtin-tools` override the F2 selection.
- Extension and custom tools remain available unless their own source is
  disabled or blocked by policy.
- Text, reasoning and tool progress stream independently; edit metadata enters
  the native file/diff pipeline instead of printing an unstructured patch.

The tool card can stay compact or expand to show output and arguments. Enabling
“Other tool args” in F2 exposes raw input for generic extension tools.
