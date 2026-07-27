# Plan Mode & Todo

Press `Ctrl+Shift+T` to toggle grok-pi Plan mode for the current Pi session.

- Pi can read and search the real repository before proposing an approach.
- The built-in grok-pi plan extension blocks normal `edit`, `write` and `bash`
  mutations except for the session-private plan file.
- The mode state is stored beside the Pi session and survives resume.
- Pi's `exit_plan_mode` bridge opens the native approval view so you can accept
  the plan or request changes before implementation.

Todo is a separate structured projection: when a Pi todo tool reports
`details.tasks`, grok-pi maps it to the native TodoPane, badge and ACP Plan
instead of rendering a duplicate tool card. Plan and Todo are product bridges,
not fixed Pi kernel features.
