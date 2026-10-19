use super::*;

#[test]
fn server_options_require_explicit_upload_permission() {
    let defaults = parse_server_args([]).unwrap();
    assert_eq!(
        defaults,
        ServerCommand {
            root: PathBuf::from("."),
            port: 69,
            writable: false
        }
    );
    for flag in ["--writable", "-w"] {
        let command =
            parse_server_args(["--root", "images", "--port", "1069", flag].map(OsString::from))
                .unwrap();
        assert_eq!(
            command,
            ServerCommand {
                root: PathBuf::from("images"),
                port: 1069,
                writable: true
            }
        );
    }
    for args in [
        vec!["--port", "0"],
        vec!["--port", "65536"],
        vec!["--root"],
        vec!["--writable", "-w"],
        vec!["--port", "69", "-p", "70"],
    ] {
        assert!(parse_server_args(args.into_iter().map(OsString::from)).is_err());
    }
}

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
