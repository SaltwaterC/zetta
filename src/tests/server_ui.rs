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
