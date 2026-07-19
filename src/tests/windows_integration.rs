use super::*;

#[test]
fn quotes_profile_names_for_windows_command_lines() {
    assert_eq!(quote_windows_argument("PowerShell"), "PowerShell");
    assert_eq!(quote_windows_argument("WSL: Ubuntu"), r#""WSL: Ubuntu""#);
    assert_eq!(
        quote_windows_argument(r#"A "quoted" profile"#),
        r#""A \"quoted\" profile""#
    );
    assert_eq!(quote_windows_argument(r#"A\"B"#), r#""A\\\"B""#);
    assert_eq!(
        quote_windows_argument(r"Trailing slash\"),
        r#""Trailing slash\\""#
    );
}
