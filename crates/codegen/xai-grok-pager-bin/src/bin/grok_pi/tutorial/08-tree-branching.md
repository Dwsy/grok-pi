# Session Tree & Branching

A Pi session is a tree, not just a flat transcript. Pi owns the active leaf;
grok-pi provides native navigation views.

- `/tree` opens a searchable tree with collapse, detail, copy and label support.
  Navigation can optionally summarize the branch being left.
- `/jump` selects a message in the current history.
- `/tree-map` shows user-message branch points as a compact map.
- `/fork` selects a prior user message, creates a new Pi session file and returns
  that text to the prompt.
- `/clone` duplicates the current leaf into a new session with an empty prompt.

Tree navigation is non-destructive. It is not stock Grok rewind, and Pi session
forking is not Grok's worktree product flow.
