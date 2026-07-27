# Extensions & Dynamic Commands

Pi extensions are TypeScript modules with full access to the host process. They
can observe or change the agent lifecycle rather than merely adding prompts.

An extension can:

- call `registerTool`, `registerCommand`, `registerShortcut` or register flags;
- call `registerProvider` for a custom model backend;
- inspect or modify context, system prompts, tool calls and provider requests;
- send messages, switch active tools, persist session state or react to events.

Extension, Skill and Prompt Template commands enter Pager's native slash catalog
dynamically. Extension commands execute directly in Pi instead of becoming a
fake follow-up row. Use `--extension` for an explicit module, `--no-extensions`
to suppress discovery, and `/reload` after changing resources.

Extensions are trusted code with your user permissions—review them before use.
