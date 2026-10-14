use super::*;
fn parse(args: &[&str]) -> io::Result<Options> {
    Options::parse(args.iter().map(OsString::from))
}
#[test]
fn flags_accept_long_and_short_forms() {
    for args in [
        vec![
            "--command",
            "cmd /c echo ok",
            "--drain",
            "--timeout",
            "2",
            "--history",
            "0",
        ],
        vec!["-c", "cmd /c echo ok", "-d", "-t", "2", "-s", "0"],
    ] {
        let result = parse(&args).unwrap();
        assert!(result.drain);
        assert_eq!(result.timeout, 2);
        assert_eq!(result.history, 0);
        assert_eq!(result.command.as_deref(), Some("cmd /c echo ok"));
    }
}
#[test]
fn rejects_missing_values_unknown_options_and_unbounded_timeout() {
    for args in [
        vec!["--command"],
        vec!["--ctrl-c", "--timeout", "0"],
        vec!["--ctrl-c", "--timeout", "bad"],
        vec!["--typo"],
        vec![],
    ] {
        assert!(parse(&args).is_err());
    }
}
#[test]
fn help_table_aligns_multiline_descriptions_without_trailing_whitespace() {
    let help = format_help_table([("a", "first\nnext"), ("long", "second")]);
    let lines: Vec<_> = help.lines().collect();
    assert_eq!(lines[0].find("first"), lines[1].find("next"));
    assert_eq!(lines[0].find("first"), lines[2].find("second"));
    assert!(lines.iter().all(|line| !line.ends_with(' ')));
}

#[test]
fn pipeline_accepts_both_flag_forms() {
    for flag in ["--pipeline", "-p"] {
        assert!(
            parse(&["--command", "cmd /c echo ok", flag])
                .unwrap()
                .pipeline
        );
    }
}
