//! Process-local Pi notification history dispatch.

use crate::app::actions::Effect;
use crate::app::app_view::AppView;
use crate::app::dispatch::ctx::get_active_agent_mut;
use crate::views::modal::{ActiveModal, NotificationListState};
use crate::views::modal_window::ModalWindowState;

pub(in crate::app::dispatch) fn dispatch_show_notifications(app: &mut AppView) -> Vec<Effect> {
    if !app.external_agent {
        app.show_toast("Notifications are only available for Pi sessions");
        return vec![];
    }
    let notifications = app.external_notifications_for_active_session().to_vec();
    let Some(agent) = get_active_agent_mut(app) else {
        app.show_toast("No active session");
        return vec![];
    };
    agent.active_modal = Some(ActiveModal::Notifications {
        state: NotificationListState::new(notifications),
        window: ModalWindowState::new(),
    });
    vec![]
}
