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
fn a_repeated_option_is_refused_before_its_value_is_read() {
    let mut size = Some(1);
    let error = set_once(&mut size, "--size", || anyhow::bail!("the value was read")).unwrap_err();
    assert_eq!(error.to_string(), "--size may only be specified once");
    assert_eq!(size, Some(1));

    let mut list = false;
    set_flag(&mut list, "--list").unwrap();
    assert!(list);
    let error = set_flag(&mut list, "--list").unwrap_err();
    assert_eq!(error.to_string(), "--list may only be specified once");
}
