# Optional Interaction & Automation

Several grok-pi integrations are default off in F2 and require a full process
restart because their Pi extensions are injected only at startup:

- **Q&A** adds an `ask_user_question` tool backed by native QuestionView.
- **Pi BTW** adds side questions and optional multi-model chains without
  interrupting the primary turn.
- **Pi workflows** hosts Rhai scripts; use `/workflows` to browse and
  `/workflow <name> [args]` to launch one.
- **Pi goal** is a legacy MVP goal loop with native status projection, not a
  full autonomous multi-agent planner.
- **Pi loop** schedules recurring prompts for the current process; it is
  session-only rather than a durable external scheduler.

When one of these native bridges is enabled, the resource policy blocks known
packages that provide the same role. Enable only the integrations your project
needs and read the F2 conflict description before restart.
