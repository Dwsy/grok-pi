# Skills, Prompts & Context Files

Pi discovers reusable instructions from global, project and package sources.

- **Skills** follow the Agent Skills format and load progressively. Invoke one
  directly with `/skill:name`; project Skills require project trust.
- **Prompt Templates** are Markdown slash commands with arguments, defaults and
  positional slicing.
- **Context files** such as `AGENTS.md` and `CLAUDE.md` are collected from the
  project hierarchy and global Pi configuration.
- Pi Packages can bundle Skills, templates, extensions and themes together.

These resources join grok-pi's native slash dropdown through Pi's command
catalog. Use `/reload` after edits. Startup flags such as `--no-skills` and
`--no-context-files` let Pi start without those discovery layers.

A Skill or context file guides the model; it does not bypass tool policy or
project trust.
