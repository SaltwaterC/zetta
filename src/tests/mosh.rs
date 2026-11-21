use super::*;

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn the_original_arguments_are_forwarded_without_rewriting_them() {
    let raw = arguments(&["--family=prefer-inet6", "pi@adsb", "--", "zsh", "-l"]);
    assert_eq!(
        parse_mosh_args(&raw).unwrap().raw_arguments,
        raw,
        "zetta mosh must not reinterpret arguments before zosh sees them"
    );
}

#[cfg(feature = "zosh-client")]
#[test]
fn every_option_zosh_accepts_is_accepted() {
    // The copy of the parser this replaced refused `--scrollback`.
    for raw in [
        &["--scrollback", "host"][..],
        &["--no-scrollback", "host"],
        &["-k", "host"],
        &["--help"],
        &["--version"],
    ] {
        assert!(parse_mosh_args(&arguments(raw)).is_ok(), "{raw:?}");
    }
}

#[cfg(feature = "zosh-client")]
#[test]
fn a_command_line_zosh_would_refuse_is_reported_before_the_handoff() {
    for raw in [&["--no-such-option", "host"][..], &[], &["--"]] {
        assert!(parse_mosh_args(&arguments(raw)).is_err(), "{raw:?}");
    }
}
