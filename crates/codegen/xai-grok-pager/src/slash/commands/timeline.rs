//! `/timeline` toggles the per-turn timeline sidebar.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct TimelineCommand;

impl SlashCommand for TimelineCommand {
    fn name(&self) -> &str {
        "timeline"
    }

    fn description(&self) -> &str {
        "Toggle the timeline sidebar"
    }

    fn available_in_minimal(&self) -> bool {
        false
    }

    fn usage(&self) -> &str {
        "/timeline"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::SetTimeline(
            !crate::appearance::cache::load_show_timeline(),
        ))
    }
}
