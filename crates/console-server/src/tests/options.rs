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
fn pipeline_accepts_both_flag_forms() {
    for flag in ["--pipeline", "-p"] {
        assert!(
            parse(&["--command", "cmd /c echo ok", flag])
                .unwrap()
                .pipeline
        );
    }
}
