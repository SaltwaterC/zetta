//! Applying an edit a settings control asks for: activating a control, typing
//! into a text field, toggling a switch, and the sliders and numeric fields
//! that repeat while held.
//!
//! Every path here writes the typed form and then persists it, so the file on
//! disk and the form the page renders from cannot drift apart.

use super::*;

/// Where the Themes page sends a user looking for a theme to install.
const THEME_STORE_URL: &str = "https://zed.dev/extensions?filter=themes";

impl Zetta {
    pub(crate) fn activate_settings_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .settings_editor
            .as_ref()
            .is_some_and(settings_save_in_flight)
        {
            return;
        }
        match control {
            SettingsControl::Tab(page) => self.select_settings_page(page, window, cx),
            SettingsControl::Close => self.activate_settings_close(window, cx),
            SettingsControl::Save => self.save_settings(window, cx),
            SettingsControl::Input(input) => self.focus_settings_input(input, window, cx),
            SettingsControl::CaptureKeymap(target) => self.start_keymap_capture(target, window, cx),
            SettingsControl::Dropdown(dropdown) => {
                self.open_settings_dropdown(dropdown, window.mouse_position(), cx);
            }
            SettingsControl::Toggle(toggle) => self.activate_settings_toggle(toggle, window, cx),
            #[cfg(target_os = "macos")]
            SettingsControl::RequestFocusStatusAccess => {
                window.dispatch_action(Box::new(RequestFocusStatusAccess), cx);
            }
            SettingsControl::FontPicker => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.font_query = Some(TextField::default());
                    editor.scroll_geometry_initialized = false;
                    Self::rebuild_font_search_cache(editor);
                    // The picker's tab order replaces the page's.
                    invalidate_controls_cache(editor);
                }
                self.focus_settings_input(SettingsInput::FontSearch, window, cx);
            }
            SettingsControl::CloseModal => self.close_settings_modal(cx),
            SettingsControl::KeepEditing
            | SettingsControl::DiscardChanges
            | SettingsControl::SaveBeforeClosing => {
                self.answer_settings_close(control, window, cx);
            }
            SettingsControl::DefaultTabIconPicker => {
                self.open_default_tab_icon_picker(window, cx);
            }
            // Sliders and steppers act on their arrow keys, not on activation.
            SettingsControl::Numeric(_) | SettingsControl::Opacity(_) => {}
            SettingsControl::AddProfile => self.begin_profile_draft(window, cx),
            SettingsControl::RemoveProfile(index) => {
                self.remove_settings_profile(index, window, cx);
            }
            SettingsControl::AddProfileArgument(target) => {
                self.add_profile_argument(target, window, cx);
            }
            SettingsControl::RemoveProfileArgument(target, argument) => {
                self.remove_profile_argument(target, argument, cx);
            }
            SettingsControl::OpenThemeStore => cx.open_url(THEME_STORE_URL),
            SettingsControl::SearchThemes => self.fetch_theme_extensions(window, cx),
            SettingsControl::InstallTheme(id) => self.download_theme_extension(id, window, cx),
            SettingsControl::RemoveTheme(ref id) => {
                if self.confirm_settings_control(&control, cx) {
                    self.remove_theme_extension(id.clone(), window, cx);
                }
            }
            SettingsControl::RemoveBinding(section, binding) => {
                self.edit_settings_keymap(
                    |editor| keymap::remove_binding(editor, section, binding),
                    cx,
                );
            }
            SettingsControl::UnbindBinding(section, binding) => {
                self.edit_settings_keymap(
                    |editor| keymap::unbind_binding(editor, section, binding),
                    cx,
                );
            }
            SettingsControl::RestoreBinding(section, unbound) => {
                self.edit_settings_keymap(
                    |editor| keymap::restore_binding(editor, section, unbound),
                    cx,
                );
            }
            SettingsControl::AddBinding(section) => {
                self.edit_settings_keymap(|editor| keymap::add_binding(editor, section), cx);
            }
            SettingsControl::AddKeymapSection => {
                self.edit_settings_keymap(keymap::add_keymap_section, cx);
            }
            SettingsControl::Font(index) => self.select_settings_font(index, window, cx),
            SettingsControl::CreateProfile => self.create_settings_profile(window, cx),
            SettingsControl::SelectPaneTemplate(_)
            | SettingsControl::SelectPaneTemplateNode(_)
            | SettingsControl::NewPaneTemplate
            | SettingsControl::DuplicatePaneTemplate
            | SettingsControl::DeletePaneTemplate
            | SettingsControl::SplitPaneTemplate(_, _)
            | SettingsControl::RemovePaneTemplateNode(_)
            | SettingsControl::SwapPaneTemplateChildren(_)
            | SettingsControl::AddPaneTemplateArgument(_)
            | SettingsControl::RemovePaneTemplateArgument(_, _)
            | SettingsControl::AddPaneTemplateStackEntry(_)
            | SettingsControl::RemovePaneTemplateStackEntry(_, _)
            | SettingsControl::AddPaneTemplateStackArgument(_, _)
            | SettingsControl::RemovePaneTemplateStackArgument(_, _, _)
            | SettingsControl::AddPaneTemplateGlobalEnvironment
            | SettingsControl::RemovePaneTemplateGlobalEnvironment(_)
            | SettingsControl::AddPaneTemplateEnvironment(_)
            | SettingsControl::RemovePaneTemplateEnvironment(_, _)
            | SettingsControl::TogglePaneTemplateOverlay(_) => {
                self.activate_settings_pane_template_control(control, window, cx);
            }
            SettingsControl::CloseProjectConfig
            | SettingsControl::OpenProjectConfigFile
            | SettingsControl::ProjectTabIconPicker
            | SettingsControl::ClearProjectTabIcon
            | SettingsControl::AddProjectEnvironment
            | SettingsControl::RemoveProjectEnvironment(_)
            | SettingsControl::AddProjectCommand
            | SettingsControl::RemoveProjectCommand(_)
            | SettingsControl::AddProjectCommandEnvironment(_)
            | SettingsControl::RemoveProjectCommandEnvironment(_, _)
            | SettingsControl::AddProjectProfile
            | SettingsControl::RemoveProjectProfile(_) => {
                self.activate_settings_project_control(control, window, cx);
            }
            SettingsControl::AddProject => self.add_project_from_settings(window, cx),
            SettingsControl::OpenProject(index) => {
                self.request_settings_close(
                    close_guard::CloseRequest::OpenProject(index),
                    window,
                    cx,
                );
            }
            SettingsControl::EditProject(index) => {
                self.edit_project_from_settings(index, window, cx);
            }
            SettingsControl::RemoveProject(index) => {
                if self.confirm_settings_control(&control, cx) {
                    self.remove_project_from_settings(index, window, cx);
                }
            }
        }
    }

    /// Whether a destructive control acts on this press; see
    /// [`controls::confirm_destructive`].
    fn confirm_settings_control(
        &mut self,
        control: &SettingsControl,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.settings_editor.as_mut() else {
            return false;
        };
        let confirmed = controls::confirm_destructive(editor, control);
        if !confirmed {
            cx.notify();
        }
        confirmed
    }

    /// The header's Close.
    fn activate_settings_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_settings_close(close_guard::CloseRequest::Dialog, window, cx);
    }

    /// Closes the font picker or the Add profile modal, whichever is open,
    /// discarding its draft: the modal's Cancel, its Close, and Esc.
    pub(crate) fn close_settings_modal(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.clear_dropdown();
            editor.font_query = None;
            editor.dismiss_profile_draft();
            editor.focused_input = None;
            editor.focused_control = None;
            editor.focus_scroll_request = None;
            editor.message = None;
            invalidate_controls_cache(editor);
            cx.notify();
        }
    }

    /// Flipping one of the Configuration page's switches.
    fn activate_settings_toggle(
        &mut self,
        toggle: SettingsToggle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = self.settings_editor.as_ref().map(|editor| match toggle {
            SettingsToggle::Setting(setting) => {
                setting.switch_shown(&editor.configuration).unwrap_or(false)
            }
            SettingsToggle::ProfileVisibility(index) => editor
                .configuration
                .profiles
                .get(index)
                .is_some_and(|profile| !profile.hidden),
            SettingsToggle::ProfileDraftVisibility => editor
                .profile_draft
                .as_ref()
                .is_some_and(|profile| !profile.hidden),
            SettingsToggle::ProjectOpacityOverride => editor
                .project
                .as_ref()
                .is_some_and(|project| project.form.inactive_pane_opacity.is_some()),
            SettingsToggle::ProjectProfileVisibility(index) => editor
                .project
                .as_ref()
                .and_then(|project| project.form.profiles.get(index))
                .is_some_and(|profile| !profile.hidden),
        });
        if let Some(value) = value {
            self.set_settings_toggle(toggle, !value, window, cx);
        }
    }

    /// Opening the Add profile modal on a blank draft.
    fn begin_profile_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.profile_draft_scroll = ScrollHandle::new();
            editor.profile_draft = Some(settings_editor::ProfileForm::blank());
            editor.message = None;
            invalidate_controls_cache(editor);
        }
        self.focus_settings_input(
            SettingsInput::ProfileDraft(ProfileDraftField::Name),
            window,
            cx,
        );
    }

    /// Appending an empty argument to a profile, and putting the cursor in it.
    fn add_profile_argument(
        &mut self,
        target: ProfileTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(arguments) = profile_arguments_mut(editor, target) else {
            return;
        };
        arguments.push(TextField::default());
        let added = arguments.len() - 1;
        profile_arguments_edited(editor, target);
        self.focus_settings_input(target.argument_input(added), window, cx);
    }

    /// Removing one argument from a profile. Focus stays on the same button of
    /// the argument that moved up into its place, or moves to Add argument when
    /// it was the last.
    fn remove_profile_argument(
        &mut self,
        target: ProfileTarget,
        argument: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(arguments) = profile_arguments_mut(editor, target) else {
            return;
        };
        if argument >= arguments.len() {
            return;
        }
        arguments.remove(argument);
        let remaining = arguments.len();
        profile_arguments_edited(editor, target);
        editor.focused_control = Some(if argument < remaining {
            SettingsControl::RemoveProfileArgument(target, argument)
        } else {
            SettingsControl::AddProfileArgument(target)
        });
        cx.notify();
    }

    /// Removing a user-defined profile.
    fn remove_settings_profile(
        &mut self,
        index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut()
            && index < editor.configuration.profiles.len()
        {
            editor.configuration.profiles.remove(index);
            editor.configuration_dirty = true;
            editor.focused_control = None;
            invalidate_controls_cache(editor);
            cx.notify();
        }
    }

    /// Removing, unbinding, restoring and adding keymap bindings. The edits
    /// themselves live in `keymap`, shared with the row buttons.
    fn edit_settings_keymap(
        &mut self,
        edit: impl FnOnce(&mut SettingsEditor) -> bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut()
            && edit(editor)
        {
            cx.notify();
        }
    }

    /// Choosing a font family from the picker.
    fn select_settings_font(&mut self, index: usize, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut()
            && let Some(font) = editor.fonts.get(index)
        {
            editor.configuration.terminal_font_family = font.clone();
            editor.configuration_dirty = true;
            editor.clear_dropdown();
            editor.font_query = None;
            editor.focused_input = None;
            editor.focused_control = None;
            editor.message = None;
            invalidate_controls_cache(editor);
            cx.notify();
        }
    }

    /// Committing the Add profile modal's draft.
    fn create_settings_profile(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let valid = self.settings_editor.as_ref().is_some_and(|editor| {
            editor.profile_draft.as_ref().is_some_and(|draft| {
                Self::profile_draft_has_required_fields(&draft.name.text, &draft.program.text)
            })
        });
        if !valid {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.message = Some((
                    Tone::Error,
                    "Profile name and program are required.".to_owned(),
                ));
            }
            cx.notify();
            return;
        }
        let duplicate = self.settings_editor.as_ref().and_then(|editor| {
            let name = editor.profile_draft.as_ref()?.name.text.trim();
            editor
                .configuration
                .profiles
                .iter()
                .any(|profile| profile.name.text.trim().eq_ignore_ascii_case(name))
                .then(|| name.to_owned())
        });
        if let Some(name) = duplicate {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.message = Some((
                    Tone::Error,
                    format!("A profile named {name:?} already exists; choose another name."),
                ));
            }
            cx.notify();
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            let mut draft = editor.profile_draft.take().unwrap();
            draft.automatic_icon = ProfileIcon::automatic_for_program(&draft.program.text);
            editor.configuration.profiles.push(draft);
            editor.configuration_dirty = true;
            editor.clear_dropdown();
            editor.focused_input = None;
            editor.focused_control = None;
            editor.focus_scroll_request = None;
            editor.message = None;
            invalidate_controls_cache(editor);
            cx.notify();
        }
    }

    /// The pane-template page's controls, which the template editor owns.
    fn activate_settings_pane_template_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = pane_templates::activate_pane_template_control(self, control, window, cx);
    }

    /// The project configuration builder's controls.
    fn activate_settings_project_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_project_config_control(control, window, cx);
    }

    pub(crate) fn edit_settings_input(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if settings_save_in_flight(editor) {
            return;
        }
        editor.clear_dropdown();
        let Some(input) = editor.focused_input else {
            return;
        };
        let field = settings_text_field(editor, input);
        let Some(field) = field else {
            return;
        };
        // Settled before the surface's own keys, so `Ctrl-X` cuts rather than
        // typing an `x` and `Shift-Delete` cuts rather than forward-deleting.
        let clipboard = apply_clipboard_shortcut(field, &event.keystroke, cx);
        // Whether the keystroke changed the text. Moving the cursor, selecting
        // and copying do not, and used to be treated as edits all the same: an
        // arrow key was enough to make the dialog believe it had unsaved changes,
        // rebuild the control cache and clear whatever it was showing.
        let edited = match clipboard {
            ClipboardOutcome::Edited => true,
            ClipboardOutcome::Unchanged => false,
            ClipboardOutcome::Ignored => {
                apply_text_field_key(field, &event.keystroke) == TextFieldEdit::Edited
            }
        };
        if !edited {
            // Copying is otherwise silent, and a clipboard that may or may not
            // have been written is the thing worth saying something about.
            if is_copy_chord(&event.keystroke) {
                editor.message =
                    Some((Tone::Info, "Copied the field to the clipboard.".to_owned()));
            }
            cx.notify();
            return;
        }
        record_settings_input_edit(editor, input);
        editor.message = None;
        if matches!(input, SettingsInput::PaneTemplate(_)) {
            pane_templates::schedule_pane_template_validation(self, cx);
        } else {
            cx.notify();
        }
    }
}

/// The argument list of whichever profile `target` names.
pub(crate) fn profile_arguments_mut(
    editor: &mut SettingsEditor,
    target: ProfileTarget,
) -> Option<&mut Vec<TextField>> {
    match target {
        ProfileTarget::Configuration(index) => editor
            .configuration
            .profiles
            .get_mut(index)
            .map(|profile| &mut profile.arguments),
        ProfileTarget::Draft => editor
            .profile_draft
            .as_mut()
            .map(|profile| &mut profile.arguments),
        ProfileTarget::Project(index) => editor
            .project
            .as_mut()
            .and_then(|project| project.form.profiles.get_mut(index))
            .map(|profile| &mut profile.arguments),
    }
}

/// Marks the file a profile's arguments are saved to as changed. A draft is
/// saved with its profile, when it is created.
fn profile_arguments_edited(editor: &mut SettingsEditor, target: ProfileTarget) {
    match target {
        ProfileTarget::Configuration(_) => editor.configuration_dirty = true,
        ProfileTarget::Draft => {}
        ProfileTarget::Project(_) => projects::mark_project_dirty(editor),
    }
    editor.message = None;
    invalidate_controls_cache(editor);
}

/// What typing into `input` owes the form: marking the file that field is
/// saved to as changed, and refreshing whatever is derived from the field.
///
/// Separate from [`Zetta::edit_settings_input`] so the mapping can be tested
/// without a window: a pane-template field once marked the user configuration
/// dirty even in the project builder, whose Save then saw nothing to write.
pub(crate) fn record_settings_input_edit(editor: &mut SettingsEditor, input: SettingsInput) {
    match input {
        SettingsInput::Configuration(field) => {
            editor.configuration_dirty = true;
            if let ConfigTextField::Setting(setting) = field {
                clear_invalid_setting(editor, setting);
            }
            invalidate_controls_cache(editor);
        }
        SettingsInput::Keymap(_) => {
            editor.keymap_dirty = true;
            refresh_keymap_cache(editor);
            invalidate_controls_cache(editor);
        }
        SettingsInput::ThemeSearch => {}
        SettingsInput::FontSearch => {
            Zetta::rebuild_font_search_cache(editor);
        }
        SettingsInput::KeymapSearch => {
            refresh_keymap_cache(editor);
            invalidate_controls_cache(editor);
        }
        SettingsInput::PaneTemplate(_) => {
            // The project builder edits its own copy of the templates, so
            // this has to mark whichever form the field belongs to.
            pane_templates::mark_templates_dirty(editor);
            if matches!(
                input,
                SettingsInput::PaneTemplate(PaneTemplateTextField::Name(_))
            ) {
                pane_templates::refresh_template_names(editor);
            }
            invalidate_controls_cache(editor);
        }
        SettingsInput::Project(_) => {
            projects::mark_project_dirty(editor);
            invalidate_controls_cache(editor);
        }
        SettingsInput::ProfileDraft(_) => {}
    }
}

impl Zetta {
    pub(crate) fn set_settings_toggle(
        &mut self,
        toggle: SettingsToggle,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let inherited_opacity = self.launch_config.inactive_pane_opacity;
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        match toggle {
            SettingsToggle::Setting(setting) => {
                setting.set_switch_shown(&mut editor.configuration, value);
            }
            SettingsToggle::ProfileVisibility(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.hidden = !value;
                }
            }
            SettingsToggle::ProfileDraftVisibility => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.hidden = !value;
                }
            }
            SettingsToggle::ProjectOpacityOverride => {
                if let Some(project) = editor.project.as_mut() {
                    // Turning the override on starts from whatever the user
                    // configuration resolves to, so the slider does not jump.
                    project.form.inactive_pane_opacity = value.then_some(inherited_opacity);
                }
            }
            SettingsToggle::ProjectProfileVisibility(index) => {
                if let Some(profile) = editor
                    .project
                    .as_mut()
                    .and_then(|project| project.form.profiles.get_mut(index))
                {
                    profile.hidden = !value;
                }
            }
        }
        if matches!(
            toggle,
            SettingsToggle::ProjectOpacityOverride | SettingsToggle::ProjectProfileVisibility(_)
        ) {
            projects::mark_project_dirty(editor);
            invalidate_controls_cache(editor);
        } else if !matches!(toggle, SettingsToggle::ProfileDraftVisibility) {
            editor.configuration_dirty = true;
        }
        editor.message = None;
        self.focus_settings_control(SettingsControl::Toggle(toggle), window, cx);
        cx.notify();
    }

    /// The inactive-pane opacity a target currently shows. A project that does
    /// not override it has no slider, so the fallback only matters for the
    /// frame in which the override is being switched on.
    pub(crate) fn settings_opacity(editor: &SettingsEditor, target: OpacityTarget) -> Option<f32> {
        match target {
            OpacityTarget::Configuration => Some(editor.configuration.inactive_pane_opacity),
            OpacityTarget::Project => editor
                .project
                .as_ref()
                .and_then(|project| project.form.inactive_pane_opacity),
            OpacityTarget::PaneTemplateOverlay(path) => {
                pane_templates::overlay_opacity(editor, path)
            }
        }
    }

    pub(crate) fn set_settings_opacity(
        &mut self,
        target: OpacityTarget,
        opacity: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let opacity = opacity.clamp(0., 1.);
        match target {
            OpacityTarget::Configuration => {
                editor.configuration.inactive_pane_opacity = opacity;
                editor.configuration_dirty = true;
                editor.message = None;
            }
            OpacityTarget::Project => {
                let Some(project) = editor
                    .project
                    .as_mut()
                    .filter(|project| project.form.inactive_pane_opacity.is_some())
                else {
                    return;
                };
                project.form.inactive_pane_opacity = Some(opacity);
                projects::mark_project_dirty(editor);
            }
            OpacityTarget::PaneTemplateOverlay(path) => {
                if !pane_templates::set_overlay_opacity(editor, path, opacity) {
                    return;
                }
                invalidate_controls_cache(editor);
            }
        }
        cx.notify();
    }

    pub(crate) fn adjust_settings_opacity(
        &mut self,
        target: OpacityTarget,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(current) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| Self::settings_opacity(editor, target))
        else {
            return;
        };
        self.set_settings_opacity(target, current + direction as f32 / 20., cx);
    }

    pub(crate) fn adjust_numeric_setting(
        &mut self,
        setting: ConfigSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let theme_font_size = editor.terminal_font_size_default;
        setting.step_number(&mut editor.configuration, direction, theme_font_size);
        clear_invalid_setting(editor, setting);
        editor.configuration_dirty = true;
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn begin_numeric_repeat(
        &mut self,
        setting: ConfigSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        let generation = editor.numeric_repeat_generation;
        self.adjust_numeric_setting(setting, direction, cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            loop {
                let repeating = this
                    .update(cx, |this, cx| {
                        let repeating = this
                            .settings_editor
                            .as_ref()
                            .is_some_and(|editor| editor.numeric_repeat_generation == generation);
                        if repeating {
                            this.adjust_numeric_setting(setting, direction, cx);
                        }
                        repeating
                    })
                    .unwrap_or(false);
                if !repeating {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(75))
                    .await;
            }
        })
        .detach();
    }

    pub(crate) fn end_numeric_repeat(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        cx.notify();
    }
}

pub(crate) fn settings_text_field(
    editor: &mut SettingsEditor,
    input: SettingsInput,
) -> Option<&mut TextField> {
    match input {
        SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
        SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
        SettingsInput::PaneTemplate(field) => pane_templates::pane_template_text_mut(editor, field),
        SettingsInput::Project(field) => editor
            .project
            .as_mut()
            .and_then(|project| project.form.text_mut(field)),
        SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
        SettingsInput::FontSearch => editor.font_query.as_mut(),
        SettingsInput::KeymapSearch => Some(&mut editor.keymap_search),
        SettingsInput::ProfileDraft(field) => {
            editor.profile_draft.as_mut().and_then(|draft| match field {
                ProfileDraftField::Name => Some(&mut draft.name),
                ProfileDraftField::Program => Some(&mut draft.program),
                ProfileDraftField::Argument(argument) => draft.arguments.get_mut(argument),
            })
        }
    }
}

#[cfg(test)]
#[path = "../tests/settings_ui/editing.rs"]
mod tests;
