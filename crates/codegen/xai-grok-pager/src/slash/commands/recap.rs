//! `/recap` (alias `/summarize`) -- summarize the session so far ("where was I").
//!
//! Optional free-text args become `customInstructions` injected into the recap
//! prompt (same pattern as `/compact`). Returns
//! `CommandResult::Action(Action::SendRecap { .. })` so the dispatch layer fires
//! ACP `x.ai/recap` (bypasses the prompt queue). The recap arrives as a
//! scrollback line and is never added to the model conversation.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct RecapCommand;

impl SlashCommand for RecapCommand {
    fn name(&self) -> &str {
        "recap"
    }

    fn aliases(&self) -> &[&str] {
        &["summarize"]
    }

    fn description(&self) -> &str {
        "Summarize the session so far"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/recap [focus instructions]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        false
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("focus instructions")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        CommandResult::Action(Action::SendRecap {
            auto: false,
            custom_instructions: if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            },
        })
    }
}
