use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Upstream-aligned `/workflow` — launch or manage host-owned runs.
pub struct WorkflowCommand;

impl SlashCommand for WorkflowCommand {
    fn name(&self) -> &str {
        "workflow"
    }

    fn description(&self) -> &str {
        "Launch a saved workflow, or manage a run (pause, resume, stop, save)"
    }

    fn usage(&self) -> &str {
        "/workflow <name> [args] | pause|resume|stop|save [name]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<name> [args] | pause|resume|stop|save [name]")
    }

    fn visible(&self, ctx: &crate::slash::command::AppCtx) -> bool {
        ctx.workflows_available
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        const OPS: [&str; 4] = ["pause", "resume", "stop", "save"];
        let trimmed = args.trim();
        let mut parts = trimmed.split_whitespace();
        let first = parts.next().unwrap_or_default();
        let second = parts.next().unwrap_or_default();
        let first_is_op = OPS.contains(&first.to_lowercase().as_str());
        let second_is_final_op =
            OPS.contains(&second.to_lowercase().as_str()) && parts.next().is_none();

        if first.is_empty() || first_is_op || second_is_final_op {
            let (op, target) = if first_is_op {
                (
                    first.to_lowercase(),
                    trimmed[first.len()..].trim_start().to_string(),
                )
            } else if second_is_final_op {
                (second.to_lowercase(), first.to_string())
            } else {
                (String::new(), String::new())
            };
            if op.is_empty() {
                return CommandResult::Message(
                    "Usage: /workflow <name> [args] to launch, or /workflow pause|resume|stop|save [name]"
                        .into(),
                );
            }
            if op == "save" {
                return CommandResult::Message(
                    "Save is available for non-builtin project scripts via /workflows panel (coming to host RPC)."
                        .into(),
                );
            }
            if op == "resume" {
                return CommandResult::Message(
                    "Resume a paused run from /workflows (Ctrl+R) or re-launch with /workflow <name>."
                        .into(),
                );
            }
            return CommandResult::Action(Action::WorkflowManage { op, target });
        }

        // Launch: /workflow <name> [args...]
        let name = first.to_string();
        let rest = trimmed[first.len()..].trim_start().to_string();
        CommandResult::Action(Action::WorkflowLaunch { name, args: rest })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    #[test]
    fn launch_parses_name_and_args() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &crate::app::bundle::BundleState {
                has_cache: false,
                version: String::new(),
                personas: Vec::new(),
                roles: Vec::new(),
                agents: Vec::new(),
                skills: Vec::new(),
                persona_details: Vec::new(),
                role_details: Vec::new(),
            },
            screen_mode: crate::app::ScreenMode::Fullscreen,
            billing_surface_visible: false,
            usage_command_visible: true,
            pager_state: crate::settings::PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            WorkflowCommand.run(&mut ctx, "deep-research hello"),
            CommandResult::Action(Action::WorkflowLaunch { name, args })
                if name == "deep-research" && args == "hello"
        ));
        assert!(matches!(
            WorkflowCommand.run(&mut ctx, "pause deep-research"),
            CommandResult::Action(Action::WorkflowManage { op, target })
                if op == "pause" && target == "deep-research"
        ));
    }
}
