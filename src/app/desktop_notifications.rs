//! Showing the desktop notifications a pane's program asks for (OSC 99).
//!
//! The terminal core assembles a notification; this decides whether the
//! program's `o=` condition holds for its pane right now, and hands it to the
//! sibling `zntfy` with the tab's attention ID as the target, so a click focuses
//! the tab and the tab's silent mode is honoured exactly as for `zetta notify`.

use std::sync::atomic::{AtomicUsize, Ordering};

use terminal::{DesktopNotification, NotificationExpiry, NotificationOccasion};

use super::*;
use crate::cli_services::{
    NotificationRequest, NotificationTarget, NotificationTimeout, run_notification_proxy,
};

/// Helper processes started and not yet finished. Every notification costs a
/// process, so a program printing them in a loop must not be able to start
/// processes without limit; past this, notifications are dropped.
const MAX_DELIVERIES_IN_FLIGHT: usize = 4;

static DELIVERIES_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// How much of a pane the user can currently see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneExposure {
    /// The pane has keyboard focus in the active window.
    Focused,
    /// The pane is on screen in the active window, but not focused.
    Visible,
    /// In another tab, minimized, in a background session, or the window is
    /// not active.
    Hidden,
}

pub(crate) fn occasion_allows(occasion: NotificationOccasion, exposure: PaneExposure) -> bool {
    match occasion {
        NotificationOccasion::Always => true,
        NotificationOccasion::Unfocused => exposure != PaneExposure::Focused,
        NotificationOccasion::Invisible => exposure == PaneExposure::Hidden,
    }
}

pub(crate) fn notification_request(notification: &DesktopNotification) -> NotificationRequest {
    NotificationRequest {
        summary: notification.title.clone(),
        body: Some(notification.body.clone()).filter(|body| !body.is_empty()),
        app_name: notification.app_name.clone(),
        icon: None,
        sound: None,
        timeout: match notification.expiry {
            NotificationExpiry::Default => None,
            NotificationExpiry::Never => Some(NotificationTimeout::Never),
            NotificationExpiry::Milliseconds(milliseconds) => {
                Some(NotificationTimeout::Milliseconds(milliseconds))
            }
        },
    }
}

impl Zetta {
    pub(crate) fn show_desktop_notification(
        &self,
        tab_id: u64,
        pane_id: u64,
        notification: &DesktopNotification,
        window: &Window,
        cx: &App,
    ) {
        let exposure = self.pane_exposure(tab_id, pane_id, window, cx);
        if !occasion_allows(notification.occasion, exposure) {
            return;
        }
        let Some(attention_id) = self.attention_id_for_tab(tab_id) else {
            return;
        };
        if DELIVERIES_IN_FLIGHT.fetch_add(1, Ordering::AcqRel) >= MAX_DELIVERIES_IN_FLIGHT {
            DELIVERIES_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            log::warn!("dropping a desktop notification: too many still being delivered");
            return;
        }
        let request = notification_request(notification);
        let target = NotificationTarget {
            process_id: std::process::id(),
            attention_id,
        };
        // The helper asks this process for the tab's silent mode, which is
        // answered on the window's thread, so it must not be waited for there.
        let delivery = std::thread::Builder::new()
            .name("zetta-notification".to_owned())
            .spawn(move || {
                if let Err(error) = run_notification_proxy(&request, Some(target)) {
                    log::warn!("desktop notification failed: {error:#}");
                }
                DELIVERIES_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            });
        if let Err(error) = delivery {
            DELIVERIES_IN_FLIGHT.fetch_sub(1, Ordering::AcqRel);
            log::warn!("desktop notification failed to start: {error}");
        }
    }

    fn pane_exposure(&self, tab_id: u64, pane_id: u64, window: &Window, cx: &App) -> PaneExposure {
        let Some(tab) = self
            .tabs
            .get(self.active_tab)
            .filter(|tab| window.is_window_active() && tab.id == tab_id)
            .filter(|tab| tab.pane_is_visible(pane_id))
        else {
            return PaneExposure::Hidden;
        };
        let focused = tab.active_pane == pane_id
            && tab
                .active_view()
                .is_some_and(|view| view.focus_handle(cx).is_focused(window));
        if focused {
            PaneExposure::Focused
        } else {
            PaneExposure::Visible
        }
    }
}

#[cfg(test)]
#[path = "../tests/app/desktop_notifications.rs"]
mod tests;
