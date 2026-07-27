# Context, Cache & Compaction

grok-pi exposes Pi's context state without filling the conversation with
repeated diagnostics.

- The context bar shows current token use.
- `/context` opens a live native modal with messages, tools and cache views.
- The Pi cache graph is default on; keys `1`, `2`, `3`, `s` and `e` switch
  breakdowns or export the current snapshot.
- `/session-info` prints Pi session statistics into scrollback.
- Pi still owns automatic compaction. `/compact [instructions]` requests it
  manually and can preserve a focus such as migration decisions.
- `/recap [focus]` generates a return-to-work summary through the configured
  recap pipeline; auto-away recap and Mermaid output are optional F2 settings.

Compaction changes the model context. Recap is display-only and does not rewrite
Pi's session history.
