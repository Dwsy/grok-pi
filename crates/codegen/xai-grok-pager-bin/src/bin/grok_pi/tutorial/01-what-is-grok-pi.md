# What Is grok-pi?

grok-pi is not stock Grok. It combines the **Pi agent core** with Grok Pager
through three deliberately separate layers:

- **Pi is the agent core.** Pi owns providers, models, the agent loop, tools,
  extensions, compaction and local session files.
- **Grok Pager is the terminal UI.** It owns the prompt, scrollback, Markdown,
  tool cards, diffs, pickers, modals and terminal lifecycle.
- **The adapter stays headless.** It translates Pi RPC events into Pager-native
  surfaces and never draws a second terminal interface.

Some product features—Plan mode, background Bash, subagents and workflows—are
built-in grok-pi bridge extensions. They are not fixed Pi kernel features, and
Pi remains replaceable and extensible underneath them.

Your models and sessions belong to Pi, not a Grok cloud account.
