//! `/pi-config` -- open the native Pi resource configuration modal.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Opens the Pager-owned Pi resource manager without starting Pi's separate TUI.
pub struct PiConfigCommand;

impl SlashCommand for PiConfigCommand {
    fn name(&self) -> &str {
        "pi-config"
    }

    fn aliases(&self) -> &[&str] {
        &["pi-resources"]
    }

    fn description(&self) -> &str {
        "Manage Pi extensions, skills, prompts, and themes"
    }

    fn usage(&self) -> &str {
        "/pi-config"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenPiConfig)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::ScreenMode;
    use crate::app::bundle::BundleState;

    #[test]
    fn dispatches_native_pi_config_action() {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: ScreenMode::Fullscreen,
            pager_state: Default::default(),
        };
        assert!(matches!(
            PiConfigCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenPiConfig)
        ));
    }

    #[test]
    fn registers_resource_alias() {
        assert_eq!(PiConfigCommand.aliases(), &["pi-resources"]);
    }
}
