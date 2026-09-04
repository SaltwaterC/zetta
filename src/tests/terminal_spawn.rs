use super::*;
use crate::config::PaneSplitCommand;

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
