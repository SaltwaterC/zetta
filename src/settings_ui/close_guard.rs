//! Leaving the settings dialog, or the project builder inside it, without
//! losing unsaved changes by accident.
//!
//! Every way out goes through [`Zetta::request_settings_close`]: the header's
//! Close, Esc, the settings shortcut, the builder's Back button, and the two
//! buttons that close the dialog to do something else (opening a project,
//! opening its file in an editor). With nothing unsaved the request is carried
//! out at once. Otherwise it is held in `SettingsEditor::close_request` and the
//! dialog asks whether to keep editing, discard, or — where the header's Save
//! can do it — save first. These used to discard staged edits silently, and
//! the builder's Back said so only once it had already happened.

use super::*;

/// A way out of the dialog, or out of the project builder, held while the
/// dialog asks what to do about unsaved changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseRequest {
    /// Close the dialog.
    Dialog,
    /// Leave the project builder for the project list.
    ProjectBuilder,
    /// Close the dialog and open the registered project at this index.
    OpenProject(usize),
    /// Close the dialog and open the builder's project file in an editor.
    OpenProjectFile,
}

impl CloseRequest {
    /// Whether carrying out the request would throw away anything unsaved.
    pub(crate) fn loses_changes(self, editor: &SettingsEditor) -> bool {
        let project_dirty = project_editor(editor).is_some_and(|project| project.dirty);
        match self {
            Self::ProjectBuilder => project_dirty,
            Self::Dialog | Self::OpenProject(_) | Self::OpenProjectFile => {
                project_dirty || editor.configuration_dirty || editor.keymap_dirty
            }
        }
    }

    /// Whether the confirmation can offer to save first. The header's Save
    /// writes the open project, or the configuration and keymap, but not both
    /// in one press, so saving first is offered only when it would save
    /// everything this request would otherwise lose — and only when saving
    /// ends by closing the dialog, which is the configuration's Save.
    pub(crate) fn can_save_first(self, editor: &SettingsEditor) -> bool {
        let project_dirty = project_editor(editor).is_some_and(|project| project.dirty);
        self == Self::Dialog && !project_dirty
    }
}

/// The confirmation's controls, in the order it draws them.
pub(crate) fn close_request_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
    let mut controls = vec![
        SettingsControl::KeepEditing,
        SettingsControl::DiscardChanges,
    ];
    if editor
        .close_request
        .is_some_and(|request| request.can_save_first(editor))
    {
        controls.push(SettingsControl::SaveBeforeClosing);
    }
    controls
}

impl Zetta {
    /// Carries out `request`, or holds it and asks first when it would lose
    /// unsaved changes.
    pub(crate) fn request_settings_close(
        &mut self,
        request: CloseRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if settings_save_in_flight(editor) {
            return;
        }
        if !request.loses_changes(editor) {
            self.carry_out_settings_close(request, window, cx);
            return;
        }
        editor.clear_dropdown();
        editor.close_request = Some(request);
        editor.focused_input = None;
        editor.focused_control = Some(SettingsControl::KeepEditing);
        invalidate_controls_cache(editor);
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    /// The confirmation's three answers.
    pub(crate) fn answer_settings_close(
        &mut self,
        answer: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(request) = editor.close_request.take() else {
            return;
        };
        invalidate_controls_cache(editor);
        match answer {
            SettingsControl::DiscardChanges => self.carry_out_settings_close(request, window, cx),
            // Save closes the dialog once it has written; a failure leaves it
            // open with the error, which is the right place to be.
            SettingsControl::SaveBeforeClosing => self.save_settings(window, cx),
            _ => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.focused_control = Some(match request {
                        CloseRequest::ProjectBuilder => SettingsControl::CloseProjectConfig,
                        _ => SettingsControl::Close,
                    });
                }
                cx.notify();
            }
        }
    }

    fn carry_out_settings_close(
        &mut self,
        request: CloseRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request {
            CloseRequest::Dialog => self.dismiss_settings(window, cx),
            CloseRequest::ProjectBuilder => self.close_project_config(window, cx),
            CloseRequest::OpenProject(index) => self.open_project_from_settings(index, window, cx),
            CloseRequest::OpenProjectFile => self.open_project_config_file(window, cx),
        }
    }
}

#[cfg(test)]
#[path = "../tests/settings_ui/close_guard.rs"]
mod tests;
