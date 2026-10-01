use super::*;

use crate::project_form::ProjectForm;
use crate::settings_ui::controls::tests::configuration_editor;
use crate::settings_ui::projects::ProjectEditor;
use std::path::{Path, PathBuf};

fn config_with_template() -> Config {
    Config::parse(
        r#"{
            "pane_split_templates": {
                "user-pair": { "layout": { "vertical": [{ "label": "left" }, { "label": "right" }] } }
            }
        }"#,
        None,
        None,
    )
    .unwrap()
}

fn default_config() -> Config {
    Config::parse("{}", None, None).unwrap()
}

fn project_builder(config: &Config) -> SettingsEditor {
    let mut editor = configuration_editor(config);
    editor.page = SettingsPage::Projects;
    editor.project = Some(ProjectEditor {
        root: PathBuf::from("/projects/demo"),
        config_root: PathBuf::from("/projects/demo"),
        index: 0,
        form: ProjectForm::parse(
            r#"{
                "pane_split_templates": {
                    "project-pair": { "layout": { "vertical": [{ "label": "a" }, { "label": "b" }] } }
                }
            }"#,
            Path::new("/projects/demo/.zetta/config.json"),
            config,
        )
        .unwrap(),
        dirty: false,
        save_in_progress: false,
    });
    editor
}

/// The project builder edits its own copy of the pane templates. Typing into
/// one of its fields used to mark the *user configuration* dirty, so the
/// builder's Save saw a clean project, closed without writing, and dropped the
/// edit.
#[test]
fn typing_into_a_project_template_field_marks_the_project_dirty() {
    let config = config_with_template();
    let mut editor = project_builder(&config);

    record_settings_input_edit(
        &mut editor,
        SettingsInput::PaneTemplate(PaneTemplateTextField::Name(0)),
    );

    assert!(editor.project.as_ref().unwrap().dirty);
    assert!(
        !editor.configuration_dirty,
        "the user configuration was not edited"
    );
}

#[test]
fn typing_into_a_user_template_field_marks_the_configuration_dirty() {
    let config = config_with_template();
    let mut editor = configuration_editor(&config);
    editor.page = SettingsPage::PaneTemplates;

    record_settings_input_edit(
        &mut editor,
        SettingsInput::PaneTemplate(PaneTemplateTextField::Name(0)),
    );

    assert!(editor.configuration_dirty);
}

/// The first built-in binding of the bundled keymap, which every default
/// keymap form starts with.
fn first_default_binding(editor: &SettingsEditor) -> (usize, usize) {
    editor
        .keymap
        .sections
        .iter()
        .enumerate()
        .find_map(|(section, form)| {
            (0..form.bindings.len())
                .find(|&binding| editor.is_default_binding(section, binding))
                .map(|binding| (section, binding))
        })
        .expect("the bundled keymap has built-in bindings")
}

/// The keyboard reaches a built-in binding's button through the tab order, and
/// that used to always say `RemoveBinding`: Enter deleted the binding outright,
/// where the button beside it disables it and lists it for restoring.
#[test]
fn the_tab_order_unbinds_a_built_in_binding_rather_than_removing_it() {
    let mut editor = configuration_editor(&default_config());
    editor.page = SettingsPage::Keymap;
    let (section, binding) = first_default_binding(&editor);

    let controls = Zetta::build_settings_controls(&editor);

    assert!(controls.contains(&SettingsControl::UnbindBinding(section, binding)));
    assert!(!controls.contains(&SettingsControl::RemoveBinding(section, binding)));
}

#[test]
fn unbinding_a_built_in_binding_lists_it_for_restoring_and_refreshes_the_rows() {
    let mut editor = configuration_editor(&default_config());
    editor.page = SettingsPage::Keymap;
    keymap::refresh_keymap_cache(&mut editor);
    let (section, binding) = first_default_binding(&editor);
    let keystroke = editor.keymap.sections[section].bindings[binding]
        .keystroke
        .text
        .clone();

    assert!(keymap::unbind_binding(&mut editor, section, binding));

    let form = &editor.keymap.sections[section];
    assert_eq!(form.unbound_defaults.len(), 1);
    assert_eq!(form.unbound_defaults[0].keystroke.text, keystroke);
    assert!(
        form.unbind
            .contains_key(&keymap_keystroke_storage(&keystroke))
    );
    assert!(editor.keymap_dirty);
    // The rows the page renders from are the cached ones, so a stale cache is
    // what the keyboard path used to leave behind.
    assert!(
        keymap::keymap_rows(&editor).contains(&KeymapRow::UnboundDefault(section, 0)),
        "the cached rows list the disabled binding"
    );
    assert!(
        Zetta::build_settings_controls(&editor)
            .contains(&SettingsControl::RestoreBinding(section, 0)),
        "the disabled binding can be restored from the keyboard"
    );
}

#[test]
fn restoring_a_disabled_binding_undoes_the_unbind() {
    let mut editor = configuration_editor(&default_config());
    editor.page = SettingsPage::Keymap;
    let (section, binding) = first_default_binding(&editor);
    let before = editor.keymap.sections[section].bindings.len();
    keymap::unbind_binding(&mut editor, section, binding);

    assert!(keymap::restore_binding(&mut editor, section, 0));

    let form = &editor.keymap.sections[section];
    assert_eq!(form.bindings.len(), before);
    assert!(form.unbound_defaults.is_empty());
    assert!(form.unbind.is_empty());
}

#[test]
fn adding_a_binding_refreshes_the_cached_rows() {
    let mut editor = configuration_editor(&default_config());
    editor.page = SettingsPage::Keymap;
    keymap::refresh_keymap_cache(&mut editor);
    let before = keymap::keymap_rows(&editor).len();

    assert!(keymap::add_binding(&mut editor, 0));

    assert_eq!(keymap::keymap_rows(&editor).len(), before + 1);
    assert!(editor.keymap_dirty);
}

/// Removing a binding leaves focus on the same button of the binding that moved
/// into its place, rather than on nothing.
#[test]
fn removing_a_binding_moves_focus_to_the_next_bindings_button() {
    let mut editor = configuration_editor(&default_config());
    editor.page = SettingsPage::Keymap;
    keymap::add_binding(&mut editor, 0);
    keymap::add_binding(&mut editor, 0);
    let last = editor.keymap.sections[0].bindings.len() - 1;

    assert!(keymap::remove_binding(&mut editor, 0, last - 1));
    assert_eq!(
        editor.focused_control,
        Some(keymap::binding_removal_control(&editor, 0, last - 1))
    );

    let last = editor.keymap.sections[0].bindings.len() - 1;
    assert!(keymap::remove_binding(&mut editor, 0, last));
    assert_eq!(
        editor.focused_control,
        Some(SettingsControl::AddBinding(0)),
        "with nothing below, focus moves to the section's Add button"
    );
}

fn ring_bytes_input() -> SettingsInput {
    SettingsInput::Configuration(ConfigTextField::Setting(ConfigSetting::SessionRingBytes))
}

/// A value Save would refuse is reported as the keyboard leaves its field, not
/// only once Save is pressed, and it stays reported until the field changes.
#[test]
fn leaving_a_field_that_cannot_be_saved_reports_it_until_it_is_edited() {
    let mut editor = configuration_editor(&default_config());
    editor.configuration.session_ring_bytes.text = "12".to_owned();
    editor.focused_input = Some(ring_bytes_input());

    // Clicking back into the field, or onto its steppers, is not leaving it.
    check_setting_being_left(&mut editor, &SettingsControl::Input(ring_bytes_input()));
    check_setting_being_left(
        &mut editor,
        &SettingsControl::Numeric(ConfigSetting::SessionRingBytes),
    );
    assert_eq!(editor.invalid_setting, None);

    check_setting_being_left(&mut editor, &SettingsControl::AddProfile);
    let (setting, message) = editor.invalid_setting.clone().unwrap();
    assert_eq!(setting, ConfigSetting::SessionRingBytes);
    assert!(message.starts_with("Retained screen size"), "{message}");

    // Editing another field leaves it reported; editing this one clears it.
    record_settings_input_edit(
        &mut editor,
        SettingsInput::Configuration(ConfigTextField::Setting(ConfigSetting::WorkingDirectory)),
    );
    assert!(editor.invalid_setting.is_some());
    record_settings_input_edit(&mut editor, ring_bytes_input());
    assert_eq!(editor.invalid_setting, None);
}

/// Leaving a valid field must not clear what another field reported.
#[test]
fn leaving_a_valid_field_keeps_another_fields_report() {
    let mut editor = configuration_editor(&default_config());
    editor.invalid_setting = Some((ConfigSetting::FontSize, "Font size must be".to_owned()));
    editor.focused_input = Some(ring_bytes_input());
    check_setting_being_left(&mut editor, &SettingsControl::AddProfile);
    assert_eq!(
        editor.invalid_setting.map(|(setting, _)| setting),
        Some(ConfigSetting::FontSize)
    );
}
