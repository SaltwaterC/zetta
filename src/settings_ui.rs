use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsInput {
    Configuration(ConfigTextField),
    Keymap(KeymapTextField),
    ThemeSearch,
    FontSearch,
    ProfileDraft(ProfileDraftField),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProfileDraftField {
    Name,
    Program,
    Arguments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsDropdown {
    DefaultProfile,
    NewTabProfile,
    Theme,
    WorkingDirectoryScope,
    PaneControlsPosition,
    PaneControlsDefaultVisibility,
    ProfileTheme(usize),
    ProfileDraftTheme,
    BindingAction(usize, usize),
    BindingTemplate(usize, usize),
    BindingProfile(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsToggle {
    PaneSize,
    TitleBarLabels,
    TitleBarButtons,
    ProfileVisibility(usize),
    #[cfg(target_os = "macos")]
    TitleBarMenus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumericSetting {
    FontSize,
    ScrollHistory,
    #[cfg(feature = "http-server")]
    HttpServerPort,
    #[cfg(feature = "tftp-server")]
    TftpServerPort,
}

/// A keyboard-reachable control in the settings dialog. Keeping this separate
/// from the input being edited lets buttons, selectors, and dynamic list rows
/// participate in the same tab order as text fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsControl {
    Tab(SettingsPage),
    Save,
    Close,
    Input(SettingsInput),
    Dropdown(SettingsDropdown),
    Toggle(SettingsToggle),
    Numeric(NumericSetting),
    FontPicker,
    Opacity,
    AddProfile,
    RemoveProfile(usize),
    SearchThemes,
    InstallTheme(Arc<str>),
    RemoveTheme(String),
    RemoveBinding(usize, usize),
    AddBinding(usize),
    AddKeymapSection,
    CloseFontPicker,
    Font(usize),
    CloseProfileDialog,
    CreateProfile,
}

#[derive(Clone)]
pub(crate) struct SettingsEditor {
    pub(crate) page: SettingsPage,
    pub(crate) configuration: ConfigurationForm,
    pub(crate) keymap: KeymapForm,
    pub(crate) profile_names: Arc<[String]>,
    pub(crate) themes: Arc<[String]>,
    pub(crate) theme_extension_query: TextField,
    pub(crate) theme_extensions: Vec<ThemeExtension>,
    pub(crate) installed_theme_extensions: Vec<InstalledThemeExtension>,
    pub(crate) theme_extensions_loading: bool,
    pub(crate) theme_extensions_searched: bool,
    pub(crate) theme_extension_downloading: Option<Arc<str>>,
    pub(crate) actions: Arc<[String]>,
    pub(crate) pane_template_names: Arc<[String]>,
    pub(crate) fonts: Arc<[String]>,
    pub(crate) normalized_fonts: Arc<[String]>,
    pub(crate) font_query: Option<TextField>,
    pub(crate) profile_draft: Option<settings_editor::ProfileForm>,
    pub(crate) settings_scroll: ScrollHandle,
    pub(crate) dropdown_scroll: ScrollHandle,
    pub(crate) font_scroll: UniformListScrollHandle,
    pub(crate) numeric_repeat_generation: u64,
    pub(crate) scroll_geometry_initialized: bool,
    pub(crate) focused_input: Option<SettingsInput>,
    pub(crate) focused_control: Option<SettingsControl>,
    pub(crate) open_dropdown: Option<SettingsDropdown>,
    pub(crate) dropdown_index: usize,
    pub(crate) configuration_dirty: bool,
    pub(crate) keymap_dirty: bool,
    pub(crate) message: Option<(bool, String)>,
}

pub(crate) fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

pub(crate) fn matching_font_indices(normalized_fonts: &[String], query: &str) -> Arc<[usize]> {
    let search = query.to_lowercase();
    normalized_fonts
        .iter()
        .enumerate()
        .filter_map(|(index, font)| (search.is_empty() || font.contains(&search)).then_some(index))
        .collect::<Vec<_>>()
        .into()
}

pub(crate) fn adjusted_scroll_history(current: u64, direction: i32, maximum: u64) -> u64 {
    let step_basis = if direction < 0 {
        current.saturating_sub(1)
    } else {
        current
    };
    let step = match step_basis {
        0..100_000 => 1_000,
        100_000..1_000_000 => 100_000,
        1_000_000..10_000_000 => 1_000_000,
        10_000_000..100_000_000 => 10_000_000,
        _ => 100_000_000,
    };
    if direction < 0 {
        current.saturating_sub(step)
    } else {
        current.saturating_add(step).min(maximum)
    }
}

pub(crate) fn adjacent_settings_control_index(
    len: usize,
    current: Option<usize>,
    reverse: bool,
) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = current.unwrap_or_else(|| if reverse { 0 } else { len - 1 });
    Some(if reverse {
        current.checked_sub(1).unwrap_or(len - 1)
    } else {
        (current + 1) % len
    })
}

pub(crate) fn next_char_boundary(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .chars()
        .next()
        .map(|character| cursor + character.len_utf8())
        .unwrap_or(text.len())
}

impl Zetta {
    pub(crate) fn toggle_settings(
        &mut self,
        _: &ToggleSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_editor.is_some() {
            self.dismiss_settings(window, cx);
            return;
        }

        self.command_palette = None;
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }

        let configuration =
            match ConfigurationForm::load(&self.launch_config.config_path, &self.launch_config) {
                Ok(configuration) => configuration,
                Err(error) => {
                    self.configuration_error = Some(format!("Could not open settings: {error:#}"));
                    cx.notify();
                    return;
                }
            };
        let keymap = match KeymapForm::load(&self.launch_config.keymap_path) {
            Ok(keymap) => keymap,
            Err(error) => {
                self.configuration_error =
                    Some(format!("Could not open keymap settings: {error:#}"));
                cx.notify();
                return;
            }
        };
        let mut actions = window
            .available_actions(cx)
            .into_iter()
            .filter(|action| action_is_enabled_in_build(action.name()))
            .map(|action| action.name().to_owned())
            .collect::<Vec<_>>();
        actions.sort();
        actions.dedup();
        if !actions
            .iter()
            .any(|action| action == ApplyPaneSplitTemplate::name_for_type())
        {
            actions.push(ApplyPaneSplitTemplate::name_for_type().to_owned());
            actions.sort();
        }
        if !actions
            .iter()
            .any(|action| action == OpenProfile::name_for_type())
        {
            actions.push(OpenProfile::name_for_type().to_owned());
            actions.sort();
        }
        let mut pane_template_names = self
            .launch_config
            .pane_split_templates
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        pane_template_names.sort();
        let mut themes = ThemeRegistry::global(cx)
            .list()
            .into_iter()
            .map(|theme| theme.name.to_string())
            .collect::<Vec<_>>();
        themes.sort();
        themes.dedup();
        let installed_theme_extensions = Vec::new();
        let mut fonts = cx.text_system().all_font_names();
        if !fonts.contains(&configuration.terminal_font_family) {
            fonts.push(configuration.terminal_font_family.clone());
        }
        fonts.sort_by_key(|font| font.to_lowercase());
        fonts.dedup();
        self.settings_editor = Some(SettingsEditor {
            page: SettingsPage::Configuration,
            configuration,
            keymap,
            profile_names: self
                .profiles
                .iter()
                .map(|profile| profile.name.clone())
                .collect::<Vec<_>>()
                .into(),
            themes: themes.into(),
            theme_extension_query: TextField::default(),
            theme_extensions: Vec::new(),
            installed_theme_extensions,
            theme_extensions_loading: false,
            theme_extensions_searched: false,
            theme_extension_downloading: None,
            actions: actions.into(),
            pane_template_names: pane_template_names.into(),
            normalized_fonts: fonts
                .iter()
                .map(|font| font.to_lowercase())
                .collect::<Vec<_>>()
                .into(),
            fonts: fonts.into(),
            font_query: None,
            profile_draft: None,
            settings_scroll: ScrollHandle::new(),
            dropdown_scroll: ScrollHandle::new(),
            font_scroll: UniformListScrollHandle::new(),
            numeric_repeat_generation: 0,
            scroll_geometry_initialized: false,
            focused_input: None,
            focused_control: Some(SettingsControl::Tab(SettingsPage::Configuration)),
            open_dropdown: None,
            dropdown_index: 0,
            configuration_dirty: false,
            keymap_dirty: false,
            message: None,
        });
        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let installed = executor
                    .spawn(async move { theme_extensions::installed(&themes_dir) })
                    .await;
                this.update_in(cx, |this, _, cx| {
                    if let (Some(editor), Ok(installed)) =
                        (this.settings_editor.as_mut(), installed)
                    {
                        editor.installed_theme_extensions = installed;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings_editor = None;
        self.focus_active(window, cx);
    }

    pub(crate) fn fetch_theme_extensions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extensions_loading {
            return;
        }
        let query = editor.theme_extension_query.text.trim().to_owned();
        if query.is_empty() {
            editor.theme_extensions.clear();
            editor.theme_extensions_searched = false;
            editor.message = Some((false, "Enter a theme name to search.".to_owned()));
            cx.notify();
            return;
        }
        editor.theme_extensions_loading = true;
        editor.theme_extensions_searched = true;
        editor.message = None;
        let http = cx.http_client();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::fetch(http, &query).await;
                this.update_in(cx, |this, _, cx| {
                    let Some(editor) = this.settings_editor.as_mut() else {
                        return;
                    };
                    editor.theme_extensions_loading = false;
                    match result {
                        Ok(extensions) => editor.theme_extensions = extensions,
                        Err(error) => {
                            editor.message =
                                Some((true, format!("Could not load themes: {error:#}")));
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn download_theme_extension(
        &mut self,
        extension_id: Arc<str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.theme_extension_downloading.is_some() {
            return;
        }
        let Some(extension) = editor
            .theme_extensions
            .iter()
            .find(|extension| extension.id == extension_id)
            .cloned()
        else {
            return;
        };
        editor.theme_extension_downloading = Some(extension_id);
        editor.message = Some((false, format!("Downloading {}…", extension.name)));
        let name = extension.name.clone();
        let http = cx.http_client();
        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let result = theme_extensions::download(
                    http,
                    &extension,
                    &themes_dir,
                    executor.clone(),
                )
                .await;
                let installed_theme_extensions = if result.is_ok() {
                    executor
                        .spawn(async move { theme_extensions::installed(&themes_dir) })
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok(count) => {
                            this.reload_configuration(&ReloadConfiguration, window, cx);
                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.installed_theme_extensions = installed_theme_extensions;
                                editor.themes = themes.into();
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Installed {name} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message =
                                    Some((true, format!("Could not install {name}: {error:#}")));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn remove_theme_extension(
        &mut self,
        extension_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(installed) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| (editor.theme_extension_downloading.is_none()).then_some(editor))
            .and_then(|editor| {
                editor
                    .installed_theme_extensions
                    .iter()
                    .find(|extension| extension.id == extension_id)
            })
            .cloned()
        else {
            return;
        };
        let in_use = self.settings_editor.as_ref().is_some_and(|editor| {
            installed.theme_names.iter().any(|theme| {
                editor.configuration.theme == *theme
                    || editor.configuration.profiles.iter().any(|profile| {
                        profile
                            .theme
                            .as_ref()
                            .is_some_and(|selected| selected == theme)
                    })
            })
        });
        if in_use {
            if let Some(editor) = self.settings_editor.as_mut() {
                editor.message = Some((
                    true,
                    "Choose and save replacement application/profile themes before removing this extension."
                        .to_owned(),
                ));
            }
            cx.notify();
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.theme_extension_downloading = Some(Arc::from(extension_id.clone()));
            editor.message = Some((false, format!("Removing {extension_id}…")));
        }

        let themes_dir = config::themes_dir();
        let executor = cx.background_executor().clone();
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let id_for_work = extension_id.clone();
                let result = executor
                    .spawn(async move {
                        let count = theme_extensions::remove(&id_for_work, &themes_dir)?;
                        let installed = theme_extensions::installed(&themes_dir)?;
                        anyhow::Ok((count, installed))
                    })
                    .await;
                this.update_in(cx, |this, window, cx| {
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.theme_extension_downloading = None;
                    }
                    match result {
                        Ok((count, installed_theme_extensions)) => {
                            let theme_names = installed
                                .theme_names
                                .iter()
                                .cloned()
                                .map(SharedString::from)
                                .collect::<Vec<_>>();
                            let registry = ThemeRegistry::global(cx);
                            registry.remove_user_themes(&theme_names);
                            theme_settings::load_bundled_themes(&registry);
                            this.reload_configuration(&ReloadConfiguration, window, cx);

                            let mut themes = ThemeRegistry::global(cx)
                                .list()
                                .into_iter()
                                .map(|theme| theme.name.to_string())
                                .collect::<Vec<_>>();
                            themes.sort();
                            themes.dedup();
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.themes = themes.into();
                                editor.installed_theme_extensions = installed_theme_extensions;
                                editor.message = Some((
                                    false,
                                    format!(
                                        "Removed {extension_id} ({count} theme file{}). Theme selectors have been reloaded.",
                                        if count == 1 { "" } else { "s" }
                                    ),
                                ));
                            }
                            this.settings_focus.focus(window, cx);
                        }
                        Err(error) => {
                            if let Some(editor) = this.settings_editor.as_mut() {
                                editor.message = Some((
                                    true,
                                    format!("Could not remove {extension_id}: {error:#}"),
                                ));
                            }
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
    }

    pub(crate) fn select_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.page = page;
            editor.message = None;
            editor.focused_input = None;
            editor.focused_control = Some(SettingsControl::Tab(page));
            editor.open_dropdown = None;
            editor.font_query = None;
            editor.profile_draft = None;
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_settings_page(
        &mut self,
        page: SettingsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings_editor.is_none() {
            self.toggle_settings(&ToggleSettings, window, cx);
        }
        self.select_settings_page(page, window, cx);
    }

    pub(crate) fn open_themes(
        &mut self,
        _: &OpenThemes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_page(SettingsPage::Themes, window, cx);
    }

    pub(crate) fn open_keymap(
        &mut self,
        _: &OpenKeymap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings_page(SettingsPage::Keymap, window, cx);
    }

    pub(crate) fn focus_settings_input(
        &mut self,
        input: SettingsInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.focused_input = Some(input);
        editor.focused_control = Some(SettingsControl::Input(input));
        editor.open_dropdown = None;
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
            SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
            SettingsInput::FontSearch => editor.font_query.as_mut(),
            SettingsInput::ProfileDraft(field) => {
                editor.profile_draft.as_mut().map(|draft| match field {
                    ProfileDraftField::Name => &mut draft.name,
                    ProfileDraftField::Program => &mut draft.program,
                    ProfileDraftField::Arguments => &mut draft.arguments,
                })
            }
        };
        if let Some(field) = field {
            field.cursor = field.text.len();
            field.select_all =
                !matches!(input, SettingsInput::ProfileDraft(_)) && !field.text.is_empty();
        }
        self.scroll_settings_control_into_view(&SettingsControl::Input(input));
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn settings_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
        if let Some(query) = editor.font_query.as_ref() {
            let mut controls = vec![
                SettingsControl::CloseFontPicker,
                SettingsControl::Input(SettingsInput::FontSearch),
            ];
            controls.extend(
                matching_font_indices(&editor.normalized_fonts, &query.text)
                    .iter()
                    .copied()
                    .map(SettingsControl::Font),
            );
            return controls;
        }
        if editor.profile_draft.is_some() {
            return vec![
                SettingsControl::CloseProfileDialog,
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Name)),
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Program)),
                SettingsControl::Input(SettingsInput::ProfileDraft(ProfileDraftField::Arguments)),
                SettingsControl::Dropdown(SettingsDropdown::ProfileDraftTheme),
                SettingsControl::CreateProfile,
            ];
        }

        let mut controls = vec![
            SettingsControl::Tab(SettingsPage::Configuration),
            SettingsControl::Tab(SettingsPage::Themes),
            SettingsControl::Tab(SettingsPage::Keymap),
            SettingsControl::Save,
            SettingsControl::Close,
        ];
        match editor.page {
            SettingsPage::Configuration => {
                controls.extend([
                    SettingsControl::Dropdown(SettingsDropdown::DefaultProfile),
                    SettingsControl::Dropdown(SettingsDropdown::NewTabProfile),
                    SettingsControl::Dropdown(SettingsDropdown::Theme),
                    SettingsControl::Numeric(NumericSetting::FontSize),
                    SettingsControl::FontPicker,
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::WorkingDirectory,
                    )),
                    SettingsControl::Dropdown(SettingsDropdown::WorkingDirectoryScope),
                    SettingsControl::Numeric(NumericSetting::ScrollHistory),
                    SettingsControl::Opacity,
                    SettingsControl::Toggle(SettingsToggle::PaneSize),
                    SettingsControl::Toggle(SettingsToggle::TitleBarLabels),
                    SettingsControl::Toggle(SettingsToggle::TitleBarButtons),
                    #[cfg(target_os = "macos")]
                    SettingsControl::Toggle(SettingsToggle::TitleBarMenus),
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsPosition),
                    SettingsControl::Dropdown(SettingsDropdown::PaneControlsDefaultVisibility),
                ]);
                #[cfg(feature = "http-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::HttpServerPort));
                #[cfg(feature = "tftp-server")]
                controls.push(SettingsControl::Numeric(NumericSetting::TftpServerPort));
                for (index, profile) in editor.configuration.profiles.iter().enumerate() {
                    if !profile.detected {
                        controls.extend([
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileName(index),
                            )),
                            SettingsControl::RemoveProfile(index),
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileProgram(index),
                            )),
                            SettingsControl::Input(SettingsInput::Configuration(
                                ConfigTextField::ProfileArguments(index),
                            )),
                        ]);
                    } else {
                        controls.push(SettingsControl::Toggle(SettingsToggle::ProfileVisibility(
                            index,
                        )));
                    }
                    controls.push(SettingsControl::Dropdown(SettingsDropdown::ProfileTheme(
                        index,
                    )));
                }
                controls.push(SettingsControl::AddProfile);
            }
            SettingsPage::Themes => {
                controls.extend([
                    SettingsControl::Input(SettingsInput::ThemeSearch),
                    SettingsControl::SearchThemes,
                ]);
                if editor.theme_extension_downloading.is_none() {
                    controls.extend(
                        editor
                            .installed_theme_extensions
                            .iter()
                            .map(|extension| SettingsControl::RemoveTheme(extension.id.clone())),
                    );
                    controls.extend(
                        editor
                            .theme_extensions
                            .iter()
                            .filter(|extension| {
                                !editor
                                    .installed_theme_extensions
                                    .iter()
                                    .any(|installed| installed.id == extension.id.as_ref())
                            })
                            .map(|extension| SettingsControl::InstallTheme(extension.id.clone())),
                    );
                }
            }
            SettingsPage::Keymap => {
                for (section_index, section) in editor.keymap.sections.iter().enumerate() {
                    controls.push(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Context(section_index),
                    )));
                    for (binding_index, binding) in section.bindings.iter().enumerate() {
                        controls.extend([
                            SettingsControl::Input(SettingsInput::Keymap(
                                KeymapTextField::Keystroke(section_index, binding_index),
                            )),
                            SettingsControl::Dropdown(SettingsDropdown::BindingAction(
                                section_index,
                                binding_index,
                            )),
                        ]);
                        if binding.action_parameter("name").is_some() {
                            controls.push(SettingsControl::Dropdown(
                                SettingsDropdown::BindingTemplate(section_index, binding_index),
                            ));
                        }
                        if binding.action_usize_parameter("slot").is_some() {
                            controls.push(SettingsControl::Dropdown(
                                SettingsDropdown::BindingProfile(section_index, binding_index),
                            ));
                        }
                        controls.push(SettingsControl::RemoveBinding(section_index, binding_index));
                    }
                    controls.push(SettingsControl::AddBinding(section_index));
                }
                controls.push(SettingsControl::AddKeymapSection);
            }
        }
        controls
    }

    fn scroll_settings_control_into_view(&mut self, control: &SettingsControl) {
        let Some(editor) = self.settings_editor.as_ref() else {
            return;
        };
        if editor.font_query.is_some() || editor.profile_draft.is_some() {
            return;
        }
        let controls = Self::settings_controls(editor);
        let Some(index) = controls.iter().position(|candidate| candidate == control) else {
            return;
        };
        const FORM_START: usize = 5;
        if index < FORM_START {
            return;
        }
        let form_index = index - FORM_START;
        let form_count = controls.len().saturating_sub(FORM_START);
        let progress = form_index as f32 / form_count.saturating_sub(1).max(1) as f32;
        let maximum = editor.settings_scroll.max_offset().y;
        let offset = editor.settings_scroll.offset();
        editor
            .settings_scroll
            .set_offset(point(offset.x, -(maximum * progress)));
    }

    pub(crate) fn focus_settings_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let SettingsControl::Input(input) = control {
            self.focus_settings_input(input, window, cx);
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.focused_input = None;
            editor.focused_control = Some(control.clone());
        }
        self.scroll_settings_control_into_view(&control);
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn focus_adjacent_settings_control(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_ref() else {
            return;
        };
        let controls = Self::settings_controls(editor);
        let current = editor.focused_control.as_ref();
        let current =
            current.and_then(|current| controls.iter().position(|control| control == current));
        if let Some(control) = adjacent_settings_control_index(controls.len(), current, reverse)
            .and_then(|index| controls.get(index))
            .cloned()
        {
            self.focus_settings_control(control, window, cx);
        }
    }

    fn settings_dropdown_options(
        editor: &SettingsEditor,
        dropdown: SettingsDropdown,
    ) -> (String, Vec<String>) {
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                let mut options = editor.profile_names.to_vec();
                options.extend(
                    editor
                        .configuration
                        .profiles
                        .iter()
                        .map(|profile| profile.name.text.clone()),
                );
                options.sort();
                options.dedup();
                (editor.configuration.default_profile.clone(), options)
            }
            SettingsDropdown::NewTabProfile => (
                editor.configuration.new_tab_profile.label().to_owned(),
                vec!["Default".to_owned(), "Inherit".to_owned()],
            ),
            SettingsDropdown::Theme => (editor.configuration.theme.clone(), editor.themes.to_vec()),
            SettingsDropdown::WorkingDirectoryScope => (
                editor
                    .configuration
                    .working_directory_scope
                    .label()
                    .to_owned(),
                vec!["None".to_owned(), "Pane".to_owned(), "Tab".to_owned()],
            ),
            SettingsDropdown::PaneControlsPosition => (
                editor
                    .configuration
                    .pane_controls_position
                    .label()
                    .to_owned(),
                vec!["Right".to_owned(), "Left".to_owned()],
            ),
            SettingsDropdown::PaneControlsDefaultVisibility => (
                if editor.configuration.pane_controls_hidden_by_default {
                    "Hidden".to_owned()
                } else {
                    "Visible".to_owned()
                },
                vec!["Visible".to_owned(), "Hidden".to_owned()],
            ),
            SettingsDropdown::ProfileTheme(index) => (
                editor
                    .configuration
                    .profiles
                    .get(index)
                    .and_then(|profile| profile.theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::ProfileDraftTheme => (
                editor
                    .profile_draft
                    .as_ref()
                    .and_then(|profile| profile.theme.clone())
                    .unwrap_or_else(|| "Use application theme".to_owned()),
                std::iter::once("Use application theme".to_owned())
                    .chain(editor.themes.iter().cloned())
                    .collect(),
            ),
            SettingsDropdown::BindingAction(section, binding) => (
                editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .map(BindingForm::action_name)
                    .unwrap_or_default(),
                editor.actions.to_vec(),
            ),
            SettingsDropdown::BindingTemplate(section, binding) => (
                editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .and_then(|binding| binding.action_parameter("name"))
                    .unwrap_or_default(),
                editor.pane_template_names.to_vec(),
            ),
            SettingsDropdown::BindingProfile(section, binding) => {
                let slot = editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .and_then(|binding| binding.action_usize_parameter("slot"))
                    .unwrap_or(1);
                (
                    editor
                        .profile_names
                        .get(slot.saturating_sub(1))
                        .cloned()
                        .unwrap_or_default(),
                    editor.profile_names.to_vec(),
                )
            }
        }
    }

    pub(crate) fn open_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let (selected, options) = Self::settings_dropdown_options(editor, dropdown);
        if options.is_empty() {
            return;
        }
        editor.dropdown_index = options
            .iter()
            .position(|option| option == &selected)
            .unwrap_or(0);
        editor.dropdown_scroll.scroll_to_item(editor.dropdown_index);
        editor.open_dropdown = Some(dropdown);
        cx.notify();
    }

    fn move_open_settings_dropdown(&mut self, direction: i32, cx: &mut Context<Self>) -> bool {
        let Some(editor) = self.settings_editor.as_mut() else {
            return false;
        };
        let Some(dropdown) = editor.open_dropdown else {
            return false;
        };
        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        if options.is_empty() {
            return false;
        }
        editor.dropdown_index = if direction < 0 {
            editor
                .dropdown_index
                .checked_sub(1)
                .unwrap_or(options.len() - 1)
        } else {
            (editor.dropdown_index + 1) % options.len()
        };
        editor.dropdown_scroll.scroll_to_item(editor.dropdown_index);
        cx.notify();
        true
    }

    fn commit_open_settings_dropdown(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((dropdown, value)) = self.settings_editor.as_mut().and_then(|editor| {
            let dropdown = editor.open_dropdown.take()?;
            let (_, options) = Self::settings_dropdown_options(editor, dropdown);
            options
                .get(editor.dropdown_index)
                .cloned()
                .map(|value| (dropdown, value))
        }) else {
            return false;
        };
        self.set_settings_dropdown(dropdown, value, cx);
        true
    }

    fn activate_settings_control(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match control {
            SettingsControl::Tab(page) => self.select_settings_page(page, window, cx),
            SettingsControl::Save => self.save_settings(window, cx),
            SettingsControl::Close => self.dismiss_settings(window, cx),
            SettingsControl::Input(input) => self.focus_settings_input(input, window, cx),
            SettingsControl::Dropdown(dropdown) => self.open_settings_dropdown(dropdown, cx),
            SettingsControl::Toggle(toggle) => {
                let value = self.settings_editor.as_ref().map(|editor| match toggle {
                    SettingsToggle::PaneSize => editor.configuration.hide_pane_size,
                    SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels,
                    SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons,
                    SettingsToggle::ProfileVisibility(index) => editor
                        .configuration
                        .profiles
                        .get(index)
                        .is_some_and(|profile| !profile.hidden),
                    #[cfg(target_os = "macos")]
                    SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus,
                });
                if let Some(value) = value {
                    self.set_settings_toggle(toggle, !value, window, cx);
                }
            }
            SettingsControl::FontPicker => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.font_query = Some(TextField::default());
                    editor.scroll_geometry_initialized = false;
                }
                self.focus_settings_input(SettingsInput::FontSearch, window, cx);
            }
            SettingsControl::Numeric(_) | SettingsControl::Opacity => {}
            SettingsControl::AddProfile => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.profile_draft = Some(settings_editor::ProfileForm {
                        name: TextField::default(),
                        program: TextField::default(),
                        arguments: TextField::default(),
                        theme: None,
                        hidden: false,
                        detected: false,
                    });
                    editor.message = None;
                }
                self.focus_settings_input(
                    SettingsInput::ProfileDraft(ProfileDraftField::Name),
                    window,
                    cx,
                );
            }
            SettingsControl::RemoveProfile(index) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && index < editor.configuration.profiles.len()
                {
                    editor.configuration.profiles.remove(index);
                    editor.configuration_dirty = true;
                    editor.focused_control = None;
                    cx.notify();
                }
            }
            SettingsControl::SearchThemes => self.fetch_theme_extensions(window, cx),
            SettingsControl::InstallTheme(id) => self.download_theme_extension(id, window, cx),
            SettingsControl::RemoveTheme(id) => self.remove_theme_extension(id, window, cx),
            SettingsControl::RemoveBinding(section, binding) => {
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
            SettingsControl::AddBinding(section_index) => {
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
            SettingsControl::AddKeymapSection => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor
                        .keymap
                        .sections
                        .push(KeymapSectionForm::new("Zetta > Terminal"));
                    editor.keymap_dirty = true;
                    cx.notify();
                }
            }
            SettingsControl::CloseFontPicker => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.font_query = None;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    cx.notify();
                }
            }
            SettingsControl::Font(index) => {
                if let Some(editor) = self.settings_editor.as_mut()
                    && let Some(font) = editor.fonts.get(index)
                {
                    editor.configuration.terminal_font_family = font.clone();
                    editor.configuration_dirty = true;
                    editor.font_query = None;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    cx.notify();
                }
            }
            SettingsControl::CloseProfileDialog => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.profile_draft = None;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    cx.notify();
                }
            }
            SettingsControl::CreateProfile => {
                let valid = self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.profile_draft.as_ref().is_some_and(|draft| {
                        !draft.name.text.trim().is_empty() && !draft.program.text.trim().is_empty()
                    })
                });
                if !valid {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.message =
                            Some((true, "Profile name and program are required.".to_owned()));
                    }
                    cx.notify();
                    return;
                }
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor
                        .configuration
                        .profiles
                        .push(editor.profile_draft.take().unwrap());
                    editor.configuration_dirty = true;
                    editor.focused_input = None;
                    editor.focused_control = None;
                    editor.message = None;
                    cx.notify();
                }
            }
        }
    }

    fn edit_settings_input(&mut self, event: &KeyDownEvent, command: bool, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.open_dropdown = None;
        let Some(input) = editor.focused_input else {
            return;
        };
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
            SettingsInput::ThemeSearch => Some(&mut editor.theme_extension_query),
            SettingsInput::FontSearch => editor.font_query.as_mut(),
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
        match event.keystroke.key.as_str() {
            "backspace" => field.backspace(),
            "delete" => field.delete(),
            "left" => field.move_left(),
            "right" => field.move_right(),
            "home" => {
                field.cursor = 0;
                field.select_all = false;
            }
            "end" => {
                field.cursor = field.text.len();
                field.select_all = false;
            }
            "a" if command => field.select_all(),
            "v" if command => {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    field.insert(&text);
                }
            }
            _ if !command && !event.keystroke.modifiers.alt => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    field.insert(text);
                }
            }
            _ => {}
        }
        match input {
            SettingsInput::Configuration(_) => editor.configuration_dirty = true,
            SettingsInput::Keymap(_) => editor.keymap_dirty = true,
            SettingsInput::ThemeSearch
            | SettingsInput::FontSearch
            | SettingsInput::ProfileDraft(_) => {}
        }
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn set_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        value: String,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        editor.open_dropdown = None;
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                editor.configuration.default_profile = value;
            }
            SettingsDropdown::NewTabProfile => {
                editor.configuration.new_tab_profile = if value == "Inherit" {
                    NewTabProfile::Inherit
                } else {
                    NewTabProfile::Default
                };
            }
            SettingsDropdown::Theme => editor.configuration.theme = value,
            SettingsDropdown::WorkingDirectoryScope => {
                editor.configuration.working_directory_scope = match value.as_str() {
                    "None" => WorkingDirectoryScope::None,
                    "Pane" => WorkingDirectoryScope::Pane,
                    _ => WorkingDirectoryScope::Tab,
                };
            }
            SettingsDropdown::PaneControlsPosition => {
                editor.configuration.pane_controls_position = if value == "Left" {
                    PaneControlsPosition::Left
                } else {
                    PaneControlsPosition::Right
                };
            }
            SettingsDropdown::PaneControlsDefaultVisibility => {
                editor.configuration.pane_controls_hidden_by_default = value == "Hidden";
            }
            SettingsDropdown::ProfileTheme(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::ProfileDraftTheme => {
                if let Some(profile) = editor.profile_draft.as_mut() {
                    profile.theme = (value != "Use application theme").then_some(value);
                }
            }
            SettingsDropdown::BindingAction(section, binding) => {
                if let Some(binding) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                {
                    binding.action = if value == ApplyPaneSplitTemplate::name_for_type() {
                        serde_json::json!([
                            value,
                            {
                                "name": editor
                                    .pane_template_names
                                    .first()
                                    .cloned()
                                    .unwrap_or_default()
                            }
                        ])
                    } else if value == OpenProfile::name_for_type() {
                        serde_json::json!([value, { "slot": 1 }])
                    } else {
                        serde_json::Value::String(value)
                    };
                }
            }
            SettingsDropdown::BindingTemplate(section, binding) => {
                if let Some(arguments) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                    .and_then(|binding| binding.action.as_array_mut())
                    .and_then(|action| action.get_mut(1))
                    .and_then(serde_json::Value::as_object_mut)
                {
                    arguments.insert("name".to_owned(), serde_json::Value::String(value));
                }
            }
            SettingsDropdown::BindingProfile(section, binding) => {
                let Some(slot) = editor
                    .profile_names
                    .iter()
                    .position(|profile| profile == &value)
                    .map(|index| index + 1)
                else {
                    return;
                };
                if let Some(arguments) = editor
                    .keymap
                    .sections
                    .get_mut(section)
                    .and_then(|section| section.bindings.get_mut(binding))
                    .and_then(|binding| binding.action.as_array_mut())
                    .and_then(|action| action.get_mut(1))
                    .and_then(serde_json::Value::as_object_mut)
                {
                    arguments.insert("slot".to_owned(), serde_json::json!(slot));
                }
            }
        }
        match dropdown {
            SettingsDropdown::BindingAction(_, _) | SettingsDropdown::BindingTemplate(_, _) => {
                editor.keymap_dirty = true
            }
            SettingsDropdown::ProfileDraftTheme => {}
            _ => editor.configuration_dirty = true,
        }
        editor.message = None;
        cx.notify();
    }

    pub(crate) fn set_settings_toggle(
        &mut self,
        toggle: SettingsToggle,
        value: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        match toggle {
            SettingsToggle::PaneSize => editor.configuration.hide_pane_size = value,
            SettingsToggle::TitleBarLabels => editor.configuration.hide_title_bar_labels = value,
            SettingsToggle::TitleBarButtons => editor.configuration.hide_title_bar_buttons = value,
            SettingsToggle::ProfileVisibility(index) => {
                if let Some(profile) = editor.configuration.profiles.get_mut(index) {
                    profile.hidden = !value;
                }
            }
            #[cfg(target_os = "macos")]
            SettingsToggle::TitleBarMenus => editor.configuration.hide_title_bar_menus = value,
        }
        editor.configuration_dirty = true;
        editor.message = None;
        self.focus_settings_control(SettingsControl::Toggle(toggle), window, cx);
        cx.notify();
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

    pub(crate) fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.settings_editor.as_ref() else {
            return;
        };
        let config_path = self.launch_config.config_path.clone();
        let result = (|| -> Result<()> {
            let keymap = if editor.keymap_dirty {
                let keymap = editor.keymap.to_json()?;
                validate_keymap_contents(&keymap, cx)?;
                Some(keymap)
            } else {
                None
            };
            let configuration = if editor.configuration_dirty {
                let configuration = editor.configuration.to_json()?;
                Config::parse(
                    &configuration,
                    Some(&config_path),
                    self.launch_config.keymap_override.clone(),
                )?;
                Some(configuration)
            } else {
                None
            };

            if let Some(keymap) = keymap {
                save_settings_file(&self.launch_config.keymap_path, &keymap)?;
            }
            if let Some(configuration) = configuration {
                save_settings_file(&config_path, &configuration)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.settings_editor = None;
                self.reload_configuration(&ReloadConfiguration, window, cx);
            }
            Err(error) => {
                if let Some(editor) = self.settings_editor.as_mut() {
                    editor.message = Some((true, format!("Not saved: {error:#}")));
                }
                cx.notify();
            }
        }
    }

    pub(crate) fn settings_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.open_dropdown)
            .is_some()
        {
            match event.keystroke.key.as_str() {
                "escape" => {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.open_dropdown = None;
                        cx.notify();
                    }
                }
                "up" => {
                    self.move_open_settings_dropdown(-1, cx);
                }
                "down" => {
                    self.move_open_settings_dropdown(1, cx);
                }
                "left" => {
                    self.move_open_settings_dropdown(-1, cx);
                }
                "right" => {
                    self.move_open_settings_dropdown(1, cx);
                }
                "enter" | "space" => {
                    self.commit_open_settings_dropdown(cx);
                }
                "tab" => {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.open_dropdown = None;
                    }
                    self.focus_adjacent_settings_control(
                        event.keystroke.modifiers.shift,
                        window,
                        cx,
                    );
                }
                _ => {
                    cx.stop_propagation();
                    return;
                }
            }
            cx.stop_propagation();
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.font_query.is_some() || editor.profile_draft.is_some()
                }) {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.font_query = None;
                        editor.profile_draft = None;
                        editor.focused_input = None;
                        editor.focused_control = None;
                        editor.message = None;
                    }
                    cx.notify();
                } else {
                    self.dismiss_settings(window, cx);
                }
            }
            "s" if command => self.save_settings(window, cx),
            "1" if command => self.select_settings_page(SettingsPage::Configuration, window, cx),
            "2" if command => self.select_settings_page(SettingsPage::Themes, window, cx),
            "3" if command => self.select_settings_page(SettingsPage::Keymap, window, cx),
            "tab" => {
                self.focus_adjacent_settings_control(event.keystroke.modifiers.shift, window, cx)
            }
            "up" | "down" => {
                let direction = if event.keystroke.key == "up" { -1 } else { 1 };
                let control = self
                    .settings_editor
                    .as_ref()
                    .and_then(|editor| editor.focused_control.clone());
                match control {
                    Some(SettingsControl::Dropdown(dropdown)) => {
                        self.open_settings_dropdown(dropdown, cx);
                        self.move_open_settings_dropdown(direction, cx);
                    }
                    Some(SettingsControl::Numeric(setting)) => {
                        self.adjust_numeric_setting(setting, direction, cx)
                    }
                    Some(SettingsControl::Opacity) => {
                        if let Some(editor) = self.settings_editor.as_mut() {
                            editor.configuration.inactive_pane_opacity =
                                (editor.configuration.inactive_pane_opacity
                                    + direction as f32 / 20.)
                                    .clamp(0., 1.);
                            editor.configuration_dirty = true;
                            editor.message = None;
                            cx.notify();
                        }
                    }
                    Some(SettingsControl::Input(_)) => self.edit_settings_input(event, command, cx),
                    _ => self.focus_adjacent_settings_control(direction < 0, window, cx),
                }
            }
            "left" | "right" => {
                let direction = if event.keystroke.key == "left" { -1 } else { 1 };
                let control = self
                    .settings_editor
                    .as_ref()
                    .and_then(|editor| editor.focused_control.clone());
                match control {
                    Some(SettingsControl::Tab(page)) => {
                        let pages = [
                            SettingsPage::Configuration,
                            SettingsPage::Themes,
                            SettingsPage::Keymap,
                        ];
                        let index = pages
                            .iter()
                            .position(|candidate| *candidate == page)
                            .unwrap_or(0);
                        let next = if direction < 0 {
                            index.checked_sub(1).unwrap_or(pages.len() - 1)
                        } else {
                            (index + 1) % pages.len()
                        };
                        self.select_settings_page(pages[next], window, cx);
                        self.focus_settings_control(SettingsControl::Tab(pages[next]), window, cx);
                    }
                    Some(SettingsControl::Dropdown(dropdown)) => {
                        self.open_settings_dropdown(dropdown, cx);
                        self.move_open_settings_dropdown(direction, cx);
                    }
                    Some(SettingsControl::Input(_)) => self.edit_settings_input(event, command, cx),
                    _ => self.focus_adjacent_settings_control(direction < 0, window, cx),
                }
            }
            "enter" => {
                let control = self
                    .settings_editor
                    .as_ref()
                    .and_then(|editor| editor.focused_control.clone());
                if control == Some(SettingsControl::Input(SettingsInput::ThemeSearch)) {
                    self.fetch_theme_extensions(window, cx);
                } else if matches!(control, Some(SettingsControl::Input(_))) {
                    // A text input keeps its editing state when Enter is pressed.
                } else if let Some(control) = control {
                    self.activate_settings_control(control, window, cx);
                }
            }
            "space" => {
                let control = self
                    .settings_editor
                    .as_ref()
                    .and_then(|editor| editor.focused_control.clone());
                if let Some(control) =
                    control.filter(|control| !matches!(control, SettingsControl::Input(_)))
                {
                    self.activate_settings_control(control, window, cx);
                } else {
                    self.edit_settings_input(event, command, cx);
                }
            }
            key => {
                let _ = key;
                self.edit_settings_input(event, command, cx);
            }
        }
        cx.stop_propagation();
    }
}

#[cfg(test)]
#[path = "tests/settings_ui.rs"]
mod tests;
