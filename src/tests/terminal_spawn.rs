use super::*;
use crate::config::PaneSplitCommand;

/// The exact race a rapid new-tab-then-close can hit: the tab is gone before
/// the pane's spawn resolves and the multiplexer tells this process about it.
/// Without releasing it here, the pane stays marked as held by this process
/// forever — see `Zetta::release_mux_pane` and `finish_terminal_spawn`'s
/// orphan branch.
#[gpui::test]
fn a_spawn_that_resolves_after_its_tab_closed_releases_the_mux_pane(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = crate::config::Config::defaults(None, None);
        // An empty profile list keeps `Zetta::new` from opening its own tab,
        // so the only spawn in this test is the one it drives below.
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

    #[cfg(not(windows))]
    let command = Shell::Program("true".to_owned());
    #[cfg(windows)]
    let command = Shell::WithArguments {
        program: "cmd.exe".to_owned(),
        args: vec!["/c".to_owned(), "exit".to_owned()],
        title_override: None,
    };
    let profile = Profile {
        name: "Test".to_owned(),
        command,
        theme: None,
        dark_theme: None,
        icon: ProfileIcon::Zetta,
    };

    let pane_id = zetta.update_in(cx, |zetta, window, cx| {
        zetta.open_tab_with_profile(profile, window, cx);
        let tab = zetta.tabs.last().expect("the tab was just opened");
        let tab_id = tab.id;
        let pane_id = tab.active_pane;
        // The tab closes — and, standing in for the multiplexer telling this
        // process about the pane, which in the real race arrives only after
        // the tab is already gone — the pane is recorded as held. Neither
        // step goes through the normal close path, which would release it
        // itself; the point is to reach `finish_terminal_spawn` with a pane
        // that is tracked but belongs to no live tab.
        zetta.tabs.retain(|tab| tab.id != tab_id);
        zetta.mux_panes.record(pane_id, 900);
        pane_id
    });

    // Lets the still in-flight spawn resolve and `finish_terminal_spawn` run
    // against the state set up above.
    cx.run_until_parked();

    zetta.update(cx, |zetta, _cx| {
        assert_eq!(
            zetta.mux_panes.mux_pane_id(pane_id),
            None,
            "a spawn resolving after its tab closed must release the pane back to the \
             multiplexer instead of leaking it as held by this process forever"
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
    assert!(
        !environment.contains_key("ZETTA_HOST_EXECUTABLE"),
        "the Windows-side executable path is forwarded by WSLENV, not inherited"
    );
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
