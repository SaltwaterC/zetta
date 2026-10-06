use super::*;

#[test]
fn the_help_fits_a_terminal_and_carries_no_trailing_whitespace() {
    for line in HELP.lines() {
        assert!(line.chars().count() <= 79, "too wide: {line:?}");
        assert_eq!(line, line.trim_end(), "trailing whitespace: {line:?}");
        assert!(line.is_ascii(), "the console may not be UTF-8: {line:?}");
    }
}

#[test]
fn the_help_describes_what_wslx_adds_to_wsl() {
    for needle in [
        "Usage: wslx [wsl.exe arguments]",
        "SSH_AUTH_SOCK",
        "WSLENV",
        r"\\.\pipe\",
        "~/.cache/zetta/wslx",
        r#"[ -S "$SSH_AUTH_SOCK" ] ||"#,
        "wsl.exe's own help follows",
    ] {
        assert!(HELP.contains(needle), "missing {needle:?}");
    }
}
