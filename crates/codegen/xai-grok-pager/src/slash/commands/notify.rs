//! `/notify` — browse Pi extension notification events for the active session.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

pub struct NotifyCommand;

impl SlashCommand for NotifyCommand {
    fn name(&self) -> &str {
        "notify"
    }

    fn description(&self) -> &str {
        "Browse Pi notification events for this session"
    }

    fn usage(&self) -> &str {
        "/notify"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".into());
        }
        CommandResult::Action(Action::ShowNotifications)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::bundle::BundleState;
    use crate::settings::PagerLocalSnapshot;

    static BUNDLE: BundleState = BundleState {
        has_cache: false,
        version: String::new(),
        personas: Vec::new(),
        roles: Vec::new(),
        agents: Vec::new(),
        skills: Vec::new(),
        persona_details: Vec::new(),
        role_details: Vec::new(),
    };

    #[test]
    fn requires_an_active_session() {
        let models = crate::acp::model_state::ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &BUNDLE,
            screen_mode: crate::app::ScreenMode::Minimal,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            NotifyCommand.run(&mut ctx, ""),
            CommandResult::Error(_)
        ));
    }

    #[test]
    fn opens_the_notification_list() {
        let models = crate::acp::model_state::ModelState::default();
        let session_id = agent_client_protocol::SessionId::from("s1".to_string());
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: Some(&session_id),
            bundle_state: &BUNDLE,
            screen_mode: crate::app::ScreenMode::Minimal,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            NotifyCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::ShowNotifications)
        ));
    }
}
