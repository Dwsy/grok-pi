//! `/jump` picker: transcript preview syncing and key/mouse handling.

use super::AgentView;
use crate::app::actions::Action;
use crate::app::app_view::InputOutcome;
use crate::views::jump::{
    JumpInput, JumpRestore, handle_jump_key, jump_activate, jump_row_at, move_cursor,
    set_jump_cursor,
};
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};

impl AgentView {
    pub(crate) fn dismiss_jump_picker(&mut self) {
        if let Some(state) = self.jump_state.take() {
            self.restore_jump_viewport(state.restore);
        }
    }

    pub(crate) fn restore_jump_viewport(&mut self, restore: JumpRestore) {
        self.scrollback.set_selected(restore.selected);
        if let Some(bookmark) = restore.bookmark {
            self.scrollback.restore_scroll_bookmark(bookmark);
        }
        if restore.follow_mode {
            self.scrollback.enable_follow();
        }
    }

    pub(crate) fn jump_slot_taken(&self) -> bool {
        self.rewind_state.is_some()
            || self.inline_edit.is_some()
            || self.btw_state.is_some()
            || self.fork_state.is_some()
            || !self.no_input_overlay_pending()
    }

    pub(super) fn dismiss_jump_picker_if_suppressed(&mut self) -> bool {
        if self.jump_state.is_some() && self.jump_slot_taken() {
            self.dismiss_jump_picker();
            return true;
        }
        false
    }

    pub(super) fn sync_jump_preview(&mut self) {
        let Some(prompt_id) = self
            .jump_state
            .as_ref()
            .and_then(|state| state.entries.get(state.selected))
            .map(|entry| entry.prompt_entry_id)
        else {
            return;
        };
        if let Some(index) = self.scrollback.index_of_id(prompt_id) {
            self.scrollback.scroll_to_entry_top(index);
        }
    }

    pub(super) fn handle_jump_key(&mut self, key: &KeyEvent) -> InputOutcome {
        let Some(state) = self.jump_state.as_ref() else {
            return InputOutcome::Unchanged;
        };
        match handle_jump_key(state, key) {
            JumpInput::MoveUp => {
                if let Some(state) = self.jump_state.as_mut() {
                    move_cursor(state, -1);
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            JumpInput::MoveDown => {
                if let Some(state) = self.jump_state.as_mut() {
                    move_cursor(state, 1);
                    self.sync_jump_preview();
                }
                InputOutcome::Changed
            }
            input => Self::jump_input_to_outcome(input),
        }
    }

    fn jump_input_to_outcome(input: JumpInput) -> InputOutcome {
        match input {
            JumpInput::Select(id) => InputOutcome::Action(Action::JumpPickerSelect(id)),
            JumpInput::Dismissed => InputOutcome::Action(Action::JumpDismiss),
            JumpInput::MoveUp | JumpInput::MoveDown | JumpInput::Consumed => InputOutcome::Changed,
        }
    }

    pub(super) fn handle_jump_mouse(&mut self, mouse: &MouseEvent) -> InputOutcome {
        let Some(state) = self.jump_state.as_mut() else {
            return InputOutcome::Unchanged;
        };
        let area = self.pane_areas.prompt;
        let Some(index) = jump_row_at(state, area, mouse.column, mouse.row) else {
            return InputOutcome::Unchanged;
        };

        match mouse.kind {
            MouseEventKind::Moved => {
                if set_jump_cursor(state, index) {
                    self.sync_jump_preview();
                    InputOutcome::Changed
                } else {
                    InputOutcome::Unchanged
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                set_jump_cursor(state, index);
                Self::jump_input_to_outcome(jump_activate(state))
            }
            _ => InputOutcome::Unchanged,
        }
    }
}
