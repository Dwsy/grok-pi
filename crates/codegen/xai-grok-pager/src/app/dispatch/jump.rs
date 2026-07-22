//! `/jump` picker dispatchers: pure client-side turn navigation.

use crate::app::actions::Effect;
use crate::app::app_view::{ActiveView, AppView};
use crate::scrollback::entry::EntryId;
use crate::views::jump::{JumpRestore, JumpState};

pub(super) fn dispatch_jump_show_picker(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    if agent.jump_slot_taken() {
        return vec![];
    }

    let entries = agent.scrollback.timeline_entries();
    if entries.len() < 2 {
        app.show_toast("Nothing to jump to yet");
        return vec![];
    }

    let restore = JumpRestore {
        bookmark: agent.scrollback.capture_scroll_bookmark(),
        selected: agent.scrollback.selected(),
        follow_mode: agent.scrollback.is_follow_mode(),
    };
    let selected = agent
        .scrollback
        .active_turn_for_viewport()
        .unwrap_or(entries.len() - 1)
        .min(entries.len() - 1);

    let preview_id = entries[selected].prompt_entry_id;
    agent.jump_state = Some(JumpState::new(entries, selected, restore));
    if let Some(index) = agent.scrollback.index_of_id(preview_id) {
        agent.scrollback.scroll_to_entry_top(index);
    }
    vec![]
}

pub(super) fn dispatch_jump_picker_select(app: &mut AppView, prompt_id: EntryId) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    let Some(state) = agent.jump_state.take() else {
        return vec![];
    };
    if !agent.scrollback.jump_to_entry(prompt_id) {
        agent.restore_jump_viewport(state.restore);
    }
    vec![]
}

pub(super) fn dispatch_jump_dismiss(app: &mut AppView) -> Vec<Effect> {
    let ActiveView::Agent(id) = app.active_view else {
        return vec![];
    };
    let Some(agent) = app.agents.get_mut(&id) else {
        return vec![];
    };
    agent.dismiss_jump_picker();
    vec![]
}
