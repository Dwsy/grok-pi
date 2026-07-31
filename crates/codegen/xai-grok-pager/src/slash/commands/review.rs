//! `/review-session` and `/review-message` — native code-review surfaces.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct ReviewSessionCommand;

impl SlashCommand for ReviewSessionCommand {
    fn name(&self) -> &str {
        "review-session"
    }

    fn description(&self) -> &str {
        "Review file changes in this session (edit/write)"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/review-session"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::ReviewShowSession)
    }
}

pub struct ReviewMessageCommand;

impl SlashCommand for ReviewMessageCommand {
    fn name(&self) -> &str {
        "review-message"
    }

    fn description(&self) -> &str {
        "Pick a turn (jump-style) then review its file changes"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/review-message"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::ReviewShowMessagePicker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    static DEFAULT_BUNDLE_STATE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    fn ctx<'a>(models: &'a ModelState) -> CommandExecCtx<'a> {
        CommandExecCtx {
            models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Fullscreen,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        }
    }

    #[test]
    fn review_session_action() {
        let models = ModelState::default();
        let mut c = ctx(&models);
        assert!(matches!(
            ReviewSessionCommand.run(&mut c, ""),
            CommandResult::Action(Action::ReviewShowSession)
        ));
    }

    #[test]
    fn review_message_action() {
        let models = ModelState::default();
        let mut c = ctx(&models);
        assert!(matches!(
            ReviewMessageCommand.run(&mut c, ""),
            CommandResult::Action(Action::ReviewShowMessagePicker)
        ));
    }
}
