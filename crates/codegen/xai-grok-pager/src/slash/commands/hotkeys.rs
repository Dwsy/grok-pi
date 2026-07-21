//! `/hotkeys` -- open the keyboard shortcuts cheatsheet modal.
//!
//! Aligns with Pi's `/hotkeys` name. Grok already owns the searchable
//! `ShortcutsHelp` modal (Ctrl+. / Ctrl+X); this slash is the discoverable
//! entry for users who type the Pi command form.

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Open the native shortcuts help modal.
pub struct HotkeysCommand;

impl SlashCommand for HotkeysCommand {
    fn name(&self) -> &str {
        "hotkeys"
    }

    fn aliases(&self) -> &[&str] {
        // Grok keybinding help language uses "shortcuts"; keep Pi name primary.
        &["shortcuts", "keys"]
    }

    fn description(&self) -> &str {
        "Show keyboard shortcuts"
    }

    fn usage(&self) -> &str {
        "/hotkeys"
    }

    fn run(&self, _ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        CommandResult::Action(Action::OpenShortcutsHelp)
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

    #[test]
    fn dispatches_open_shortcuts_help() {
        let models = ModelState::default();
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: None,
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Minimal,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            HotkeysCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenShortcutsHelp)
        ));
        assert_eq!(HotkeysCommand.aliases(), &["shortcuts", "keys"]);
    }
}
