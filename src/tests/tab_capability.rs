use std::collections::HashMap;

use super::*;
use crate::{
    Config, IconName, PaneLayout, Profile, ProfileIcon, Shell, Tab, TabClosePolicy,
    TabIconOverride, TerminalPane, background_sessions::SessionAuthentication,
};

fn window(cx: &mut gpui::TestAppContext) -> (gpui::Entity<Zetta>, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
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
    });
    cx.run_until_parked();
    (zetta, cx)
}

/// Opens a tab whose one pane has no terminal yet, so a command sent to it is
/// queued where the test can see it rather than typed into a shell, and
/// returns its attention ID.
fn open_stand_in_tab(zetta: &gpui::Entity<Zetta>, cx: &mut gpui::VisualTestContext) -> u64 {
    zetta.update(cx, |zetta, _| {
        let profile = Profile {
            name: "System".to_owned(),
            command: Shell::System,
            theme: None,
            dark_theme: None,
            icon: ProfileIcon::Zetta,
        };
        let pane_id = zetta.next_pane_id;
        zetta.next_pane_id += 1;
        let tab_id = zetta.next_tab_id;
        zetta.next_tab_id += 1;
        zetta.tabs.push(Tab {
            id: tab_id,
            attention_id: tab_id,
            attention: None,
            panes: vec![TerminalPane::new(pane_id, profile).with_label_number(1)],
            pane_indices: HashMap::from([(pane_id, 0)]),
            next_pane_label: 2,
            theme_override: None,
            layout: PaneLayout::Pane(pane_id),
            active_pane: pane_id,
            focus_history: vec![pane_id],
            maximized_pane: None,
            minimized_panes: Vec::new(),
            selected_minimized_pane: None,
            broadcast_input: false,
            silent_mode: false,
            close_policy: TabClosePolicy::Close,
            protected: false,
            shared: false,
            custom_title: None,
            worktree_seed_title: None,
            process_title: None,
            icon: Some(IconName::Terminal),
            icon_override: TabIconOverride::None,
            pinned: false,
            renaming_pane: None,
            rename_buffer: None,
            editing_overlay_pane: None,
            overlay_buffer: None,
            overlay_style_picker: None,
        });
        zetta.active_tab = zetta.tabs.len() - 1;
        tab_id
    })
}

/// Protects the active tab as reattaching a protected multiplexer session
/// does, which is the case the published catalog cannot show: a private
/// session this window holds is not listed.
fn protect_active_tab(zetta: &gpui::Entity<Zetta>, cx: &mut gpui::VisualTestContext) {
    zetta.update(cx, |zetta, _| {
        zetta.tabs[zetta.active_tab].protected = true;
    });
}

/// Protects the active tab as keeping it running with a secret does in
/// `--no-mux` mode.
fn keep_active_tab_running_protected(
    zetta: &gpui::Entity<Zetta>,
    cx: &mut gpui::VisualTestContext,
) {
    zetta.update(cx, |zetta, _| {
        zetta.tabs[zetta.active_tab].close_policy = TabClosePolicy::Background {
            authentication: Some(SessionAuthentication::create("secret").unwrap()),
        };
    });
}

fn queued_commands(zetta: &gpui::Entity<Zetta>, cx: &mut gpui::VisualTestContext) -> Vec<String> {
    zetta.update(cx, |zetta, _| {
        zetta.tabs[zetta.active_tab]
            .panes
            .iter()
            .filter_map(|pane| pane.pending_command.clone())
            .collect()
    })
}

fn shell_command() -> ShellCommandRequest {
    ShellCommandRequest {
        command: "echo injected".to_owned(),
        arguments: Vec::new(),
        environment: Default::default(),
    }
}

fn refused_as_protected(result: Result<()>) -> bool {
    result.is_err_and(|error| format!("{error:#}").contains("protected"))
}

#[gpui::test]
fn an_unprotected_tab_takes_control_requests(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = window(cx);
    let attention_id = open_stand_in_tab(&zetta, cx);
    zetta.update_in(cx, |zetta, window, cx| {
        zetta
            .run_shell_command_from_control(shell_command(), window, cx)
            .unwrap();
        assert!(zetta.command_pane_labels_from_control(None).is_ok());
        assert!(
            zetta
                .command_pane_labels_from_control(Some(attention_id))
                .is_ok()
        );
    });
    assert_eq!(queued_commands(&zetta, cx).len(), 1);
}

fn assert_refuses_control_requests(zetta: &gpui::Entity<Zetta>, cx: &mut gpui::VisualTestContext) {
    let attention_id = zetta.update(cx, |zetta, _| zetta.tabs[zetta.active_tab].attention_id);
    zetta.update_in(cx, |zetta, window, cx| {
        assert!(refused_as_protected(zetta.run_shell_command_from_control(
            shell_command(),
            window,
            cx
        )));
        for (direction, stack) in [
            (None, false),
            (Some(crate::pane::PaneDirection::Right), false),
            (None, true),
        ] {
            let request = PaneCommand {
                direction,
                label: None,
                pane: None,
                overlay: None,
                stack,
                list: false,
                command: vec!["sh".to_owned()],
            };
            assert!(refused_as_protected(
                zetta.run_command_pane_from_control(request, window, cx)
            ));
        }
        assert!(zetta.command_pane_labels_from_control(None).is_err());
        assert!(
            zetta
                .command_pane_labels_from_control(Some(attention_id))
                .is_err()
        );
        assert_eq!(
            zetta.tabs[zetta.active_tab].panes.len(),
            1,
            "no pane was opened"
        );
    });
    assert!(
        queued_commands(zetta, cx).is_empty(),
        "nothing was sent to the protected pane"
    );
}

/// The window must not become a deputy that types what any token holder sends
/// into a protected pane, or opens panes beside it.
#[gpui::test]
fn an_attached_protected_tab_refuses_control_requests(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = window(cx);
    open_stand_in_tab(&zetta, cx);
    protect_active_tab(&zetta, cx);
    assert_refuses_control_requests(&zetta, cx);
}

#[gpui::test]
fn a_tab_kept_running_with_a_secret_refuses_control_requests(cx: &mut gpui::TestAppContext) {
    let (zetta, cx) = window(cx);
    open_stand_in_tab(&zetta, cx);
    keep_active_tab_running_protected(&zetta, cx);
    assert_refuses_control_requests(&zetta, cx);
}
