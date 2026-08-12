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

    apply_terminal_environment_overrides(&mut environment, &overrides, 42, 7);

    assert_eq!(environment["PATH"], "custom-path");
    assert_eq!(environment["ROLE"], "server");
    assert_eq!(environment["ZETTA_HOST_EXECUTABLE"], "host");
    assert_eq!(environment["ZETTA_PROCESS_ID"], "42");
    assert_eq!(environment["ZETTA_ATTENTION_ID"], "7");
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
