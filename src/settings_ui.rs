use super::*;

use crate::startup::keymap_keystroke_display;
use std::collections::HashMap;
use std::sync::Arc;

mod controls;
pub(crate) mod keymap;
mod theme_extensions_ui;

pub(crate) use controls::invalidate_controls_cache;
use keymap::{
    KeymapCapture, is_modifier_key, is_unmodified_capture_control, keybinding_for_capture,
    rebuild_keymap_search_cache,
};
pub(crate) use keymap::{KeymapRow, invalidate_keymap_cache, render_keymap_sticky_candidate};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsInput {
    Configuration(ConfigTextField),
    Keymap(KeymapTextField),
    ThemeSearch,
    FontSearch,
    KeymapSearch,
    ProfileDraft(ProfileDraftField),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProfileDraftField {
    Name,
    Program,
    Arguments,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SettingsDropdown {
    DefaultProfile,
    NewTabProfile,
    Theme,
    WorkingDirectoryScope,
    PaneControlsPosition,
    PaneControlsDefaultVisibility,
    ProfileTheme(usize),
    ProfileIcon(usize),
    ProfileDraftTheme,
    ProfileDraftIcon,
    BindingAction(usize, usize),
    BindingTemplate(usize, usize),
    BindingProfile(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsToggle {
    CompactMode,
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
    Input(SettingsInput),
    CaptureKeymap(KeymapTextField),
    Dropdown(SettingsDropdown),
    Toggle(SettingsToggle),
    Numeric(NumericSetting),
    FontPicker,
    DefaultTabIconPicker,
    Opacity,
    AddProfile,
    RemoveProfile(usize),
    SearchThemes,
    InstallTheme(Arc<str>),
    RemoveTheme(String),
    RemoveBinding(usize, usize),
    UnbindBinding(usize, usize),
    AddBinding(usize),
    AddKeymapSection,
    Font(usize),
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
    pub(crate) keymap_search: TextField,
    pub(crate) settings_scroll: ScrollHandle,
    pub(crate) dropdown_scroll: UniformListScrollHandle,
    pub(crate) font_scroll: UniformListScrollHandle,
    pub(crate) keymap_scroll: UniformListScrollHandle,
    pub(crate) numeric_repeat_generation: u64,
    pub(crate) scroll_geometry_initialized: bool,
    pub(crate) focused_input: Option<SettingsInput>,
    pub(crate) focused_control: Option<SettingsControl>,
    pub(crate) keymap_capture: Option<KeymapCapture>,
    pub(crate) open_dropdown: Option<SettingsDropdown>,
    pub(crate) dropdown_index: usize,
    pub(crate) dropdown_query: String,
    /// Window-space point the open dropdown's option popover is anchored to, captured from
    /// the click (or, for keyboard activation, the cursor position) that opened it. The popover
    /// renders as a sibling of the settings dialog rather than nested in place, because a
    /// `deferred`+`anchored` popover positioned inline inside a virtualized `uniform_list` row
    /// (the keymap bindings list) does not paint correctly.
    pub(crate) dropdown_anchor: Point<Pixels>,
    pub(crate) configuration_dirty: bool,
    pub(crate) keymap_dirty: bool,
    pub(crate) message: Option<(bool, String)>,

    // Cached search/filter results for performance
    pub(crate) keymap_filtered_sections: Option<Vec<usize>>,
    pub(crate) keymap_search_query_cache: String,
    pub(crate) keymap_filtered_bindings: HashMap<usize, Vec<usize>>,
    pub(crate) dropdown_filtered_options: HashMap<SettingsDropdown, Vec<usize>>,
    pub(crate) font_filtered_indices: Option<Arc<[usize]>>,
    pub(crate) font_search_query_cache: String,

    // Controls cache for keyboard navigation
    pub(crate) controls_cache: Option<Vec<SettingsControl>>,
    pub(crate) controls_generation: u64,
}

impl SettingsEditor {
    /// Check if a binding in the given section matches a default binding.
    /// Returns true if the binding (keystroke + action) exists in the default template.
    pub(crate) fn is_default_binding(&self, section_index: usize, binding_index: usize) -> bool {
        let Some(section) = self.keymap.sections.get(section_index) else {
            return false;
        };
        let Some(binding) = section.bindings.get(binding_index) else {
            return false;
        };

        let context = &section.context.text;
        let keystroke = keymap_keystroke_storage(&binding.keystroke.text);
        let action = &binding.action;

        // Load default bindings for this context
        if let Ok(defaults_by_context) = settings_editor::default_bindings_by_context()
            && let Some(default_bindings) = defaults_by_context.get(context)
        {
            return default_bindings
                .get(&keystroke)
                .is_some_and(|default_action| default_action == action);
        }
        false
    }
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

fn matching_font_position(
    normalized_fonts: &[String],
    query: &str,
    font_index: usize,
) -> Option<usize> {
    matching_font_indices(normalized_fonts, query)
        .iter()
        .position(|index| *index == font_index)
}

fn fuzzy_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate = candidate.to_lowercase();
    let query = query.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }

    let mut characters = query.chars();
    let mut wanted = characters.next()?;
    let mut score = 0;
    let mut previous_match = None;
    for (index, character) in candidate.char_indices() {
        if character != wanted {
            continue;
        }
        score += 10;
        if previous_match.is_some_and(|previous| previous + character.len_utf8() == index) {
            score += 8;
        }
        if index == 0
            || candidate[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| matches!(previous, ' ' | ':' | '_' | '-'))
        {
            score += 5;
        }
        previous_match = Some(index);
        match characters.next() {
            Some(next) => wanted = next,
            None => return Some(score - candidate.len() as i32 / 8),
        }
    }
    None
}

fn fuzzy_match_index(options: &[String], query: &str) -> Option<usize> {
    if query.is_empty() {
        return (!options.is_empty()).then_some(0);
    }
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| fuzzy_score(option, query).map(|score| (index, score)))
        .max_by(|(left_index, left_score), (right_index, right_score)| {
            left_score
                .cmp(right_score)
                .then_with(|| right_index.cmp(left_index))
        })
        .map(|(index, _)| index)
}

pub(crate) fn fuzzy_match_indices(options: &[String], query: &str) -> Vec<usize> {
    if query.is_empty() {
        return (0..options.len()).collect();
    }
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| fuzzy_score(option, query).map(|_| index))
        .collect()
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
    fn rebuild_font_search_cache(editor: &mut SettingsEditor) {
        if let Some(font_query) = editor.font_query.as_ref() {
            let query = font_query.text.clone();
            editor.font_search_query_cache = query.clone();
            editor.font_filtered_indices =
                Some(matching_font_indices(&editor.normalized_fonts, &query));
        } else {
            editor.font_filtered_indices = None;
            editor.font_search_query_cache.clear();
        }
    }

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
        // Use cached font enumeration from Zetta.font_cache if available, otherwise compute inline
        let mut fonts = self
            .font_cache
            .get()
            .map(|cache| cache.fonts.to_vec())
            .unwrap_or_else(|| cx.text_system().all_font_names());
        if !fonts.contains(&configuration.terminal_font_family) {
            fonts.push(configuration.terminal_font_family.clone());
        }
        fonts.sort_by_key(|font| font.to_lowercase());
        fonts.dedup();
        let normalized_fonts: Arc<[String]> = fonts
            .iter()
            .map(|font| font.to_lowercase())
            .collect::<Vec<_>>()
            .into();
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
            fonts: fonts.into(),
            normalized_fonts,
            font_query: None,
            profile_draft: None,
            keymap_search: TextField::new(""),
            settings_scroll: ScrollHandle::new(),
            dropdown_scroll: UniformListScrollHandle::new(),
            font_scroll: UniformListScrollHandle::new(),
            keymap_scroll: UniformListScrollHandle::new(),
            numeric_repeat_generation: 0,
            scroll_geometry_initialized: false,
            focused_input: None,
            focused_control: Some(SettingsControl::Tab(SettingsPage::Configuration)),
            keymap_capture: None,
            open_dropdown: None,
            dropdown_index: 0,
            dropdown_query: String::new(),
            dropdown_anchor: Point::default(),
            configuration_dirty: false,
            keymap_dirty: false,
            message: None,

            // Cache fields
            keymap_filtered_sections: None,
            keymap_search_query_cache: String::new(),
            keymap_filtered_bindings: HashMap::new(),
            dropdown_filtered_options: HashMap::new(),
            font_filtered_indices: None,
            font_search_query_cache: String::new(),
            controls_cache: None,
            controls_generation: 0,
        });

        // Initialize keymap search cache on first load
        if let Some(editor) = self.settings_editor.as_mut() {
            rebuild_keymap_search_cache(editor);
        }
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
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.keymap_capture = None;
        }
        self.settings_editor = None;
        self.focus_active(window, cx);
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
            editor.keymap_capture = None;
            editor.open_dropdown = None;
            editor.dropdown_query.clear();
            editor.font_query = None;
            editor.profile_draft = None;
            editor.numeric_repeat_generation = editor.numeric_repeat_generation.wrapping_add(1);
            invalidate_controls_cache(editor);
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
        editor.dropdown_query.clear();
        let field = match input {
            SettingsInput::Configuration(field) => editor.configuration.text_mut(field),
            SettingsInput::Keymap(field) => editor.keymap.text_mut(field),
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
        if let Some(field) = field {
            field.cursor = field.text.len();
            field.select_all =
                !matches!(input, SettingsInput::ProfileDraft(_)) && !field.text.is_empty();
        }
        self.scroll_settings_control_into_view(&SettingsControl::Input(input));
        self.settings_focus.focus(window, cx);
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

    pub(crate) fn save_settings_action(
        &mut self,
        _: &SaveSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_settings(window, cx);
    }

    pub(crate) fn settings_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if let Some(capture) = self
            .settings_editor
            .as_ref()
            .and_then(|editor| editor.keymap_capture.as_ref())
            .cloned()
        {
            let modifiers = event.keystroke.modifiers;
            match event.keystroke.key.as_str() {
                "escape" if is_unmodified_capture_control("escape", &modifiers) => {
                    self.cancel_keymap_capture(capture.target, window, cx)
                }
                "enter" if is_unmodified_capture_control("enter", &modifiers) => {
                    self.commit_keymap_capture(capture.target, window, cx)
                }
                key if !is_modifier_key(key) => {
                    if let Some(editor) = self.settings_editor.as_mut()
                        && let Some(active_capture) = editor.keymap_capture.as_mut()
                    {
                        active_capture.keystroke = Some(keybinding_for_capture(
                            &event.keystroke,
                            cx.keyboard_mapper().as_ref(),
                        ));
                        cx.notify();
                    }
                }
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
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
                        editor.dropdown_query.clear();
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
                "backspace" => {
                    self.type_into_open_settings_dropdown(event, command, cx);
                }
                "tab" => {
                    if let Some(editor) = self.settings_editor.as_mut() {
                        editor.open_dropdown = None;
                        editor.dropdown_query.clear();
                    }
                    self.focus_adjacent_settings_control(
                        event.keystroke.modifiers.shift,
                        window,
                        cx,
                    );
                }
                _ if !command
                    && !event.keystroke.modifiers.alt
                    && event.keystroke.key_char.is_some() =>
                {
                    self.type_into_open_settings_dropdown(event, command, cx);
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
                        self.open_settings_dropdown(dropdown, window.mouse_position(), cx);
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
                        self.open_settings_dropdown(dropdown, window.mouse_position(), cx);
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
