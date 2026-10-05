use super::*;

fn arguments(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn target_of(args: &[&str]) -> Option<Vec<String>> {
    let args = arguments(args);
    match classify(&args) {
        Invocation::Session { target } => Some(
            target
                .iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect(),
        ),
        Invocation::Management => None,
    }
}

#[test]
fn no_arguments_is_a_session_in_the_default_distribution() {
    assert_eq!(target_of(&[]), Some(vec![]));
}

#[test]
fn distribution_and_user_options_are_kept_for_the_relay() {
    assert_eq!(
        target_of(&["-d", "Ubuntu", "-u", "root"]),
        Some(vec![
            "-d".into(),
            "Ubuntu".into(),
            "-u".into(),
            "root".into()
        ])
    );
    assert_eq!(
        target_of(&["--distribution-id", "{0b5e}", "--user", "me", "--system"]),
        Some(vec![
            "--distribution-id".into(),
            "{0b5e}".into(),
            "--user".into(),
            "me".into(),
            "--system".into()
        ])
    );
}

#[test]
fn session_options_that_do_not_select_a_target_are_skipped() {
    assert_eq!(
        target_of(&[
            "--cd",
            "/tmp",
            "--distribution",
            "Debian",
            "--shell-type",
            "login"
        ]),
        Some(vec!["--distribution".into(), "Debian".into()])
    );
    assert_eq!(
        target_of(&["~", "-d", "Arch"]),
        Some(vec!["-d".into(), "Arch".into()])
    );
}

#[test]
fn nothing_after_the_command_line_starts_is_read_as_an_option() {
    assert_eq!(target_of(&["-e", "ssh", "-d", "x"]), Some(vec![]));
    assert_eq!(target_of(&["--exec", "--list"]), Some(vec![]));
    assert_eq!(
        target_of(&["-d", "Ubuntu", "--", "-u", "root"]),
        Some(vec!["-d".into(), "Ubuntu".into()])
    );
    assert_eq!(target_of(&["ls", "-la", "--user", "root"]), Some(vec![]));
}

#[test]
fn management_commands_get_no_relay() {
    for args in [
        &["--list", "--verbose"][..],
        &["-l"],
        &["--shutdown"],
        &["--install"],
        &["--help"],
        &["--version"],
        &["-d", "Ubuntu", "--terminate"],
        &["--set-default", "Ubuntu"],
        &["--unknown-option"],
    ] {
        assert_eq!(target_of(args), None, "{args:?}");
    }
}

#[test]
fn a_tilde_is_only_the_home_shortcut_in_first_place() {
    assert_eq!(
        target_of(&["-d", "Ubuntu", "~"]),
        Some(vec!["-d".into(), "Ubuntu".into()])
    );
}

#[test]
fn an_option_missing_its_value_is_left_to_wsl_to_report() {
    assert_eq!(target_of(&["-d"]), None);
    assert_eq!(target_of(&["--cd"]), None);
}
