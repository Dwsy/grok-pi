# pi_user_markdown — grok-pi user prompt markdown rendering

**Status:** done
**Date:** 2026-08-05

## Summary

Add an F2 setting (`pi_user_markdown`, default **on**) so grok-pi user messages render with the agent markdown renderer (expanded, no collapse). Turning it off restores classic collapsible plain-text `UserPromptBlock` behavior.

## Scope

- External-agent (grok-pi) profile only (`external_only` setting).
- Applies immediately on toggle (no restart).
- Preserves user prompt chrome (prefix, accent band, background).

## Implementation

| Layer | Change |
|---|---|
| `[ui].pi_user_markdown` | `UiConfig` field, default `true` |
| Appearance cache | `load_pi_user_markdown` / `set_pi_user_markdown` |
| F2 | Agent → **Markdown user messages** |
| Setter | `set_pi_user_markdown` + `apply_pi_user_markdown_flip` on all agent/subagent scrollbacks |
| `UserPromptBlock` | Lazy `MarkdownContent`; `use_agent_renderer()` = external profile ∧ cache flag |
| Persist | `helpers.rs` → `set_pi_user_markdown` |

## Verification

```bash
./scripts/cargo-shared.sh test -p xai-grok-pager --lib -- scrollback::blocks::user
./scripts/cargo-shared.sh test -p xai-grok-pager --lib -- settings::registry
./scripts/cargo-shared.sh test -p xai-grok-pager-render --lib -- appearance::cache
./scripts/cargo-shared.sh check -p xai-grok-pager-bin --bin grok-pi
```
