//! `/reload` — reload Pi settings, extensions, skills, prompts, themes, context.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct ReloadCommand;

impl SlashCommand for ReloadCommand {
    fn name(&self) -> &str {
        "reload"
    }

    fn description(&self) -> &str {
        "Reload keybindings, extensions, skills, prompts, themes, and context files"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/reload"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::PiReload)
    }
}
