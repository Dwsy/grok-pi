//! `/pi-shortcut-manager` — native Rust UI for Pi extension shortcuts.
//!
//! Opens the Pager-owned modal that lists/enables/remaps shortcuts registered
//! via `pi.registerShortcut`. Independent of remote-tui (extension component
//! host stays untouched).

use crate::app::actions::Action;
use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Open the native extension-shortcut manager modal.
pub struct PiShortcutManagerCommand;

impl SlashCommand for PiShortcutManagerCommand {
    fn name(&self) -> &str {
        "pi-shortcut-manager"
    }

    fn description(&self) -> &str {
        "Manage Pi extension shortcuts (enable / disable / remap)"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/pi-shortcut-manager"
    }

    fn run(&self, ctx: &mut CommandExecCtx, _args: &str) -> CommandResult {
        if ctx.session_id.is_none() {
            return CommandResult::Error("No active session".to_string());
        }
        CommandResult::Action(Action::OpenPiShortcutManager)
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
    fn name_and_action() {
        let cmd = PiShortcutManagerCommand;
        assert_eq!(cmd.name(), "pi-shortcut-manager");
        assert!(!cmd.takes_args());
        let models = ModelState::default();
        let sid = agent_client_protocol::SessionId::new("s1".to_string());
        let mut ctx = CommandExecCtx {
            models: &models,
            session_id: Some(&sid),
            bundle_state: &DEFAULT_BUNDLE_STATE,
            screen_mode: crate::app::ScreenMode::Minimal,
            billing_surface_visible: true,
            usage_command_visible: true,
            pager_state: PagerLocalSnapshot::default(),
        };
        assert!(matches!(
            cmd.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenPiShortcutManager)
        ));
    }
}
