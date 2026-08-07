use super::*;

use crate::startup::keymap_keystroke_display;
use std::collections::HashMap;
use std::sync::Arc;

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
    ProfileDraftTheme,
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
    Close,
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

#[derive(Clone, Debug)]
pub(crate) struct KeymapCapture {
    pub(crate) target: KeymapTextField,
    pub(crate) keystroke: Option<KeybindingKeystroke>,
}

pub(crate) fn is_modifier_key(key: &str) -> bool {
    matches!(
        key,
        "alt"
            | "control"
            | "ctrl"
            | "fn"
            | "function"
            | "meta"
            | "platform"
            | "shift"
            | "super"
            | "win"
            | "command"
            | "cmd"
    )
}

fn is_unmodified_capture_control(key: &str, modifiers: &gpui::Modifiers) -> bool {
    !modifiers.modified() && matches!(key, "escape" | "enter")
}

fn keybinding_for_capture(
    keystroke: &gpui::Keystroke,
    keyboard_mapper: &dyn PlatformKeyboardMapper,
) -> KeybindingKeystroke {
    KeybindingKeystroke::new_with_mapper(keystroke.clone(), false, keyboard_mapper)
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

/// A binding matches a query if its own keystroke or action name contains it, or if its
/// section's context does (so searching a context name surfaces all of that context's bindings).
fn keymap_search_matches(
    sections: &[KeymapSectionForm],
    query: &str,
) -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
    if query.is_empty() {
        let section_indices = (0..sections.len()).collect();
        let bindings = sections
            .iter()
            .enumerate()
            .map(|(index, section)| (index, (0..section.bindings.len()).collect()))
            .collect();
        return (section_indices, bindings);
    }
    let mut filtered_sections = Vec::new();
    let mut filtered_bindings = HashMap::new();
    for (section_index, section) in sections.iter().enumerate() {
        let context_matches = section.context.text.to_lowercase().contains(query);
        let matching_bindings: Vec<usize> = section
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                context_matches
                    || binding.keystroke.text.to_lowercase().contains(query)
                    || binding.action_name().to_lowercase().contains(query)
            })
            .map(|(index, _)| index)
            .collect();
        if !matching_bindings.is_empty() {
            filtered_bindings.insert(section_index, matching_bindings);
            filtered_sections.push(section_index);
        }
    }
    (filtered_sections, filtered_bindings)
}

pub(crate) fn rebuild_keymap_search_cache(editor: &mut SettingsEditor) {
    let query = editor.keymap_search.text.trim().to_lowercase();
    let (sections, bindings) = keymap_search_matches(&editor.keymap.sections, &query);
    editor.keymap_search_query_cache = query;
    editor.keymap_filtered_sections = Some(sections);
    editor.keymap_filtered_bindings = bindings;
}

pub(crate) fn invalidate_keymap_cache(editor: &mut SettingsEditor) {
    editor.keymap_filtered_sections = None;
    editor.keymap_search_query_cache.clear();
    editor.keymap_filtered_bindings.clear();
}

/// Returns the current search-filtered (section, bindings) indices, using the cache
/// when it's still valid for the current query and recomputing inline otherwise
/// (render only has `&SettingsEditor`, so it can't refresh the cache in place).
pub(crate) fn keymap_filtered_indices(
    editor: &SettingsEditor,
) -> (Vec<usize>, HashMap<usize, Vec<usize>>) {
    let query = editor.keymap_search.text.trim().to_lowercase();
    if editor.keymap_search_query_cache == query
        && let Some(sections) = editor.keymap_filtered_sections.as_ref()
    {
        return (sections.clone(), editor.keymap_filtered_bindings.clone());
    }
    keymap_search_matches(&editor.keymap.sections, &query)
}

/// A single row of the virtualized keymap list, in display order. Kept in sync with
/// [`build_settings_controls`]'s `SettingsPage::Keymap` arm, which walks the same
/// filtered indices, so keyboard navigation and rendering never disagree about
/// which bindings are visible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeymapRow {
    SectionHeader(usize),
    Binding(usize, usize),
    AddBinding(usize),
    AddSection,
}

pub(crate) fn keymap_rows(editor: &SettingsEditor) -> Vec<KeymapRow> {
    let (filtered_sections, filtered_bindings) = keymap_filtered_indices(editor);
    let mut rows = Vec::new();
    for section_index in filtered_sections {
        rows.push(KeymapRow::SectionHeader(section_index));
        if let Some(binding_indices) = filtered_bindings.get(&section_index) {
            rows.extend(
                binding_indices
                    .iter()
                    .map(|&binding_index| KeymapRow::Binding(section_index, binding_index)),
            );
        }
        rows.push(KeymapRow::AddBinding(section_index));
    }
    rows.push(KeymapRow::AddSection);
    rows
}

pub(crate) fn invalidate_controls_cache(editor: &mut SettingsEditor) {
    editor.controls_cache = None;
    editor.controls_generation = editor.controls_generation.wrapping_add(1);
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

    pub(crate) fn start_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor.keymap.text_mut(target).is_none() {
            return;
        }
        editor.keymap_capture = Some(KeymapCapture {
            target,
            keystroke: None,
        });
        editor.focused_input = None;
        editor.focused_control = Some(SettingsControl::CaptureKeymap(target));
        editor.open_dropdown = None;
        editor.dropdown_query.clear();
        editor.message = None;
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn cancel_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if editor
            .keymap_capture
            .as_ref()
            .is_some_and(|capture| capture.target == target)
        {
            editor.keymap_capture = None;
            self.focus_settings_input(SettingsInput::Keymap(target), window, cx);
        }
    }

    pub(crate) fn commit_keymap_capture(
        &mut self,
        target: KeymapTextField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        let Some(capture) = editor.keymap_capture.take() else {
            return;
        };
        if capture.target != target {
            editor.keymap_capture = Some(capture);
            return;
        }
        let Some(keystroke) = capture.keystroke else {
            editor.keymap_capture = Some(capture);
            return;
        };
        let text = keymap_keystroke_display(&keystroke.unparse());
        if let Some(field) = editor.keymap.text_mut(target) {
            field.text = text;
            field.cursor = field.text.len();
            field.select_all = false;
            editor.keymap_dirty = true;
            editor.message = None;
        }
        self.focus_settings_input(SettingsInput::Keymap(target), window, cx);
    }

    fn settings_controls(editor: &mut SettingsEditor) -> Vec<SettingsControl> {
        // Check cache first
        if let Some(ref cache) = editor.controls_cache {
            return cache.clone();
        }

        let controls = Self::build_settings_controls(editor);
        editor.controls_cache = Some(controls.clone());
        controls
    }

    fn build_settings_controls(editor: &SettingsEditor) -> Vec<SettingsControl> {
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

        if editor.keymap_capture.is_some() {
            return Vec::new();
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
                    SettingsControl::DefaultTabIconPicker,
                    SettingsControl::Numeric(NumericSetting::FontSize),
                    SettingsControl::FontPicker,
                    SettingsControl::Input(SettingsInput::Configuration(
                        ConfigTextField::WorkingDirectory,
                    )),
                    SettingsControl::Dropdown(SettingsDropdown::WorkingDirectoryScope),
                    SettingsControl::Numeric(NumericSetting::ScrollHistory),
                    SettingsControl::Opacity,
                    SettingsControl::Toggle(SettingsToggle::CompactMode),
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
                controls.push(SettingsControl::Input(SettingsInput::KeymapSearch));
                let (filtered_sections, filtered_bindings) = keymap_filtered_indices(editor);
                for section_index in filtered_sections {
                    let Some(section) = editor.keymap.sections.get(section_index) else {
                        continue;
                    };
                    controls.push(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Context(section_index),
                    )));
                    if let Some(binding_indices) = filtered_bindings.get(&section_index) {
                        for &binding_index in binding_indices {
                            let Some(binding) = section.bindings.get(binding_index) else {
                                continue;
                            };
                            controls.extend([
                                SettingsControl::Input(SettingsInput::Keymap(
                                    KeymapTextField::Keystroke(section_index, binding_index),
                                )),
                                SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(
                                    section_index,
                                    binding_index,
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
                            controls
                                .push(SettingsControl::RemoveBinding(section_index, binding_index));
                        }
                    }
                    controls.push(SettingsControl::AddBinding(section_index));
                }
                controls.push(SettingsControl::AddKeymapSection);
            }
        }
        controls
    }

    fn scroll_settings_control_into_view(&mut self, control: &SettingsControl) {
        let Some(editor) = self.settings_editor.as_mut() else {
            return;
        };
        if let Some(query) = editor.font_query.as_ref() {
            if let SettingsControl::Font(index) = control
                && let Some(row_index) =
                    matching_font_position(&editor.normalized_fonts, &query.text, *index)
            {
                editor
                    .font_scroll
                    .scroll_to_item(row_index, ScrollStrategy::Nearest);
            }
            return;
        }
        if editor.profile_draft.is_some() {
            return;
        }
        if editor.page == SettingsPage::Keymap {
            let row = match control {
                SettingsControl::Input(SettingsInput::Keymap(KeymapTextField::Context(
                    section,
                ))) => Some(KeymapRow::SectionHeader(*section)),
                SettingsControl::Input(SettingsInput::Keymap(KeymapTextField::Keystroke(
                    section,
                    binding,
                )))
                | SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingAction(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingTemplate(section, binding))
                | SettingsControl::Dropdown(SettingsDropdown::BindingProfile(section, binding))
                | SettingsControl::RemoveBinding(section, binding) => {
                    Some(KeymapRow::Binding(*section, *binding))
                }
                SettingsControl::AddBinding(section) => Some(KeymapRow::AddBinding(*section)),
                SettingsControl::AddKeymapSection => Some(KeymapRow::AddSection),
                _ => None,
            };
            if let Some(row) = row {
                let rows = keymap_rows(editor);
                if let Some(row_index) = rows.iter().position(|candidate| *candidate == row) {
                    editor
                        .keymap_scroll
                        .scroll_to_item(row_index, ScrollStrategy::Nearest);
                }
            }
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
        self.focus_settings_control_with_scroll(control, window, cx, true);
    }

    pub(crate) fn focus_settings_control_without_scroll(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_settings_control_with_scroll(control, window, cx, false);
    }

    fn focus_settings_control_with_scroll(
        &mut self,
        control: SettingsControl,
        window: &mut Window,
        cx: &mut Context<Self>,
        scroll: bool,
    ) {
        if let SettingsControl::Input(input) = control {
            self.focus_settings_input(input, window, cx);
            return;
        }
        if let Some(editor) = self.settings_editor.as_mut() {
            editor.focused_input = None;
            editor.focused_control = Some(control.clone());
        }
        if scroll {
            self.scroll_settings_control_into_view(&control);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }

    fn focus_adjacent_settings_control(
        &mut self,
        reverse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.settings_editor.as_mut() else {
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

    pub(crate) fn settings_dropdown_options(
        editor: &SettingsEditor,
        dropdown: SettingsDropdown,
    ) -> (String, Arc<[String]>) {
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
                (editor.configuration.default_profile.clone(), options.into())
            }
            SettingsDropdown::NewTabProfile => (
                editor.configuration.new_tab_profile.label().to_owned(),
                Arc::from([String::from("Default"), String::from("Inherit")]),
            ),
            SettingsDropdown::Theme => (editor.configuration.theme.clone(), editor.themes.clone()),
            SettingsDropdown::WorkingDirectoryScope => (
                editor
                    .configuration
                    .working_directory_scope
                    .label()
                    .to_owned(),
                Arc::from([
                    String::from("None"),
                    String::from("Pane"),
                    String::from("Tab"),
                ]),
            ),
            SettingsDropdown::PaneControlsPosition => (
                editor
                    .configuration
                    .pane_controls_position
                    .label()
                    .to_owned(),
                Arc::from([String::from("Right"), String::from("Left")]),
            ),
            SettingsDropdown::PaneControlsDefaultVisibility => (
                if editor.configuration.pane_controls_hidden_by_default {
                    "Hidden".to_owned()
                } else {
                    "Visible".to_owned()
                },
                Arc::from([String::from("Visible"), String::from("Hidden")]),
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
                editor.actions.clone(),
            ),
            SettingsDropdown::BindingTemplate(section, binding) => (
                editor
                    .keymap
                    .sections
                    .get(section)
                    .and_then(|section| section.bindings.get(binding))
                    .and_then(|binding| binding.action_parameter("name"))
                    .unwrap_or_default(),
                editor.pane_template_names.clone(),
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
                    editor.profile_names.clone(),
                )
            }
        }
    }

    pub(crate) fn open_settings_dropdown(
        &mut self,
        dropdown: SettingsDropdown,
        anchor: Point<Pixels>,
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
        editor.dropdown_query.clear();
        // Only one dropdown is ever open at a time, so the previous dropdown's
        // filtered-options entry is dead weight the moment a different one opens.
        editor.dropdown_filtered_options.clear();
        editor
            .dropdown_scroll
            .scroll_to_item(editor.dropdown_index, ScrollStrategy::Nearest);
        editor.dropdown_anchor = anchor;
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
        let matching_indices = fuzzy_match_indices(&options, &editor.dropdown_query);
        if matching_indices.is_empty() {
            return false;
        }
        let current = matching_indices
            .iter()
            .position(|index| *index == editor.dropdown_index)
            .unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(matching_indices.len() - 1)
        } else {
            (current + 1) % matching_indices.len()
        };
        editor.dropdown_index = matching_indices[next];
        editor
            .dropdown_scroll
            .scroll_to_item(editor.dropdown_index, ScrollStrategy::Nearest);
        cx.notify();
        true
    }

    fn type_into_open_settings_dropdown(
        &mut self,
        event: &KeyDownEvent,
        command: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(editor) = self.settings_editor.as_mut() else {
            return false;
        };
        let Some(dropdown) = editor.open_dropdown else {
            return false;
        };

        let changed = if event.keystroke.key == "backspace" {
            editor.dropdown_query.pop().is_some()
        } else if !command
            && !event.keystroke.modifiers.alt
            && let Some(text) = event.keystroke.key_char.as_ref()
            && !text.chars().any(char::is_control)
        {
            editor.dropdown_query.push_str(text);
            true
        } else {
            false
        };
        if !changed {
            return false;
        }

        let (_, options) = Self::settings_dropdown_options(editor, dropdown);
        let query = editor.dropdown_query.clone();
        if let Some(index) = fuzzy_match_index(&options, &query) {
            editor.dropdown_index = index;
            editor
                .dropdown_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        if query.is_empty() {
            editor.dropdown_filtered_options.remove(&dropdown);
        } else {
            editor
                .dropdown_filtered_options
                .insert(dropdown, fuzzy_match_indices(&options, &query));
        }
        cx.notify();
        true
    }

    fn commit_open_settings_dropdown(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((dropdown, value)) = self.settings_editor.as_mut().and_then(|editor| {
            let dropdown = editor.open_dropdown.take()?;
            let (_, options) = Self::settings_dropdown_options(editor, dropdown);
            if !editor.dropdown_query.is_empty()
                && fuzzy_match_indices(&options, &editor.dropdown_query).is_empty()
            {
                editor.open_dropdown = Some(dropdown);
                return None;
            }
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
            SettingsControl::CaptureKeymap(target) => self.start_keymap_capture(target, window, cx),
            SettingsControl::Dropdown(dropdown) => {
                self.open_settings_dropdown(dropdown, window.mouse_position(), cx)
            }
            SettingsControl::Toggle(toggle) => {
                let value = self.settings_editor.as_ref().map(|editor| match toggle {
                    SettingsToggle::CompactMode => editor.configuration.compact_mode,
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
                    Self::rebuild_font_search_cache(editor);
                }
                self.focus_settings_input(SettingsInput::FontSearch, window, cx);
            }
            SettingsControl::DefaultTabIconPicker => {
                self.open_default_tab_icon_picker(window, cx);
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
        editor.dropdown_query.clear();
        let Some(input) = editor.focused_input else {
            return;
        };
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
            SettingsInput::Configuration(_) => {
                editor.configuration_dirty = true;
                invalidate_controls_cache(editor);
            }
            SettingsInput::Keymap(_) => {
                editor.keymap_dirty = true;
                invalidate_keymap_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ThemeSearch => {}
            SettingsInput::FontSearch => {
                Self::rebuild_font_search_cache(editor);
            }
            SettingsInput::KeymapSearch => {
                rebuild_keymap_search_cache(editor);
                invalidate_controls_cache(editor);
            }
            SettingsInput::ProfileDraft(_) => {}
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
        editor.dropdown_query.clear();
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
                editor.keymap_dirty = true;
                invalidate_keymap_cache(editor);
                invalidate_controls_cache(editor);
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
            SettingsToggle::CompactMode => editor.configuration.compact_mode = value,
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
