use super::*;
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
#[test]
fn copy_options_and_help() {
    assert_eq!(
        parse_copy_args(args(&["-pboard", "General"])).unwrap(),
        CopyMode::Copy
    );
    assert_eq!(
        parse_copy_args(args(&[CLIPBOARD_DAEMON_FLAG])).unwrap(),
        CopyMode::Daemon
    );
    for values in [
        &["-pboard"][..],
        &["-pboard", "other"][..],
        &["-pboard", "general", "-pboard", "font"][..],
        &["--bad"][..],
    ] {
        assert!(parse_copy_args(args(values)).is_err());
    }
    assert!(copy_help().contains("Usage: zcopy"));
    assert!(copy_help().contains("-pboard"));
}
#[test]
fn paste_options_and_help() {
    parse_paste_args(args(&["-pboard", "ruler", "-Prefer", "rtf"])).unwrap();
    for values in [
        &["-Prefer"][..],
        &["-Prefer", "other"][..],
        &["-Prefer", "txt", "-Prefer", "ps"][..],
        &["--bad"][..],
    ] {
        assert!(parse_paste_args(args(values)).is_err());
    }
    assert!(paste_help().contains("Usage: zpaste"));
    assert!(paste_help().contains("-Prefer"));
}
