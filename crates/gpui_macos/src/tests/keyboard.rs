use super::*;

#[test]
fn input_source_without_unicode_layout_data_is_unavailable() {
    let layout = MacKeyboardLayout::from_input_source_properties(
        false,
        Some("com.example.removed-layout".into()),
        Some("Removed Layout".into()),
    );

    assert!(layout.is_none());
}

#[test]
fn input_source_with_layout_data_keeps_its_identity() {
    let layout = MacKeyboardLayout::from_input_source_properties(
        true,
        Some("com.example.layout".into()),
        Some("Example Layout".into()),
    )
    .unwrap();

    assert_eq!(layout.id(), "com.example.layout");
    assert_eq!(layout.name(), "Example Layout");
}
