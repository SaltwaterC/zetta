//! Applying an edit a settings control asks for: activating a control, typing
//! into a text field, toggling a switch, and the sliders and numeric fields
//! that repeat while held.
//!
//! Every path here writes the typed form and then persists it, so the file on
//! disk and the form the page renders from cannot drift apart.

use super::*;

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
                }
                self.focus_settings_input(SettingsInput::FontSearch, window, cx);
            }
            SettingsControl::DefaultTabIconPicker => {
                self.open_default_tab_icon_picker(window, cx);
            }
            SettingsControl::Numeric(_) | SettingsControl::Opacity => {}
            SettingsControl::AddProfile => self.begin_profile_draft(window, cx),
            SettingsControl::RemoveProfile(index) => {
                self.remove_settings_profile(index, window, cx);
            }
            SettingsControl::SearchThemes => self.fetch_theme_extensions(window, cx),
            SettingsControl::InstallTheme(id) => self.download_theme_extension(id, window, cx),
            SettingsControl::RemoveTheme(id) => self.remove_theme_extension(id, window, cx),
            SettingsControl::RemoveBinding(section, binding) => {
                self.remove_settings_binding(section, binding, window, cx);
            }
            SettingsControl::UnbindBinding(section, binding) => {
                self.unbind_settings_binding(section, binding, window, cx);
            }
            SettingsControl::AddBinding(section_index) => {
                self.add_settings_binding(section_index, window, cx);
            }
            SettingsControl::AddKeymapSection => self.add_settings_keymap_section(window, cx),
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
            | SettingsControl::SaveProjectConfig
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
            SettingsControl::ProjectOpacity => {}
            SettingsControl::AddProject => self.add_project_from_settings(window, cx),
            SettingsControl::OpenProject(index) => {
                self.open_project_from_settings(index, window, cx);
            }
            SettingsControl::EditProject(index) => {
                self.edit_project_from_settings(index, window, cx);
            }
            SettingsControl::RemoveProject(index) => {
                self.remove_project_from_settings(index, window, cx);
            }
        }
    }

    /// Closing the dialog, which asks first when a profile draft would be lost.
    fn activate_settings_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .settings_editor
            .as_ref()
            .is_some_and(|editor| editor.profile_draft.is_some())
        {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.dismiss_profile_draft();
                editor.focused_input = None;
                editor.focused_control = None;
                editor.focus_scroll_request = None;
                editor.message = None;
                invalidate_controls_cache(editor);
                cx.notify();
            }
        } else {
            self.dismiss_settings(window, cx);
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
            SettingsToggle::CompactMode => editor.configuration.compact_mode,
            SettingsToggle::PaneSize => editor.configuration.hide_pane_size,
            SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels,
            SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons,
            #[cfg(feature = "session-persistence")]
            SettingsToggle::SessionAutoProtect => {
                editor.configuration.session_persistence_auto_protect
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
            #[cfg(target_os = "macos")]
            SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus,
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
            editor.profile_draft = Some(settings_editor::ProfileForm {
                name: TextField::default(),
                program: TextField::default(),
                arguments: TextField::default(),
                theme: None,
                dark_theme: None,
                icon: None,
                automatic_icon: ProfileIcon::Zetta,
                hidden: false,
                detected: false,
            });
            editor.message = None;
            invalidate_controls_cache(editor);
        }
        self.focus_settings_input(
            SettingsInput::ProfileDraft(ProfileDraftField::Name),
            window,
            cx,
        );
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

    /// Removing a keymap binding.
    fn remove_settings_binding(
        &mut self,
        section: usize,
        binding: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut()
            && let Some(section) = editor.keymap.sections.get_mut(section)
            && binding < section.bindings.len()
        {
            section.bindings.remove(binding);
            editor.keymap_dirty = true;
            editor.focused_control = None;
            cx.notify();
        }
    }

    /// Disabling a built-in binding, which is recorded rather than deleted.
    fn unbind_settings_binding(
        &mut self,
        section: usize,
        binding: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut()
            && let Some(section) = editor.keymap.sections.get_mut(section)
            && binding < section.bindings.len()
        {
            let binding = section.bindings.remove(binding);
            // Add to unbind map
            section.unbind.insert(
                keymap_keystroke_storage(&binding.keystroke.text),
                binding.action_name(),
            );
            editor.keymap_dirty = true;
            editor.focused_control = None;
            cx.notify();
        }
    }

    /// Appending a binding to a keymap context.
    fn add_settings_binding(
        &mut self,
        section_index: usize,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut()
            && let Some(section) = editor.keymap.sections.get_mut(section_index)
        {
            section.bindings.push(BindingForm {
                keystroke: TextField::new("ctrl-shift-x"),
                action: serde_json::Value::String("zetta::NewTab".to_owned()),
            });
            editor.keymap_dirty = true;
            cx.notify();
        }
    }

    /// Appending a keymap context.
    fn add_settings_keymap_section(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor
                .keymap
                .sections
                .push(KeymapSectionForm::new("Zetta > Terminal"));
            editor.keymap_dirty = true;
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
                editor.message = Some((true, "Profile name and program are required.".to_owned()));
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
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
            SettingsInput::PaneTemplate(field) => {
                pane_templates::pane_template_text_mut(editor, field)
            }
            SettingsInput::Project(field) => editor
                .project
                .as_mut()
                .and_then(|project| project.form.text_mut(field)),
            SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
            SettingsInput::FontSearch => editor.font_query.as_mut(),
            SettingsInput::KeymapSearch => Some(&mut editor.keymap_search),
            SettingsInput::ProfileDraft(field) => {
                editor.profile_draft.as_mut().map(|draft| match field {
                    ProfileDraftField::Name => &mut draft.name,
                    ProfileDraftField::Program => &mut draft.program,
                    ProfileDraftField::Arguments => &mut draft.arguments,
                })
            }
        };
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
                editor.message = Some((false, "Copied the field to the clipboard.".to_owned()));
            }
            cx.notify();
            return;
        }
        match input {
            SettingsInput::Configuration(_) => {
                editor.configuration_dirty = true;
                invalidate_controls_cache(editor);
            }
            SettingsInput::Keymap(_) => {
                editor.keymap_dirty = true;
                refresh_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ThemeSearch => {}
            SettingsInput::FontSearch => {
                Self::rebuild_font_search_cache(editor);
            }
            SettingsInput::KeymapSearch => {
                refresh_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::PaneTemplate(_) => {
                editor.configuration_dirty = true;
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
        editor.message = None;
        if matches!(input, SettingsInput::PaneTemplate(_)) {
            pane_templates::schedule_pane_template_validation(self, cx);
        } else {
            cx.notify();
        }
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
            SettingsToggle::CompactMode => editor.configuration.compact_mode = value,
            SettingsToggle::PaneSize => editor.configuration.hide_pane_size = value,
            SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels = value,
            SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons = value,
            #[cfg(feature = "session-persistence")]
            SettingsToggle::SessionAutoProtect => {
                editor.configuration.session_persistence_auto_protect = value;
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
            #[cfg(target_os = "macos")]
            SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus = value,
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
        setting: NumericSetting,
        direction: i32,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let configuration = &mut editor.configuration;
        match setting {
            NumericSetting::FontSize => {
                let current = configuration
                    .terminal_font_size
                    .text
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(14.);
                let value = (current + direction as f32).clamp(6., 100.);
                configuration.terminal_font_size = TextField::new(format!("{value}"));
            }
            NumericSetting::ScrollHistory => {
                let maximum = terminal::MAX_SCROLL_HISTORY_LINES as u64;
                let current = if configuration
                    .max_scroll_history_lines
                    .text
                    .trim()
                    .eq_ignore_ascii_case("max")
                {
                    maximum
                } else {
                    configuration
                        .max_scroll_history_lines
                        .text
                        .trim()
                        .parse::<u64>()
                        .unwrap_or(0)
                        .min(maximum)
                };
                let value = adjusted_scroll_history(current, direction, maximum);
                configuration.max_scroll_history_lines = TextField::new(if value == maximum {
                    "Max".to_owned()
                } else {
                    value.to_string()
                });
            }
            #[cfg(feature = "http-server")]
            NumericSetting::HttpServerPort => {
                let current = configuration
                    .http_server_port
                    .text
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(config::DEFAULT_HTTP_PORT);
                configuration.http_server_port = TextField::new(
                    current
                        .saturating_add_signed(direction as i16)
                        .clamp(1, u16::MAX)
                        .to_string(),
                );
            }
            #[cfg(feature = "tftp-server")]
            NumericSetting::TftpServerPort => {
                let current = configuration
                    .tftp_server_port
                    .text
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(config::DEFAULT_TFTP_SERVER_PORT);
                configuration.tftp_server_port = TextField::new(
                    current
                        .saturating_add_signed(direction as i16)
                        .clamp(1, u16::MAX)
                        .to_string(),
                );
            }
            NumericSetting::SessionRingBytes => {
                let current = configuration
                    .session_ring_bytes
                    .text
                    .trim()
                    .parse::<usize>()
                    .unwrap_or(config::DEFAULT_SESSION_RING_BYTES);
                let value = current
                    .saturating_add_signed(direction.saturating_mul(4096) as isize)
                    .clamp(4 * 1024, config::MAX_SESSION_RING_BYTES);
                configuration.session_ring_bytes = TextField::new(value.to_string());
            }
        }
        editor.configuration_dirty = true;
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn begin_numeric_repeat(
        &mut self,
        setting: NumericSetting,
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
