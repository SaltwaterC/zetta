use super::*;

#[test]
fn inherited_windows_internal_environment_names_are_valid() {
    for name in ["PATH", "ZETTA_PANE_ID", "=C:", "=z:", "=ExitCode"] {
        assert!(valid_environment_name(name), "{name:?}");
    }
}

#[test]
fn empty_or_malformed_environment_names_are_rejected() {
    for name in ["", "=", "==C:", "PATH=extra", "=C:=extra", "PATH\0extra"] {
        assert!(!valid_environment_name(name), "{name:?}");
    }
}
