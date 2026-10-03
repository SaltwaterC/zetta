use super::*;
use crate::config::PaneSplitCommand;

#[cfg(feature = "zmux")]
#[test]
fn complete_shared_batch_layout_preserves_template_axes_ratios_and_drafts() {
    let layout = PaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 200,
        first: Box::new(PaneLayout::Pane(1)),
        second: Box::new(PaneLayout::Split {
            axis: SplitAxis::Horizontal,
            first_ratio: 420,
            first: Box::new(PaneLayout::Pane(2)),
            second: Box::new(PaneLayout::Pane(3)),
        }),
    };
    let drafts = HashSet::from([2, 3]);
    let mapped =
        shared_complete_draft_layout(&layout, &drafts, &HashMap::from([(1, 101)])).unwrap();

    let zmux::messages::SharedDraftLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = mapped
    else {
        panic!("template root was not preserved")
    };
    assert_eq!(axis, "vertical");
    assert_eq!(first_ratio, 200);
    assert!(matches!(
        *first,
        zmux::messages::SharedDraftLayout::Existing { pane_id: 101 }
    ));
    let zmux::messages::SharedDraftLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = *second
    else {
        panic!("nested template split was not preserved")
    };
    assert_eq!(axis, "horizontal");
    assert_eq!(first_ratio, 420);
    assert!(matches!(
        *first,
        zmux::messages::SharedDraftLayout::Draft { draft_id: 2 }
    ));
    assert!(matches!(
        *second,
        zmux::messages::SharedDraftLayout::Draft { draft_id: 3 }
    ));
}

/// A pane that finishes spawning after its tab closes must be released, not
/// left marked as held by this process forever.
#[cfg(feature = "zmux")]
#[gpui::test]
fn an_orphaned_terminal_spawn_releases_its_mux_pane(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = crate::config::Config::defaults(None, None);
        // An empty profile list keeps `Zetta::new` from opening its own tab.
        config.profiles.clear();
        crate::app::Zetta::new(
            config,
            None,
            crate::app::ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        )
    });

    let (tab_id, pane_id) = (17, 23);

    // Drive the completion cleanup directly rather than depending on a
    // platform shell/PTY launch to finish at a particular point in the test
    // scheduler.
    zetta.update_in(cx, |zetta, _window, cx| {
        zetta.mux_panes.record(pane_id, 900);
        zetta.release_orphaned_terminal_spawn(tab_id, pane_id, None, cx);
    });

    zetta.update(cx, |zetta, _cx| {
        assert_eq!(
            zetta.mux_panes.mux_pane_id(pane_id),
            None,
            "an orphaned terminal spawn must release its pane back to the multiplexer"
        );
    });
}

#[test]
fn native_stacked_commands_use_one_interactive_shell_command() {
    let shell = stacked_task_shell(&Shell::Program("bash".to_owned()), "echo {one,two}", None);

    assert_eq!(
        shell,
        Shell::WithArguments {
            program: "bash".to_owned(),
            args: vec![
                "-i".to_owned(),
                "-c".to_owned(),
                "echo {one,two}".to_owned()
            ],
            title_override: None,
        }
    );
}

#[cfg(not(windows))]
#[test]
fn native_shell_bootstrap_loads_path_integration_only_when_needed() {
    let command = String::from_utf8(
        shell_integration_startup_command(&Shell::Program("zsh".to_owned())).unwrap(),
    )
    .unwrap();
    assert!(command.starts_with(
        "if [[ ${__ZETTA_LIFECYCLE_TRACKING_VERSION:-0} != 3 || ( -n ${ZETTA_PANE_ROUTING_ID:-${ZETTA_PANE_ID:-}} && ${__ZETTA_LIFECYCLE_TRACKING_ENABLED:-0} != 1 ) ]]; then "
    ));
    assert!(command.contains(r#"eval "$(command zetta init zsh)"; fi"#));
    assert!(!command.contains("ZETTA_HOST_EXECUTABLE"));
    assert!(command.ends_with('\r'));
    assert!(
        shell_integration_startup_command(&Shell::WithArguments {
            program: "zsh".to_owned(),
            args: vec!["-i".to_owned(), "-c".to_owned(), "make lint".to_owned()],
            title_override: None,
        })
        .is_none()
    );
}

#[cfg(windows)]
#[test]
fn powershell_integration_is_not_typed_into_the_terminal() {
    for program in ["powershell.exe", "pwsh.exe"] {
        assert!(
            shell_integration_startup_command(&Shell::Program(program.to_owned())).is_none(),
            "{program} loads its integration before the first prompt"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn native_zsh_history_filter_is_installed_before_startup_command() {
    let mut environment = HashMap::from([
        ("HOME".to_owned(), "/home/tester".to_owned()),
        ("ZDOTDIR".to_owned(), "/home/tester/config".to_owned()),
    ]);

    configure_zsh_history_environment(
        &Shell::Program("zsh".to_owned()),
        &mut environment,
        987654321,
    )
    .unwrap();

    let directory = PathBuf::from(&environment["ZETTA_ZSH_HISTORY_ZDOTDIR"]);
    let script = fs::read_to_string(directory.join(".zshenv")).unwrap();
    assert!(script.contains("zshaddhistory"));
    assert!(script.contains("fc -p"));
    assert_eq!(environment["ZDOTDIR"], directory.to_str().unwrap());
    assert_eq!(
        environment["ZETTA_ZSH_ORIGINAL_ZDOTDIR"],
        "/home/tester/config"
    );
    assert_eq!(environment["ZETTA_ZSH_ORIGINAL_ZDOTDIR_SET"], "1");

    fs::remove_file(directory.join(".zshenv")).unwrap();
    fs::remove_dir(directory).unwrap();
}

#[test]
fn pane_template_environment_overrides_merge_without_replacing_zetta_variables() {
    let mut environment = HashMap::from([
        ("PATH".to_owned(), "base-path".to_owned()),
        ("ZETTA_HOST_EXECUTABLE".to_owned(), "host".to_owned()),
    ]);
    let overrides = HashMap::from([
        ("PATH".to_owned(), "custom-path".to_owned()),
        ("ROLE".to_owned(), "server".to_owned()),
        ("ZETTA_PROCESS_ID".to_owned(), "spoofed".to_owned()),
    ]);

    apply_terminal_environment_overrides(&mut environment, &overrides, 42, 7, 9, 11, false);

    assert_eq!(environment["PATH"], "custom-path");
    assert_eq!(environment["ROLE"], "server");
    assert_eq!(environment["ZETTA_HOST_EXECUTABLE"], "host");
    assert_eq!(environment["ZETTA_PROCESS_ID"], "42");
    assert_eq!(environment["ZETTA_ATTENTION_ID"], "7");
    assert_eq!(environment["ZETTA_PANE_ID"], "9");
    assert_eq!(environment["ZETTA_PANE_ROUTING_ID"], "11");
    assert_eq!(environment["ZETTA_NO_MUX"], "0");
}

#[test]
fn no_mux_terminal_environment_is_explicit_and_cannot_be_overridden() {
    let mut environment = HashMap::new();
    let overrides = HashMap::from([("ZETTA_NO_MUX".to_owned(), "0".to_owned())]);

    apply_terminal_environment_overrides(&mut environment, &overrides, 42, 7, 9, 11, true);

    assert_eq!(environment["ZETTA_NO_MUX"], "1");
}

/// The interactive pane and the stacked command terminal build their
/// environment through the same [`TerminalEnvironment`], so the identity a
/// terminal reports to shell integration is the tracked id the caller passed
/// — the pane id for the former, the stack entry id for the latter.
#[test]
fn terminal_environment_carries_the_tracked_identity_and_theme() {
    let overrides = HashMap::from([
        ("ROLE".to_owned(), "server".to_owned()),
        ("ZETTA_ATTENTION_ID".to_owned(), "spoofed".to_owned()),
    ]);
    let environment: HashMap<String, String> = TerminalEnvironment {
        profile: &Shell::Program("bash".to_owned()),
        overrides: &overrides,
        attention_id: 7,
        tracking_id: 9,
        routing_id: 11,
        wsl_cwd_file: None,
        theme_name: "One Dark",
        no_mux: false,
    }
    .build()
    .expect("a native profile needs no CWD-tracking setup");

    assert_eq!(environment["ROLE"], "server");
    assert_eq!(environment["ZETTA_ATTENTION_ID"], "7");
    assert_eq!(environment["ZETTA_PANE_ID"], "9");
    assert_eq!(environment["ZETTA_PANE_ROUTING_ID"], "11");
    assert_eq!(environment["ZETTA_THEME"], "One Dark");
    assert_eq!(
        environment["ZETTA_PROCESS_ID"],
        std::process::id().to_string()
    );
    assert!(
        environment.contains_key("ZETTA_HOST_EXECUTABLE"),
        "a native terminal inherits this process's environment"
    );
}

/// A WSL profile starts from an empty environment rather than this process's:
/// the Windows-side variables mean nothing inside the distribution, so only
/// the ones WSL is told to forward may cross.
#[test]
fn wsl_terminal_environment_does_not_inherit_the_native_environment() {
    let overrides = HashMap::from([("ROLE".to_owned(), "server".to_owned())]);
    let environment: HashMap<String, String> = TerminalEnvironment {
        profile: &Shell::Program("wsl.exe".to_owned()),
        overrides: &overrides,
        attention_id: 7,
        tracking_id: 9,
        routing_id: 11,
        wsl_cwd_file: None,
        theme_name: "One Dark",
        no_mux: false,
    }
    .build()
    .expect("a WSL profile needs no CWD-tracking setup");

    assert_eq!(environment["ROLE"], "server");
    assert_eq!(environment["ZETTA_PANE_ID"], "9");
    assert!(
        !environment.contains_key("PATH"),
        "the Windows-side PATH must not reach the distribution"
    );
    #[cfg(windows)]
    {
        assert!(environment.contains_key("ZETTA_HOST_EXECUTABLE"));
        assert!(
            environment["WSLENV"]
                .split(':')
                .any(|entry| entry == "ZETTA_HOST_EXECUTABLE/up"),
            "the Windows-side executable path must reach WSL through WSLENV"
        );
    }
    #[cfg(not(windows))]
    assert!(!environment.contains_key("ZETTA_HOST_EXECUTABLE"));
}

#[test]
fn pane_template_commands_preserve_the_program_and_argument_boundaries() {
    let command = PaneSplitCommand {
        program: "ssh".to_owned(),
        args: vec![
            "host name".to_owned(),
            "--identity".to_owned(),
            "key file".to_owned(),
        ],
    };

    assert_eq!(
        command.shell(),
        Shell::WithArguments {
            program: "ssh".to_owned(),
            args: vec![
                "host name".to_owned(),
                "--identity".to_owned(),
                "key file".to_owned()
            ],
            title_override: None,
        }
    );
}

#[test]
fn wsl_stacked_commands_preserve_profile_and_working_directory_arguments() {
    let shell = Shell::WithArguments {
        program: "wsl.exe".to_owned(),
        args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
        title_override: Some("WSL: Ubuntu".to_owned()),
    };

    assert_eq!(
        stacked_task_shell(&shell, "printf hello", Some("/work")),
        Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec![
                "--distribution".to_owned(),
                "Ubuntu".to_owned(),
                "--cd".to_owned(),
                "/work".to_owned(),
                "--exec".to_owned(),
                "/bin/sh".to_owned(),
                "-i".to_owned(),
                "-c".to_owned(),
                "printf hello".to_owned(),
            ],
            title_override: Some("WSL: Ubuntu".to_owned()),
        }
    );
}

#[cfg(windows)]
#[test]
fn msys2_stacked_commands_use_the_profile_shell_inside_the_pty() {
    let root = Path::new(r"C:\msys64");
    let profile = Shell::WithArguments {
        program: "cmd.exe".to_owned(),
        args: vec![
            "/d".to_owned(),
            "/s".to_owned(),
            "/c".to_owned(),
            format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell bash\"",
                root.join("msys2_shell.cmd").display()
            ),
        ],
        title_override: None,
    };
    let Shell::WithArguments { program, args, .. } = stacked_task_shell(&profile, "pwd", None)
    else {
        panic!("MSYS2 stacked command should use explicit shell arguments");
    };

    assert_eq!(
        program,
        root.join("usr")
            .join("bin")
            .join("bash.exe")
            .display()
            .to_string()
    );
    assert_eq!(args, ["-i", "-c", "pwd"]);
}

#[cfg(windows)]
#[test]
fn cygwin_stacked_commands_use_the_direct_profile_shell() {
    let profile = Shell::WithArguments {
        program: r"C:\cygwin64\bin\zsh.exe".to_owned(),
        args: vec!["-l".to_owned()],
        title_override: Some("Cygwin: Zsh".to_owned()),
    };
    let Shell::WithArguments { program, args, .. } = stacked_task_shell(&profile, "pwd", None)
    else {
        panic!("Cygwin stacked command should use the direct shell executable");
    };

    assert_eq!(program, r"C:\cygwin64\bin\zsh.exe");
    assert_eq!(args, ["-l", "-i", "-c", "pwd"]);
}

/// A pane in a shared session is started by the daemon, on the daemon's host.
/// Resolving the profile here and shipping the result is only correct when
/// that host is this machine; doing it for a remote one is what asked macOS to
/// run the Linux client's `$SHELL` with a Linux `PATH` in front of it.
#[cfg(feature = "zmux")]
#[test]
fn a_remote_shared_draft_sends_no_command_and_no_local_environment() {
    let shell = Shell::WithArguments {
        program: "/usr/bin/zsh".to_owned(),
        args: vec!["-l".to_owned()],
        title_override: Some("Zsh".to_owned()),
    };
    let environment = HashMap::from([
        ("PATH".to_owned(), "/usr/local/bin:/usr/bin".to_owned()),
        ("HOME".to_owned(), "/home/someone".to_owned()),
        ("ZETTA_PANE_ROUTING_ID".to_owned(), "7".to_owned()),
        ("ZETTA_ATTENTION_ID".to_owned(), "3".to_owned()),
    ]);

    let (command, env) = shared_draft_process(true, &shell, &environment);

    assert_eq!(
        command, None,
        "the host that runs the pane resolves the profile name itself"
    );
    assert_eq!(
        env,
        HashMap::from([
            ("ZETTA_PANE_ROUTING_ID".to_owned(), "7".to_owned()),
            ("ZETTA_ATTENTION_ID".to_owned(), "3".to_owned()),
        ]),
        "only the pane's routing identity crosses; the rest of the environment \
         describes the wrong machine"
    );
}

/// The same window and the same machine: this resolution *is* the host's, and
/// it carries the working-directory tracking wrappers a profile name alone
/// cannot express.
#[cfg(feature = "zmux")]
#[test]
fn a_local_shared_draft_sends_the_command_it_resolved() {
    let shell = Shell::WithArguments {
        program: "cmd.exe".to_owned(),
        args: vec!["/c".to_owned(), "msys2_shell.cmd".to_owned()],
        title_override: Some("MSYS2".to_owned()),
    };
    let environment = HashMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]);

    let (command, env) = shared_draft_process(false, &shell, &environment);

    assert_eq!(
        command,
        Some(zetta_profiles::ProfileCommand::with_args(
            "cmd.exe",
            vec!["/c".to_owned(), "msys2_shell.cmd".to_owned()],
        ))
    );
    assert_eq!(env, environment, "same machine, same environment");
}

/// A window proposes geometry in the session's pane ids, translated from its
/// own. The session refuses the whole proposal if one of them names a pane it
/// no longer holds, so the ids have to be collectable before it is sent.
#[cfg(feature = "zmux")]
#[test]
fn a_proposed_layout_reports_every_existing_pane_it_names() {
    let layout = zmux::messages::SharedDraftLayout::Split {
        axis: "vertical".to_owned(),
        first_ratio: 200,
        first: Box::new(zmux::messages::SharedDraftLayout::Existing { pane_id: 41 }),
        second: Box::new(zmux::messages::SharedDraftLayout::Split {
            axis: "horizontal".to_owned(),
            first_ratio: 300,
            first: Box::new(zmux::messages::SharedDraftLayout::Existing { pane_id: 42 }),
            second: Box::new(zmux::messages::SharedDraftLayout::Draft { draft_id: 7 }),
        }),
    };

    let mut named = Vec::new();
    shared_draft_layout_existing_ids(&layout, &mut named);

    assert_eq!(
        named,
        vec![41, 42],
        "a draft has no id in the session yet, so only the existing panes are named"
    );
}

/// A display-only terminal in a view broadcasting its input, with the input
/// events it emits collected in order. Stands in for a pane whose tab has
/// broadcast input on, without a `Zetta` (which would spawn a shell).
struct BroadcastingView {
    terminal: Entity<terminal::Terminal>,
    view: Entity<TerminalView>,
    received: std::rc::Rc<std::cell::RefCell<Vec<TerminalInput>>>,
}

fn broadcasting_terminal_view(
    cx: &mut gpui::TestAppContext,
) -> (BroadcastingView, &mut gpui::VisualTestContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
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
    let (view, cx) = cx.add_window_view({
        let terminal = terminal.clone();
        |window, cx| TerminalView::new_with_theme(terminal, None, window, cx)
    });
    let received = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    view.update_in(cx, |view, window, cx| {
        view.set_emit_input_events(true);
        window.focus(&view.focus_handle(cx), cx);
    });
    cx.update({
        let received = received.clone();
        |_, cx| {
            cx.subscribe(&view, move |_, event, _| {
                if let TerminalViewEvent::Input(input) = event {
                    received.borrow_mut().push(input.clone());
                }
            })
            .detach();
        }
    });
    cx.run_until_parked();
    let pane = BroadcastingView {
        terminal,
        view,
        received,
    };
    (pane, cx)
}

#[gpui::test]
fn a_paste_waiting_on_the_clipboard_is_broadcast_ahead_of_later_typing(
    cx: &mut gpui::TestAppContext,
) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("pasted".to_owned()));
    cx.defer_clipboard_reads(true);
    let (
        BroadcastingView {
            terminal,
            view,
            received,
        },
        cx,
    ) = broadcasting_terminal_view(cx);

    cx.dispatch_action(terminal::Paste);
    view.update(cx, |view, cx| {
        assert!(view.forward_keystroke(&gpui::Keystroke::parse("enter").unwrap(), cx));
    });
    cx.run_until_parked();
    assert!(
        received.borrow().is_empty(),
        "typing overtook a paste still reading the clipboard: {:?}",
        received.borrow()
    );
    assert!(terminal.read_with(cx, |terminal, _| terminal.paste_pending()));

    // The clipboard changes hands before the owner answers: the paste is of
    // what was on the clipboard when it was asked for.
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("replaced".to_owned()));
    assert_eq!(cx.complete_clipboard_reads(), 1);
    cx.run_until_parked();

    let received = received.borrow();
    assert!(
        matches!(received.as_slice(), [TerminalInput::Paste(text), typed]
            if text == "pasted" && !matches!(typed, TerminalInput::Paste(_))),
        "{received:?}"
    );
    assert!(!terminal.read_with(cx, |terminal, _| terminal.paste_pending()));
}

#[gpui::test]
fn a_paste_aimed_at_a_search_that_closes_while_reading_goes_nowhere(cx: &mut gpui::TestAppContext) {
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("pasted".to_owned()));
    cx.defer_clipboard_reads(true);
    let (
        BroadcastingView {
            terminal, received, ..
        },
        cx,
    ) = broadcasting_terminal_view(cx);

    cx.dispatch_action(terminal_view::SearchScrollback);
    cx.dispatch_action(terminal::Paste);
    cx.dispatch_action(terminal_view::DismissSearch);
    assert_eq!(cx.complete_clipboard_reads(), 1);
    cx.run_until_parked();

    assert!(received.borrow().is_empty(), "{:?}", received.borrow());
    assert!(!terminal.read_with(cx, |terminal, _| terminal.paste_pending()));
}
