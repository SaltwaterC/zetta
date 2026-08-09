use super::*;

#[cfg(windows)]
fn msys2_shell(root: &Path, shell: &str) -> Shell {
    Shell::WithArguments {
        program: "cmd.exe".to_owned(),
        args: vec![
            "/d".to_owned(),
            "/s".to_owned(),
            "/c".to_owned(),
            format!(
                "\"\"{}\" -defterm -here -no-start -msys -use-full-path -shell {shell}\"",
                root.join("msys2_shell.cmd").display()
            ),
        ],
        title_override: None,
    }
}

#[cfg(windows)]
#[test]
fn recognizes_detected_msys2_profiles_and_their_custom_root() {
    let root = Path::new(r"D:\Applications with spaces\MSYS2");

    assert_eq!(
        msys2_profile(&msys2_shell(root, "bash")),
        Some((root.to_path_buf(), Msys2Shell::Bash))
    );
    assert_eq!(
        msys2_profile(&msys2_shell(root, "zsh")),
        Some((root.to_path_buf(), Msys2Shell::Zsh))
    );
}

#[cfg(windows)]
#[test]
fn translates_windows_paths_for_msys2_editors() {
    assert_eq!(
        windows_path_to_msys(Path::new(r"C:\Users\saltw\source\repos\zetta\AGENTS.md")),
        Some("/c/Users/saltw/source/repos/zetta/AGENTS.md".to_owned())
    );
}

#[cfg(windows)]
#[test]
fn converts_reported_msys2_directories_to_native_windows_paths() {
    let root = Path::new(r"D:\Applications\MSYS2");

    assert_eq!(
        msys2_path_to_windows(root, "/c/Users/saltw/source/zetta"),
        Some(PathBuf::from(r"C:\Users\saltw\source\zetta"))
    );
    assert_eq!(
        msys2_path_to_windows(root, "/home/saltw/project"),
        Some(root.join("home").join("saltw").join("project"))
    );
    assert_eq!(
        msys2_path_to_windows(root, "//server/share/project"),
        Some(PathBuf::from(r"\\server\share\project"))
    );
    assert_eq!(msys2_path_to_windows(root, "/c/../Windows"), None);
    assert_eq!(msys2_path_to_windows(root, "relative/path"), None);
}

#[cfg(windows)]
#[test]
fn configures_bash_to_report_prompt_directories_and_foreground_commands() {
    let environment = msys2_cwd_tracking_environment(
        &msys2_shell(Path::new(r"C:\msys64"), "bash"),
        7,
        Path::new(r"C:\Temp"),
    )
    .unwrap();

    assert_eq!(environment.len(), 1);
    assert_eq!(environment[0].0, "PROMPT_COMMAND");
    assert!(environment[0].1.contains("zetta-cwd:%s"));
    assert!(environment[0].1.contains("\"$PWD\""));
    assert!(environment[0].1.contains("trap '__zetta_preexec' DEBUG"));
    assert!(environment[0].1.contains("__zetta_at_prompt=0"));
    assert!(environment[0].1.contains("__zetta_at_prompt=1"));
    assert!(environment[0].1.contains("zetta-cmd:%s"));
    assert!(environment[0].1.contains("zetta-cmd:bash"));
}

#[cfg(windows)]
#[test]
fn configures_zsh_to_report_directories_and_commands_without_changing_user_files() {
    let temporary = tempfile::tempdir().unwrap();
    let environment = msys2_cwd_tracking_environment(
        &msys2_shell(Path::new(r"C:\msys64"), "zsh"),
        11,
        temporary.path(),
    )
    .unwrap();
    let integration_directory = environment
        .iter()
        .find_map(|(name, value)| (name == "ZDOTDIR").then_some(value))
        .unwrap();
    let native_directory =
        msys2_path_to_windows(Path::new(r"C:\msys64"), integration_directory).unwrap();
    let integration = fs::read_to_string(native_directory.join(".zshenv")).unwrap();

    assert!(integration.contains("add-zsh-hook precmd __zetta_report_cwd"));
    assert!(integration.contains("add-zsh-hook preexec __zetta_report_preexec"));
    assert!(integration.contains("zetta-cwd:%s"));
    assert!(integration.contains("zetta-cmd:%s"));
    assert!(integration.contains("zetta-cmd:zsh"));
    assert!(integration.contains("source \"$original_zdotdir/.zshenv\""));
}

#[test]
fn wsl_home_is_applied_to_detected_wsl_commands() {
    let shell = Shell::WithArguments {
        program: "C:\\Windows\\System32\\wsl.exe".to_owned(),
        args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
        title_override: Some("WSL: Ubuntu".to_owned()),
    };

    assert!(is_wsl_shell(&shell));
    assert!(matches!(
        wsl_shell_with_tracking(shell, Some("~"), None),
        Shell::WithArguments { args, title_override, .. }
            if args == ["--distribution", "Ubuntu", "--cd", "~"]
                && title_override.as_deref() == Some("WSL: Ubuntu")
    ));
}

#[test]
fn native_shells_are_not_treated_as_wsl() {
    assert!(!is_wsl_shell(&Shell::Program("pwsh.exe".to_owned())));
}

#[test]
fn shares_attention_target_environment_with_wsl() {
    let mut environment = HashMap::from([
        ("ZETTA_PROCESS_ID".to_owned(), "123".to_owned()),
        ("ZETTA_ATTENTION_ID".to_owned(), "456".to_owned()),
        ("WSLENV".to_owned(), String::new()),
    ]);

    add_wsl_environment_variables(&mut environment);

    assert_eq!(
        environment.get("WSLENV").map(String::as_str),
        Some("ZETTA_PROCESS_ID/u:ZETTA_ATTENTION_ID/u")
    );
}

#[test]
fn preserves_existing_wslenv_entries_without_duplicates() {
    let mut environment = HashMap::from([(
        "WSLENV".to_owned(),
        "PATH/l:ZETTA_PROCESS_ID/l:USER/u".to_owned(),
    )]);

    add_wsl_environment_variables(&mut environment);

    assert_eq!(
        environment.get("WSLENV").map(String::as_str),
        Some("PATH/l:ZETTA_PROCESS_ID/l:USER/u:ZETTA_ATTENTION_ID/u")
    );
}

#[test]
fn explicit_wsl_directory_is_not_overridden() {
    let shell = Shell::WithArguments {
        program: "wsl.exe".to_owned(),
        args: vec!["--cd".to_owned(), "/work".to_owned()],
        title_override: None,
    };

    assert!(matches!(
        wsl_shell_with_tracking(shell, Some("~"), None),
        Shell::WithArguments { args, .. } if args == ["--cd", "/work"]
    ));
}

#[test]
fn wsl_ignores_the_windows_side_inherited_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: None,
        },
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("~"));
}

#[test]
fn explicitly_configured_home_alias_still_uses_the_wsl_home() {
    let config = Config::parse(r#"{"working_directory":"~"}"#, None, None).unwrap();
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        config.working_directory,
        config.working_directory_configured,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("~"));
}

#[test]
fn native_profiles_still_inherit_the_active_directory() {
    let profile = Profile {
        name: "PowerShell".to_owned(),
        command: Shell::Program("pwsh.exe".to_owned()),
        theme: None,
    };
    let inherited = PathBuf::from(r"C:\source\zetta");

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(inherited.clone()),
        None,
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, Some(inherited));
    assert_eq!(wsl_directory, None);
}

#[test]
fn configured_directory_overrides_the_windows_side_wsl_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };
    let configured = PathBuf::from(r"C:\Users\stefan");

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        None,
        Some(configured.clone()),
        true,
    );

    assert_eq!(directory, Some(configured));
    assert_eq!(wsl_directory, None);
}

#[test]
fn tracked_wsl_directory_takes_precedence_over_the_initial_configuration() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        None,
        Some("/work".to_owned()),
        Some(PathBuf::from(r"C:\Users\stefan")),
        true,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("/work"));
}

#[test]
fn wsl_inherits_the_tracked_linux_directory() {
    let profile = Profile {
        name: "WSL: Ubuntu".to_owned(),
        command: Shell::Program("wsl.exe".to_owned()),
        theme: None,
    };

    let (directory, wsl_directory) = launch_working_directory(
        &profile,
        Some(PathBuf::from(r"C:\source\zetta")),
        Some("/home/stefan/source/zetta".to_owned()),
        Some(PathBuf::from(r"C:\Users\stefan")),
        false,
    );

    assert_eq!(directory, None);
    assert_eq!(wsl_directory.as_deref(), Some("/home/stefan/source/zetta"));
}

#[test]
fn wsl_tracker_wraps_the_default_login_shell() {
    let marker = Path::new(r"C:\Users\stefan\AppData\Local\Temp\zetta-cwd");
    let shell = wsl_shell_with_tracking(
        Shell::WithArguments {
            program: "wsl.exe".to_owned(),
            args: vec!["--distribution".to_owned(), "Ubuntu".to_owned()],
            title_override: None,
        },
        Some("/work"),
        Some(marker),
    );

    assert!(matches!(
        shell,
        Shell::WithArguments { args, .. }
            if args[..4] == ["--distribution", "Ubuntu", "--cd", "/work"]
                && args[4..8] == ["--exec", "/bin/sh", "-c", WSL_CWD_TRACKER]
                && args.last().map(String::as_str) == marker.to_str()
    ));
}

#[test]
fn wsl_wrapper_prefers_prompt_cwd_reports_and_keeps_a_shell_fallback() {
    assert!(WSL_CWD_TRACKER.contains("PROMPT_COMMAND="));
    assert!(WSL_CWD_TRACKER.contains("--on-event fish_prompt"));
    assert!(WSL_CWD_TRACKER.contains("add-zsh-hook precmd __zetta_report_cwd"));
    assert!(WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshenv\""));
    assert!(WSL_CWD_TRACKER.contains("rm -rf -- \"$ZETTA_INTEGRATION_ZDOTDIR\""));
    assert!(!WSL_CWD_TRACKER.contains("source \"$ZDOTDIR/.zshrc\""));
    assert!(WSL_CWD_TRACKER.contains("]7;file://localhost"));
    assert!(WSL_CWD_TRACKER.contains("]2;zetta-cwd:"));
    assert!(WSL_CWD_TRACKER.contains("readlink \"/proc/$parent/cwd\""));

    // Windows-side process inspection can't see into the WSL VM's own process
    // namespace, so the running command is reported explicitly by each shell's
    // preexec-equivalent hook via the `zetta-cmd:` title marker.
    assert!(WSL_CWD_TRACKER.contains("trap '__zetta_preexec' DEBUG"));
    assert!(WSL_CWD_TRACKER.contains("--on-event fish_preexec"));
    assert!(WSL_CWD_TRACKER.contains("add-zsh-hook preexec __zetta_report_preexec"));
    assert!(WSL_CWD_TRACKER.contains("]2;zetta-cmd:"));
}
