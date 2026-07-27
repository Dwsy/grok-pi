//! `/pi-models` -- open the native Pi model-management center.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Opens the Pager-owned editor for Pi's `models.json`.
pub struct PiModelsCommand;

impl SlashCommand for PiModelsCommand {
    fn name(&self) -> &str {
        "pi-models"
    }

    fn aliases(&self) -> &[&str] {
        &["model-config", "models-config"]
    }

    fn description(&self) -> &str {
        "Manage Pi providers and models with live reload"
    }

    fn usage(&self) -> &str {
        "/pi-models"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenPiModels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;
    use crate::app::ScreenMode;
    use crate::app::bundle::BundleState;

    #[test]
    fn dispatches_native_model_manager() {
        let models = ModelState::default();
        let bundle = BundleState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &bundle,
            screen_mode: ScreenMode::Fullscreen,
            billing_surface_visible: false,
            pager_state: Default::default(),
        };
        assert!(matches!(
            PiModelsCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenPiModels)
        ));
    }

    #[test]
    fn registers_compatibility_aliases() {
        assert_eq!(
            PiModelsCommand.aliases(),
            &["model-config", "models-config"]
        );
    }
}
