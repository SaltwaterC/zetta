use super::*;
use crate::background_sessions::{BackgroundPaneExitReason, BackgroundPaneExitSource};

#[test]
fn reconnect_is_immediate_only_for_one_background_session() {
    assert_eq!(reconnect_request(0), ReconnectRequest::None);
    assert_eq!(reconnect_request(1), ReconnectRequest::Immediate(0));
    assert_eq!(reconnect_request(2), ReconnectRequest::Choose);
}

#[test]
fn no_mux_keep_running_is_explicitly_process_local() {
    let action = ProtectedSessionAction::KeepRunning;

    assert_eq!(action.title(true), "Keep tab running after close");
    assert!(
        action
            .description(true)
            .contains("stays inside this Zetta process")
    );
    assert_eq!(action.submit_label(true), "Protect and keep running");

    assert_eq!(action.title(false), "Keep and share tab after close");
    assert!(action.description(false).contains("another Zetta process"));
    assert_eq!(action.submit_label(false), "Protect, keep, and share");
}

#[test]
fn background_session_is_reaped_after_its_final_pane_exits() {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let tab = Tab {
        id: 1,
        attention_id: 1,
        attention: None,
        panes: vec![TerminalPane {
            id: 3,
            label_number: 1,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile,
            environment_overrides: HashMap::new(),
            terminal: None,
            view: None,
            error: None,
            exit: None,
            base_exited: false,
            wsl_cwd_file: None,
            pending_command: None,
            detected_worktree_title: None,
            worktree_detection_directory: None,
            worktree_detection_generation: 0,
            worktree_detection_can_clear: false,
            stack: PaneStack::default(),
        }],
        pane_indices: HashMap::from([(3, 0)]),
        next_pane_label: 2,
        layout: PaneLayout::Pane(3),
        active_pane: 3,
        focus_history: vec![3],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: true,
        close_policy: TabClosePolicy::Close,
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: Some(IconName::Terminal),
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    };
    let mut sessions = BackgroundSessionRunner::default();
    sessions.detach(tab, None);

    let reconnected = sessions.reconnect_at(0).unwrap();
    assert!(reconnected.silent_mode);
    sessions.detach(reconnected, None);

    assert_eq!(
        remove_exited_background_pane(&mut sessions, 3),
        Some(vec![3])
    );
    assert!(sessions.is_empty());
}

#[test]
fn protected_sessions_are_redacted_in_the_reconnect_picker() {
    let entries = Zetta::picker_entries_from_summaries(&[BackgroundSessionSummary {
        id: 42,
        title: "production database".to_owned(),
        authentication_required: true,
        active_pane: 7,
        layout: BackgroundPaneLayout::Pane { pane_id: 7 },
        panes: vec![BackgroundPaneSummary {
            id: 7,
            label: "secret work".to_owned(),
            profile: "System".to_owned(),
            configured_command: "sensitive-command".to_owned(),
            application: "psql".to_owned(),
            foreground_command: None,
            terminal_title: None,
            working_directory: None,
            state: BackgroundPaneState::Running,
            exit: None,
        }],
        held: false,
        scoped_to: None,
    }]);

    assert_eq!(
        entries,
        vec![(
            42,
            "Protected session".to_owned(),
            "Session 42 · protected".to_owned()
        )]
    );
}

#[test]
fn failed_sessions_show_the_exit_reason_in_the_reconnect_picker() {
    let entries = Zetta::picker_entries_from_summaries(&[BackgroundSessionSummary {
        id: 8,
        title: "shell".to_owned(),
        authentication_required: false,
        active_pane: 1,
        layout: BackgroundPaneLayout::Split {
            axis: "horizontal".to_owned(),
            first: Box::new(BackgroundPaneLayout::Pane { pane_id: 1 }),
            second: Box::new(BackgroundPaneLayout::Pane { pane_id: 2 }),
        },
        panes: vec![
            BackgroundPaneSummary {
                id: 1,
                label: "failed shell".to_owned(),
                profile: "System".to_owned(),
                configured_command: "pwsh".to_owned(),
                application: "htop".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Failed,
                exit: Some(BackgroundPaneExit {
                    source: BackgroundPaneExitSource::Child,
                    reason: BackgroundPaneExitReason::ForegroundCommand,
                    exit_code: Some(1),
                    child_pid: Some(42),
                    input_sent: true,
                    foreground_is_shell: Some(false),
                    foreground_command: Some("htop".to_owned()),
                }),
            },
            BackgroundPaneSummary {
                id: 2,
                label: "running shell".to_owned(),
                profile: "System".to_owned(),
                configured_command: "pwsh".to_owned(),
                application: "pwsh".to_owned(),
                foreground_command: None,
                terminal_title: None,
                working_directory: None,
                state: BackgroundPaneState::Running,
                exit: None,
            },
        ],
        held: false,
        scoped_to: None,
    }]);

    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .2
            .contains("failed: the shell exited while \"htop\" was foreground")
    );
}

#[test]
fn the_reconnect_picker_never_offers_a_window_the_session_it_is_showing() {
    // Sharing a tab publishes its session while the tab stays on screen, so
    // without this the window that shared it is offered its own session back.
    // Attaching it there is not a join: the multiplexer hands a pane straight
    // back to the process already holding it, so the window would end up with
    // two tabs reading one pty and splitting its output between them.
    let mut panes = crate::mux::MuxPanes::default();
    panes.session_for_tab(1).set_id(900);
    let own_runner = 3;
    let entry = |runner_id, session_id| {
        (
            runner_id,
            session_id,
            "shell".to_owned(),
            "Session".to_owned(),
        )
    };

    assert!(session_is_already_shown_here(
        &panes,
        &entry(7, 900),
        own_runner
    ));
    assert!(!session_is_already_shown_here(
        &panes,
        &entry(7, 901),
        own_runner
    ));
    // A session kept inside this process because the multiplexer was
    // unreachable is numbered from its own counter, so its id can collide with a
    // multiplexer session's. Those entries are this window's to transfer and
    // must stay listed; the reconnect path refuses a genuine duplicate anyway.
    assert!(!session_is_already_shown_here(
        &panes,
        &entry(own_runner, 900),
        own_runner
    ));
}

/// A single-pane tab with one command stacked on it, for the paths that treat a
/// stacked entry as a pane in its own right.
fn attached_tab_with_a_stacked_command() -> Tab {
    let profile = Profile {
        name: "System".to_owned(),
        command: Shell::System,
        theme: None,
        icon: ProfileIcon::Zetta,
    };
    let mut pane = TerminalPane::new(11, profile.clone()).with_label_number(1);
    pane.stack.entries.push(StackedPane::new(
        12,
        "cargo test".to_owned(),
        profile.clone(),
        None,
        None,
    ));
    Tab {
        id: 5,
        attention_id: 5,
        attention: None,
        panes: vec![pane],
        pane_indices: HashMap::from([(11, 0)]),
        next_pane_label: 2,
        layout: PaneLayout::Pane(11),
        active_pane: 11,
        focus_history: vec![11],
        maximized_pane: None,
        minimized_panes: Vec::new(),
        selected_minimized_pane: None,
        broadcast_input: false,
        silent_mode: false,
        close_policy: TabClosePolicy::Close,
        shared: false,
        custom_title: None,
        worktree_seed_title: None,
        process_title: None,
        icon: None,
        pinned: false,
        renaming_pane: None,
        rename_buffer: None,
        rename_cursor: 0,
        rename_select_all: false,
        editing_overlay_pane: None,
        overlay_buffer: None,
        overlay_cursor: 0,
        overlay_select_all: false,
        overlay_style_picker: None,
    }
}

#[test]
fn a_tab_arriving_from_elsewhere_inherits_this_windows_project() {
    // A pane's theme is resolved once, when its view is built, and it resolves
    // through the pane's project. Attaching a session recorded no project for
    // its panes at all, so the theme fell back to the default and only corrected
    // itself the next time the pane's title changed — so a joined session
    // running a full-screen program kept the wrong theme until it quit.
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("project");
    std::fs::create_dir_all(root.join(".zetta")).unwrap();
    std::fs::write(
        crate::project::ProjectConfig::path_for(&root),
        "{\"theme\": \"Ayu Dark\"}\n",
    )
    .unwrap();
    let registry_path = temporary.path().join("registry.json");
    let mut registry = crate::project::ProjectRegistry::load_from(registry_path).unwrap();
    registry.add(&root).unwrap();
    let mut projects = crate::project_context::ProjectState::new(registry);
    let config = crate::project::ProjectConfig::load(&root, &Config::defaults(None, None)).unwrap();
    projects.insert_config(config);

    // The pane this window is on already belongs to the project.
    projects.pane_roots.insert(7, root.clone());

    let tab = attached_tab_with_a_stacked_command();

    inherit_project_for_panes(&mut projects, 7, &tab);

    assert_eq!(projects.root_for_pane(11), Some(&root));
    assert!(
        projects.config_for_pane(11).is_some(),
        "the pane has to resolve to a project, or its theme falls back to the default"
    );
    // Stacked commands are panes to the registry: their own id, view and theme.
    assert_eq!(
        projects.root_for_pane(tab.panes[0].stack.entries[0].id),
        Some(&root),
        "a stacked command must inherit the project too"
    );
    // Nothing to inherit is not an error: a window with no project leaves the
    // incoming panes as they were.
    let mut empty = crate::project_context::ProjectState::new(
        crate::project::ProjectRegistry::load_from(temporary.path().join("empty.json")).unwrap(),
    );
    inherit_project_for_panes(&mut empty, 7, &tab);
    assert!(empty.root_for_pane(11).is_none());
}

#[test]
fn a_viewer_reports_no_size_until_its_pane_is_laid_out() {
    use terminal::TerminalBounds;

    // The placeholder a `TerminalContent` starts with. Reporting it made a window
    // that had only just joined claim six rows, and because a shared pane has to
    // fit inside every viewer, the window that had been showing it at fifty-one was
    // resized down to match — which is what a joining window did to both of them.
    assert_eq!(TerminalBounds::default().num_lines(), 6);
    assert_eq!(shared_size_to_report(TerminalBounds::default()), None);

    let laid_out = TerminalBounds::new(
        gpui::px(10.),
        gpui::px(8.),
        gpui::Bounds {
            origin: gpui::Point::default(),
            size: gpui::Size {
                width: gpui::px(98. * 8.),
                height: gpui::px(51. * 10.),
            },
        },
    );
    assert_eq!(shared_size_to_report(laid_out), Some((98, 51)));
}

#[test]
fn an_arbitrated_size_is_only_applied_when_it_differs() {
    use terminal::TerminalBounds;

    let bounds = |columns: f32, lines: f32| {
        TerminalBounds::new(
            gpui::px(10.),
            gpui::px(8.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::Size {
                    width: gpui::px(columns * 8.),
                    height: gpui::px(lines * 10.),
                },
            },
        )
    };

    // Two windows tiled to the same size by a compositor is the common case, and
    // resizing one of them to the size it already had moves the user's window for
    // no reason at all.
    assert_eq!(
        shared_size_action(Some(bounds(98., 51.)), 98, 51),
        SharedSizeAction::AlreadyMatches
    );
    // A viewer larger than the arbitrated size has to shrink, or the shell's
    // wrapping stops lining up with the cells drawn.
    assert_eq!(
        shared_size_action(Some(bounds(120., 51.)), 98, 51),
        SharedSizeAction::Resize
    );
    assert_eq!(
        shared_size_action(Some(bounds(98., 60.)), 98, 51),
        SharedSizeAction::Resize
    );

    // Before the first layout a terminal reports the placeholder a
    // `TerminalContent` starts with. Treating that as the window's real size is
    // what resized a window that already matched: the placeholder is 100x6, so
    // 98x51 looked like a large change.
    assert_eq!(
        TerminalBounds::default().num_columns(),
        100,
        "the placeholder this guards against"
    );
    assert_eq!(TerminalBounds::default().num_lines(), 6);
    assert_eq!(
        shared_size_action(Some(TerminalBounds::default()), 98, 51),
        SharedSizeAction::WaitForLayout
    );
    // No terminal yet is the same answer: wait, and stay pending.
    assert_eq!(
        shared_size_action(None, 98, 51),
        SharedSizeAction::WaitForLayout
    );
}

/// A stand-in subscriber, in place of a real `Zetta` — `Zetta::new` opens a tab,
/// which spawns a shell.
struct SizeWatcher;

impl gpui::Render for SizeWatcher {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// Watching a terminal's size must not keep the terminal alive.
///
/// GPUI stores a subscription on the *emitter* and drops it only when the
/// emitter is released, so a closure that captures its own emitter is a cycle
/// nothing can break. A shared pane's terminal caught in one never stops its
/// byte stream when its tab closes, so the relay socket stays open and the
/// multiplexer keeps counting this window among the pane's viewers — which is
/// how unsharing came to be refused for a window that had closed the tab.
#[gpui::test]
async fn watching_a_terminals_size_does_not_keep_it_alive(cx: &mut gpui::TestAppContext) {
    let terminal = cx.update(|cx| {
        cx.new(|cx| {
            terminal::TerminalBuilder::new_display_only(
                terminal::terminal_settings::CursorShape::Block,
                terminal::terminal_settings::AlternateScroll::On,
                None,
                0,
                cx.background_executor(),
                util::paths::PathStyle::local(),
            )
            .subscribe(cx)
        })
    });
    let weak = terminal.downgrade();
    let (watcher, window) = cx.add_window_view(|_, _| SizeWatcher);
    watcher.update_in(window, |_, window, cx| {
        watch_grid_size(&terminal, window, cx, |_, _, _| {});
    });

    drop(terminal);
    window.run_until_parked();

    assert!(
        weak.upgrade().is_none(),
        "the subscription must not be the terminal's last owner"
    );
}
