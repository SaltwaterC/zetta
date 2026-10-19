use super::*;

#[test]
fn format_help_table_aligns_multiline_rows_without_trailing_whitespace() {
    let help = format_help_table([
        ("short", "first description\ncontinued description"),
        ("long label", "second description"),
    ]);
    let lines = help.lines().collect::<Vec<_>>();
    let description_column = 2 + "long label".chars().count() + 2;

    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].find("first description"), Some(description_column));
    assert_eq!(
        lines[1].find("continued description"),
        Some(description_column)
    );
    assert_eq!(
        lines[2].find("second description"),
        Some(description_column)
    );
    assert!(lines.iter().all(|line| *line == line.trim_end()));
    assert!(lines[0].starts_with("  short"));
    assert!(lines[2].starts_with("  long label"));
}

#[test]
fn server_options_preserve_restrictive_defaults_and_option_values() {
    let defaults = parse_http_args([OsString::from("server")]).unwrap();
    assert_eq!(
        defaults,
        ServerCommand {
            root: PathBuf::from("."),
            port: 8000,
            options: crate::HttpServerOptions::default()
        }
    );
    let command = parse_http_args(
        [
            "server",
            "-r",
            "-d",
            "-p",
            "8080",
            "-d",
            "-a",
            "Builds.LAN.",
            "-A",
        ]
        .map(OsString::from),
    )
    .unwrap();
    assert_eq!(command.root, PathBuf::from("-d"));
    assert_eq!(command.port, 8080);
    assert_eq!(
        command.options,
        crate::HttpServerOptions {
            serve_dotfiles: true,
            allowed_hosts: vec!["builds.lan".to_owned()],
            allow_any_host: true
        }
    );
    for args in [
        vec!["server", "-p", "0"],
        vec!["server", "--root"],
        vec!["server", "-d", "-d"],
        vec!["server", "-a", "host:8000"],
        vec!["server", "-A", "-A"],
        vec!["unknown"],
    ] {
        assert!(parse_http_args(args.into_iter().map(OsString::from)).is_err());
    }
}
