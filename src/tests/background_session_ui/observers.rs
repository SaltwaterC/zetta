use super::*;

use crate::{PaneLayout, TabClosePolicy, TabIconOverride, TerminalPane};

fn zetta_window(cx: &mut gpui::TestAppContext) -> (Entity<Zetta>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear(); // No shell or daemon in this window.
        Zetta::new(
            config,
            None,
            crate::ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    })
}

fn tab(tab_id: u64, attention_id: u64) -> Tab {
    let profile = Profile {
        console_backend: Default::default(),
        name: "System".to_owned(),
        command: task::Shell::System,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::default(),
    };
    Tab {
        id: tab_id,
        attention_id,
        attention: None,
        panes: vec![TerminalPane::new(1, profile).with_label_number(1)],
        pane_indices: HashMap::from([(1, 0)]),
        next_pane_label: 2,
        theme_override: None,
        layout: PaneLayout::Pane(1),
        active_pane: 1,
        focus_history: vec![1],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        protected: false,
        shared: true,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: None,
        icon_override: TabIconOverride::None,
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_style_picker: None,
        group: Default::default(),
    }
}

/// A pane this window did not spawn — reattached from a background session,
/// restored, or joined — is connected here rather than by the spawn path, and
/// its program's OSC 99 notifications must still reach the desktop.
#[gpui::test]
fn a_reconnected_pane_shows_its_programs_desktop_notifications(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = zetta_window(cx);
    let terminal = zetta.update_in(cx, |zetta, window, cx| {
        zetta.tabs.push(tab(7, 41));
        zetta.active_tab = zetta.tabs.len() - 1;
        let terminal = cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                util::paths::PathStyle::local(),
            )
            .subscribe(cx)
        });
        let view = cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), None, window, cx));
        zetta.connect_terminal_view(7, 1, view, window, cx);
        terminal
    });
    crate::app::take_delivered_desktop_notifications();

    terminal.update(cx, |_, cx| {
        cx.emit(TerminalEvent::DesktopNotification(
            terminal::DesktopNotification {
                id: None,
                title: "Build finished".to_owned(),
                body: String::new(),
                app_name: None,
                occasion: terminal::NotificationOccasion::Always,
                expiry: terminal::NotificationExpiry::Default,
            },
        ));
    });
    cx.run_until_parked();

    let delivered = crate::app::take_delivered_desktop_notifications();
    assert_eq!(
        delivered
            .iter()
            .map(|(request, target)| (request.summary.as_str(), target.attention_id))
            .collect::<Vec<_>>(),
        [("Build finished", 41)],
        "a reconnected pane's notification must be shown, targeting its tab"
    );
}
