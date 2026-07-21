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

pub(crate) fn adjacent_profile_draft_field(
    current: Option<ProfileDraftField>,
    reverse: bool,
) -> ProfileDraftField {
    match (current, reverse) {
        (Some(ProfileDraftField::Name), false) => ProfileDraftField::Program,
        (Some(ProfileDraftField::Program), false) => ProfileDraftField::Arguments,
        (Some(ProfileDraftField::Arguments), false) | (None, false) => ProfileDraftField::Name,
        (Some(ProfileDraftField::Arguments), true) => ProfileDraftField::Program,
        (Some(ProfileDraftField::Program), true) => ProfileDraftField::Name,
        (Some(ProfileDraftField::Name), true) | (None, true) => ProfileDraftField::Arguments,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsDropdown {
    DefaultProfile,
    Theme,
    PaneControlsPosition,
    ProfileTheme(usize),
    ProfileDraftTheme,
    BindingAction(usize, usize),
    BindingTemplate(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumericSetting {
    FontSize,
    ScrollHistory,
    HttpServerPort,
}

#[derive(Clone)]
pub(crate) struct SettingsEditor {
    pub(crate) page: SettingsPage,
    pub(crate) configuration: ConfigurationForm,
    pub(crate) keymap: KeymapForm,
    pub(crate) profile_names: Vec<String>,
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
    pub(crate) font_scroll: UniformListScrollHandle,
    pub(crate) numeric_repeat_generation: u64,
    pub(crate) scroll_geometry_initialized: bool,
    pub(crate) focused_input: Option<SettingsInput>,
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
                .collect(),
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
            font_scroll: UniformListScrollHandle::new(),
            numeric_repeat_generation: 0,
            scroll_geometry_initialized: false,
            focused_input: None,
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
            editor.font_query = None;
            editor.profile_draft = None;
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
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
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_adjacent_profile_draft(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.settings_editor.as_ref().and_then(|editor| {
            if let Some(SettingsInput::ProfileDraft(field)) = editor.focused_input {
                Some(field)
            } else {
                None
            }
        });
        let field = adjacent_profile_draft_field(current, reverse);
        self.focus_settings_input(SettingsInput::ProfileDraft(field), window, cx);
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
        match dropdown {
            SettingsDropdown::DefaultProfile => {
                editor.configuration.default_profile = value;
            }
            SettingsDropdown::Theme => editor.configuration.theme = value,
            SettingsDropdown::PaneControlsPosition => {
                editor.configuration.pane_controls_position = if value == "Left" {
                    PaneControlsPosition::Left
                } else {
                    PaneControlsPosition::Right
                };
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
            NumericSetting::HttpServerPort => {
                let current = configuration
                    .http_server_port
                    .text
                    .trim()
                    .parse::<u16>()
                    .unwrap_or(DEFAULT_HTTP_PORT);
                configuration.http_server_port = TextField::new(
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
        match event.keystroke.key.as_str() {
            "escape" => {
                if self.settings_editor.as_ref().is_some_and(|editor| {
                    editor.font_query.is_some() || editor.profile_draft.is_some()
                }) {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.font_query = None;
                        editor.profile_draft = None;
                        editor.focused_input = None;
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
            "tab"
                if self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.profile_draft.is_some()) =>
            {
                self.focus_adjacent_profile_draft(event.keystroke.modifiers.shift, window, cx);
            }
            "enter" => {
                let search = self
                    .settings_editor
                    .as_ref()
                    .is_some_and(|editor| editor.focused_input == Some(SettingsInput::ThemeSearch));
                if search {
                    self.fetch_theme_extensions(window, cx);
                } else if let Some(editor) = self.settings_editor.as_mut() {
                    editor.focused_input = None;
                    cx.notify();
                }
            }
            key => {
                let Some(editor) = self.settings_editor.as_mut() else {
                    return;
                };
                let Some(input) = editor.focused_input else {
                    cx.stop_propagation();
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
                match key {
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
        }
        cx.stop_propagation();
    }
}

#[cfg(test)]
#[path = "tests/settings_ui.rs"]
mod tests;
