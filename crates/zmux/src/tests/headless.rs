use super::*;

#[test]
fn parses_recursive_layout_and_merges_environment() {
    let spec = parse_reader(
        &br#"{
            "title": "server",
            "active_pane": 1,
            "env": {"ROOT": "yes"},
            "layout": {
                "horizontal": [
                    {"label": "api", "env": {"ROLE": "api"}},
                    {"profile": "Bash", "command": {"program": "/bin/sh", "args": ["-l"]}}
                ]
            }
        }"#[..],
    )
    .expect("layout should parse");

    assert_eq!(spec.title, "server");
    assert_eq!(spec.panes.len(), 2);
    assert_eq!(spec.panes[0].env.get("ROOT"), Some(&"yes".to_owned()));
    assert_eq!(spec.panes[0].env.get("ROLE"), Some(&"api".to_owned()));
    assert_eq!(spec.panes[1].profile, "Bash");
    assert_eq!(
        spec.panes[1]
            .command
            .as_ref()
            .and_then(|command| command.program.as_deref()),
        Some("/bin/sh")
    );
    assert!(matches!(
        spec.active_pane,
        SharedPaneRef::Draft { draft_id: 2 }
    ));
}

#[test]
fn rejects_unknown_fields_and_invalid_active_pane() {
    let error = parse_reader(&br#"{"layout": {}, "extra": true}"#[..])
        .expect_err("unknown fields should be rejected");
    assert!(error.to_string().contains("headless layout JSON"));

    let error = parse_reader(&br#"{"active_pane": 1, "layout": {}}"#[..])
        .expect_err("active pane outside the tree should be rejected");
    assert!(error.to_string().contains("active_pane"));
}

#[test]
fn single_pane_defaults_to_system_and_eighty_by_twenty_four() {
    let spec = single_pane("".to_owned(), None, None, HashMap::new());
    assert_eq!(spec.panes[0].profile, "System");
    assert_eq!(spec.panes[0].size.columns, DEFAULT_COLUMNS);
    assert_eq!(spec.panes[0].size.lines, DEFAULT_LINES);
}
