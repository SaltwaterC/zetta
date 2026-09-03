use super::*;

pub(super) fn settings_test_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

pub(super) fn configuration_form_with_empty_custom_template() -> (ConfigurationForm, usize) {
    let path = settings_test_path("zetta-empty-pane-template");
    let config = Config::defaults(None, None);
    let mut form = ConfigurationForm::load(&path, &config).unwrap();
    let index = form.pane_templates.create_empty();
    (form, index)
}

#[test]
fn text_field_edits_unicode_and_replaces_selection() {
    let mut field = TextField::new("héllo");
    field.move_left();
    field.backspace();
    assert_eq!(field.text, "hélo");
    field.select_all();
    field.insert("Zetta");
    assert_eq!(field.text, "Zetta");
}

#[test]
fn default_binding_lookup_keeps_plus_and_minus_shortcuts_distinct() {
    let defaults = default_bindings_by_context().unwrap();
    let terminal = defaults.get("Zetta > Terminal").unwrap();

    assert_eq!(
        terminal.get("ctrl-+"),
        Some(&json!("zetta::IncreaseTerminalFontSize"))
    );
    assert_eq!(
        terminal.get("ctrl--"),
        Some(&json!("zetta::DecreaseTerminalFontSize"))
    );
    assert_eq!(
        terminal.get("alt-shift-+"),
        Some(&json!("zetta::IncreasePaneFontSize"))
    );
    assert_eq!(
        terminal.get("alt-shift--"),
        Some(&json!("zetta::DecreasePaneFontSize"))
    );
}

#[test]
fn save_creates_parent_directories() {
    let root = std::env::temp_dir().join(format!("zetta-settings-save-{}", std::process::id()));
    let path = root.join("nested/config.json");
    save(&path, "{}").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "{}\n");
    fs::remove_dir_all(root).unwrap();
}
