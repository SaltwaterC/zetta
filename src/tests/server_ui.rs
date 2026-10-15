use super::*;

#[test]
fn wsl_path_query_preserves_distribution_and_user() {
    let command = Shell::WithArguments {
        program: "wsl.exe".to_owned(),
        args: vec![
            "--distribution".to_owned(),
            "Ubuntu".to_owned(),
            "--user".to_owned(),
            "developer".to_owned(),
        ],
        title_override: None,
    };

    assert_eq!(
        wsl_path_command(&command, "/home/developer/project"),
        Some((
            "wsl.exe".to_owned(),
            vec![
                "--distribution",
                "Ubuntu",
                "--user",
                "developer",
                "--exec",
                "wslpath",
                "-w",
                "/home/developer/project",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        ))
    );
}

#[test]
fn wsl_path_query_replaces_shell_launch_and_initial_directory() {
    let command = Shell::WithArguments {
        program: "wsl.exe".to_owned(),
        args: vec![
            "-d".to_owned(),
            "Ubuntu".to_owned(),
            "--cd=/old".to_owned(),
            "--exec".to_owned(),
            "/bin/zsh".to_owned(),
        ],
        title_override: None,
    };

    assert_eq!(
        wsl_path_command(&command, "/work"),
        Some((
            "wsl.exe".to_owned(),
            vec!["-d", "Ubuntu", "--exec", "wslpath", "-w", "/work"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        ))
    );
}

/// Serving a directory publishes its files to the LAN, so a `zetta-cwd` the
/// pane's shell did not sign cannot choose it.
#[gpui::test]
fn a_file_server_serves_only_a_directory_the_shell_confirmed(cx: &mut gpui::TestAppContext) {
    use crate::pane::tests::{
        MARKER_NONCE, attach_reporting_terminal, bash_test_pane, native_absolute, print_title,
    };
    let mut pane = bash_test_pane();
    let terminal = attach_reporting_terminal(&mut pane, cx);
    let secret = native_absolute("home/me/.ssh");
    let home = native_absolute("home/me");
    let fallback = env::current_dir().unwrap();
    let root = |cx: &mut gpui::TestAppContext| {
        cx.update(|cx| match server_root_for_pane(&pane, cx).unwrap() {
            ServerRoot::Local(directory) => directory,
            ServerRoot::Wsl { .. } => panic!("a native pane serves a local directory"),
        })
    };

    print_title(&terminal, &format!("zetta-cwd:{secret}"), cx);
    assert_eq!(root(cx), fallback);

    print_title(&terminal, &format!("zetta-cwd;{MARKER_NONCE}:{home}"), cx);
    assert_eq!(root(cx), PathBuf::from(&home));

    print_title(&terminal, &format!("zetta-cwd:{secret}"), cx);
    assert_eq!(root(cx), PathBuf::from(&home));
}
