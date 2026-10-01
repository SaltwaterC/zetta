use super::*;
use crate::settings_ui::controls::tests::configuration_editor;

fn editor() -> SettingsEditor {
    configuration_editor(&Config::parse("{}", None, None).unwrap())
}

#[test]
fn a_clean_dialog_closes_without_asking() {
    let editor = editor();
    for request in [
        CloseRequest::Dialog,
        CloseRequest::ProjectBuilder,
        CloseRequest::OpenProject(0),
        CloseRequest::OpenProjectFile,
    ] {
        assert!(!request.loses_changes(&editor), "{request:?}");
    }
}

/// Staged edits used to be thrown away by Close, Esc and the settings
/// shortcut without a word.
#[test]
fn closing_with_staged_edits_asks_first_and_can_save_them() {
    let mut editor = editor();
    editor.keymap_dirty = true;

    assert!(CloseRequest::Dialog.loses_changes(&editor));
    assert!(CloseRequest::OpenProject(0).loses_changes(&editor));
    // Leaving the builder does not lose the user configuration's edits: they
    // stay staged in the dialog.
    assert!(!CloseRequest::ProjectBuilder.loses_changes(&editor));

    editor.close_request = Some(CloseRequest::Dialog);
    assert_eq!(
        close_request_controls(&editor),
        vec![
            SettingsControl::KeepEditing,
            SettingsControl::DiscardChanges,
            SettingsControl::SaveBeforeClosing,
        ]
    );
    assert_eq!(
        Zetta::build_settings_controls(&editor),
        close_request_controls(&editor),
        "the confirmation holds the keyboard while it is up"
    );
}

/// Save before closing is offered only where one press of Save writes
/// everything the request would lose.
#[test]
fn saving_first_is_not_offered_where_save_would_not_close() {
    let mut editor = editor();
    editor.configuration_dirty = true;

    assert!(CloseRequest::Dialog.can_save_first(&editor));
    assert!(!CloseRequest::OpenProjectFile.can_save_first(&editor));
    assert!(!CloseRequest::ProjectBuilder.can_save_first(&editor));
}
