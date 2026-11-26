use super::*;

/// The axis became an enum; the bytes a split travels as did not, so a daemon
/// and a client either side of the change still read each other's layouts.
#[test]
fn a_split_layout_keeps_its_wire_form() {
    let layout = BackgroundPaneLayout::Split {
        axis: SplitAxis::Vertical,
        first_ratio: 400,
        first: Box::new(BackgroundPaneLayout::Pane { pane_id: 1 }),
        second: Box::new(BackgroundPaneLayout::Pane { pane_id: 2 }),
    };
    let wire = r#"{"type":"split","axis":"vertical","first_ratio":400,"first":{"type":"pane","pane_id":1},"second":{"type":"pane","pane_id":2}}"#;
    assert_eq!(serde_json::to_string(&layout).unwrap(), wire);
    assert_eq!(
        serde_json::from_str::<BackgroundPaneLayout>(wire).unwrap(),
        layout
    );
}

/// What the daemon used to refuse after decoding is now refused by decoding.
#[test]
fn a_split_on_an_unknown_axis_does_not_decode() {
    let wire = r#"{"type":"split","axis":"diagonal","first":{"type":"pane","pane_id":1},"second":{"type":"pane","pane_id":2}}"#;
    assert!(serde_json::from_str::<BackgroundPaneLayout>(wire).is_err());
}
