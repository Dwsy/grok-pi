//! `/clone` — duplicate the current Pi session at the current leaf.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Pi `/clone`: new session file at the current leaf, empty prompt.
pub struct CloneCommand;

impl SlashCommand for CloneCommand {
    fn name(&self) -> &str {
        "clone"
    }

    fn description(&self) -> &str {
        "Duplicate the current session at the current position"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/clone"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::PiClone)
    }
}
