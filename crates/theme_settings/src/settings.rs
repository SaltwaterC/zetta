#![allow(missing_docs)]

use crate::content_into_gpui::IntoGpui;
use crate::schema::{status_colors_refinement, syntax_overrides, theme_colors_refinement};
use crate::{merge_accent_colors, merge_player_colors};
use collections::HashMap;
use gpui::{
    App, Context, Font, FontFeatures, FontStyle, FontWeight, Global, Pixels, SharedString,
    Subscription, Window, px,
};
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use settings_content::{FontFamilyName, IconThemeName, ThemeAppearanceMode, ThemeName};
use std::sync::Arc;
use theme::{Appearance, DEFAULT_ICON_THEME_NAME, SyntaxTheme, Theme, UiDensity};

const MIN_FONT_SIZE: Pixels = px(6.0);
const MAX_FONT_SIZE: Pixels = px(100.0);
const MIN_LINE_HEIGHT: f32 = 1.0;

pub fn appearance_to_mode(appearance: Appearance) -> ThemeAppearanceMode {
    match appearance {
        Appearance::Light => ThemeAppearanceMode::Light,
        Appearance::Dark => ThemeAppearanceMode::Dark,
    }
}

/// Customizable settings for the UI and theme system.
#[derive(Clone, PartialEq)]
pub struct ThemeSettings {
    /// The UI font size. Determines the size of text in the UI,
    /// as well as the size of a [gpui::Rems] unit.
    ///
    /// Changing this will impact the size of all UI elements.
    ui_font_size: Pixels,
    /// The font used for UI elements.
    pub ui_font: Font,
    /// The font size used for buffers, and the terminal.
    ///
    /// The terminal font size can be overridden using it's own setting.
    buffer_font_size: Pixels,
    /// The font used for buffers, and the terminal.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_font: Font,
    /// The agent UI font family. Determines the family of response text in the agent panel.
    /// Falls back to the UI font family if unset.
    agent_ui_font_family: Option<SharedString>,
    /// The agent font size. Determines the size of text in the agent panel. Falls back to the UI font size if unset.
    agent_ui_font_size: Option<Pixels>,
    /// The agent buffer font family. Determines the family of user messages in the agent panel.
    /// Falls back to the buffer font family if unset.
    agent_buffer_font_family: Option<SharedString>,
    /// The agent buffer font size. Determines the size of user messages in the agent panel.
    agent_buffer_font_size: Option<Pixels>,
    git_commit_buffer_font_size: Option<Pixels>,
    /// The font family to use for rendering in the markdown preview.
    /// Falls back to the UI font family if unset.
    markdown_preview_font_family: Option<SharedString>,
    /// The font family to use for code in the markdown preview.
    /// Falls back to the buffer font family if unset.
    markdown_preview_code_font_family: Option<SharedString>,
    /// The font size to use for rendering in the markdown preview.
    /// Falls back to the UI font size if unset.
    markdown_preview_font_size: Option<Pixels>,
    /// The theme to use for the markdown preview.
    /// Falls back to the main editor theme if unset.
    pub markdown_preview_theme: Option<ThemeSelection>,
    /// The line height for buffers, and the terminal.
    ///
    /// Changing this may affect the spacing of some UI elements.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_line_height: BufferLineHeight,
    /// The current theme selection.
    pub theme: ThemeSelection,
    /// Manual overrides for the active theme.
    ///
    /// Note: This setting is still experimental. See [this tracking issue](https://github.com/zed-industries/zed/issues/18078)
    pub experimental_theme_overrides: Option<settings_content::ThemeStyleContent>,
    /// Manual overrides per theme
    pub theme_overrides: HashMap<String, settings_content::ThemeStyleContent>,
    /// The current icon theme selection.
    pub icon_theme: IconThemeSelection,
    /// The density of the UI.
    /// Note: This setting is still experimental. See [this tracking issue](
    pub ui_density: UiDensity,
    /// The amount of fading applied to unnecessary code.
    pub unnecessary_code_fade: f32,
}

/// Returns the name of the default theme for the given [`Appearance`].
pub fn default_theme(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => settings_content::DEFAULT_LIGHT_THEME,
        Appearance::Dark => settings_content::DEFAULT_DARK_THEME,
    }
}

#[derive(Default)]
struct BufferFontSize(Pixels);

impl Global for BufferFontSize {}

#[derive(Default)]
pub(crate) struct UiFontSize(Pixels);

impl Global for UiFontSize {}

/// In-memory override for the UI font size in the agent panel.
#[derive(Default)]
pub struct AgentUiFontSize(Pixels);

impl Global for AgentUiFontSize {}

/// In-memory override for the buffer font size in the agent panel.
#[derive(Default)]
pub struct AgentBufferFontSize(Pixels);

impl Global for AgentBufferFontSize {}

#[derive(Default)]
pub struct GitCommitBufferFontSize(Pixels);

impl Global for GitCommitBufferFontSize {}

/// In-memory override for the markdown preview font size.
#[derive(Default)]
pub struct MarkdownPreviewFontSize(Pixels);

impl Global for MarkdownPreviewFontSize {}

/// Represents the selection of a theme, which can be either static or dynamic.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum ThemeSelection {
    /// A static theme selection, represented by a single theme name.
    Static(ThemeName),
    /// A dynamic theme selection, which can change based the [ThemeMode].
    Dynamic {
        /// The mode used to determine which theme to use.
        #[serde(default)]
        mode: ThemeAppearanceMode,
        /// The theme to use for light mode.
        light: ThemeName,
        /// The theme to use for dark mode.
        dark: ThemeName,
    },
}

impl From<settings_content::ThemeSelection> for ThemeSelection {
    fn from(selection: settings_content::ThemeSelection) -> Self {
        match selection {
            settings_content::ThemeSelection::Static(theme) => ThemeSelection::Static(theme),
            settings_content::ThemeSelection::Dynamic { mode, light, dark } => {
                ThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl ThemeSelection {
    /// Returns the theme name for the selected [ThemeMode].
    pub fn name(&self, system_appearance: Appearance) -> ThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [ThemeMode] for the [ThemeSelection].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            ThemeSelection::Static(_) => None,
            ThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

/// Represents the selection of an icon theme, which can be either static or dynamic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconThemeSelection {
    /// A static icon theme selection, represented by a single icon theme name.
    Static(IconThemeName),
    /// A dynamic icon theme selection, which can change based on the [`ThemeMode`].
    Dynamic {
        /// The mode used to determine which theme to use.
        mode: ThemeAppearanceMode,
        /// The icon theme to use for light mode.
        light: IconThemeName,
        /// The icon theme to use for dark mode.
        dark: IconThemeName,
    },
}

impl From<settings_content::IconThemeSelection> for IconThemeSelection {
    fn from(selection: settings_content::IconThemeSelection) -> Self {
        match selection {
            settings_content::IconThemeSelection::Static(theme) => {
                IconThemeSelection::Static(theme)
            }
            settings_content::IconThemeSelection::Dynamic { mode, light, dark } => {
                IconThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl IconThemeSelection {
    /// Returns the icon theme name based on the given [`Appearance`].
    pub fn name(&self, system_appearance: Appearance) -> IconThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [`ThemeMode`] for the [`IconThemeSelection`].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            IconThemeSelection::Static(_) => None,
            IconThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

pub use theme::BufferLineHeight;

pub fn buffer_line_height_from_settings(
    value: settings_content::BufferLineHeight,
) -> BufferLineHeight {
    match value {
        settings_content::BufferLineHeight::Comfortable => BufferLineHeight::Comfortable,
        settings_content::BufferLineHeight::Standard => BufferLineHeight::Standard,
        settings_content::BufferLineHeight::Custom(line_height) => {
            BufferLineHeight::Custom(line_height)
        }
    }
}

impl ThemeSettings {
    /// Returns the buffer font size.
    pub fn buffer_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<BufferFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.buffer_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the UI font size.
    pub fn ui_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<UiFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.ui_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the agent panel font size. Falls back to the UI font size if unset.
    pub fn agent_ui_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentUiFontSize>()
            .map(|size| size.0)
            .or(self.agent_ui_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.ui_font_size(cx))
    }

    pub fn agent_ui_font_family(&self) -> &SharedString {
        self.agent_ui_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the agent panel buffer font size.
    pub fn agent_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentBufferFontSize>()
            .map(|size| size.0)
            .or(self.agent_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    pub fn agent_buffer_font_family(&self) -> &SharedString {
        self.agent_buffer_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    pub fn git_commit_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<GitCommitBufferFontSize>()
            .map(|size| size.0)
            .or(self.git_commit_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    /// Returns the font family to use in the markdown preview,
    /// falling back to the UI font family when unset.
    pub fn markdown_preview_font_family(&self) -> &SharedString {
        self.markdown_preview_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the font family to use for code in the markdown preview,
    /// falling back to the buffer font family when unset.
    pub fn markdown_preview_code_font_family(&self) -> &SharedString {
        self.markdown_preview_code_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    /// Returns the markdown preview font size.
    ///
    /// Note: the fallback deliberately uses `self.ui_font_size` instead of `ui_font_size(cx)`,
    /// so that temporary UI zoom does not also resize the markdown preview.
    pub fn markdown_preview_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<MarkdownPreviewFontSize>()
            .map(|size| size.0)
            .or(self.markdown_preview_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| clamp_font_size(self.ui_font_size))
    }

    /// Returns the buffer font size, read from the settings.
    ///
    /// The real buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::buffer_font_size`] to get the real font size.
    pub fn buffer_font_size_settings(&self) -> Pixels {
        self.buffer_font_size
    }

    /// Returns the UI font size, read from the settings.
    ///
    /// The real UI font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::ui_font_size`] to get the real font size.
    pub fn ui_font_size_settings(&self) -> Pixels {
        self.ui_font_size
    }

    /// Returns the agent font size, read from the settings.
    ///
    /// The real agent font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_ui_font_size`] to get the real font size.
    pub fn agent_ui_font_size_settings(&self) -> Option<Pixels> {
        self.agent_ui_font_size
    }

    /// Returns the agent buffer font size, read from the settings.
    ///
    /// The real agent buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_buffer_font_size`] to get the real font size.
    pub fn agent_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.agent_buffer_font_size
    }

    pub fn git_commit_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.git_commit_buffer_font_size
    }

    /// Returns the markdown preview font size, read from the settings.
    ///
    /// The real markdown preview font size is stored in-memory, to support temporary
    /// font size changes. Use [`Self::markdown_preview_font_size`] to get the real font size.
    pub fn markdown_preview_font_size_settings(&self) -> Option<Pixels> {
        self.markdown_preview_font_size
    }

    /// Returns the buffer's line height.
    pub fn line_height(&self) -> f32 {
        f32::max(self.buffer_line_height.value(), MIN_LINE_HEIGHT)
    }

    /// Applies the theme overrides, if there are any, to the current theme.
    pub fn apply_theme_overrides(&self, mut arc_theme: Arc<Theme>) -> Arc<Theme> {
        if let Some(experimental_theme_overrides) = &self.experimental_theme_overrides {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, experimental_theme_overrides);
            arc_theme = Arc::new(theme);
        }

        if let Some(theme_overrides) = self.theme_overrides.get(arc_theme.name.as_ref()) {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, theme_overrides);
            arc_theme = Arc::new(theme);
        }

        arc_theme
    }

    fn modify_theme(base_theme: &mut Theme, theme_overrides: &settings_content::ThemeStyleContent) {
        if let Some(window_background_appearance) = theme_overrides.window_background_appearance {
            base_theme.styles.window_background_appearance =
                window_background_appearance.into_gpui();
        }
        let status_color_refinement = status_colors_refinement(&theme_overrides.status);

        let theme_color_refinement = theme_colors_refinement(
            &theme_overrides.colors,
            &status_color_refinement,
            base_theme.appearance.is_light(),
        );
        base_theme.styles.colors.refine(&theme_color_refinement);
        base_theme.styles.status.refine(&status_color_refinement);
        merge_player_colors(&mut base_theme.styles.player, &theme_overrides.players);
        merge_accent_colors(&mut base_theme.styles.accents, &theme_overrides.accents);
        base_theme.styles.syntax = SyntaxTheme::merge(
            base_theme.styles.syntax.clone(),
            syntax_overrides(theme_overrides),
        );
    }
}

/// Observe changes to the adjusted buffer font size.
pub fn observe_buffer_font_size_adjustment<V: 'static>(
    cx: &mut Context<V>,
    f: impl 'static + Fn(&mut V, &mut Context<V>),
) -> Subscription {
    cx.observe_global::<BufferFontSize>(f)
}

/// Gets the font size, adjusted by the difference between the current buffer font size and the one set in the settings.
pub fn adjusted_font_size(size: Pixels, cx: &App) -> Pixels {
    let adjusted_font_size =
        if let Some(BufferFontSize(adjusted_size)) = cx.try_global::<BufferFontSize>() {
            let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
            let delta = *adjusted_size - buffer_font_size;
            size + delta
        } else {
            size
        };
    clamp_font_size(adjusted_font_size)
}

/// Adjusts the buffer font size, without persisting the result in the settings.
/// This will be effective until the app is restarted.
pub fn adjust_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
    let adjusted_size = cx
        .try_global::<BufferFontSize>()
        .map_or(buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(BufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the buffer font size to the default value.
pub fn reset_buffer_font_size(cx: &mut App) {
    if cx.has_global::<BufferFontSize>() {
        cx.remove_global::<BufferFontSize>();
        cx.refresh_windows();
    }
}

#[allow(missing_docs)]
pub fn setup_ui_font(window: &mut Window, cx: &mut App) -> gpui::Font {
    let (ui_font, ui_font_size) = {
        let theme_settings = ThemeSettings::get_global(cx);
        let font = theme_settings.ui_font.clone();
        (font, theme_settings.ui_font_size(cx))
    };

    window.set_rem_size(ui_font_size);
    ui_font
}

/// Sets the adjusted UI font size.
pub fn adjust_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let ui_font_size = ThemeSettings::get_global(cx).ui_font_size(cx);
    let adjusted_size = cx
        .try_global::<UiFontSize>()
        .map_or(ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(UiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the UI font size to the default value.
pub fn reset_ui_font_size(cx: &mut App) {
    if cx.has_global::<UiFontSize>() {
        cx.remove_global::<UiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of agent responses in the agent panel.
pub fn adjust_agent_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let agent_ui_font_size = ThemeSettings::get_global(cx).agent_ui_font_size(cx);
    let adjusted_size = cx
        .try_global::<AgentUiFontSize>()
        .map_or(agent_ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentUiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the agent response font size in the agent panel to the default value.
pub fn reset_agent_ui_font_size(cx: &mut App) {
    if cx.has_global::<AgentUiFontSize>() {
        cx.remove_global::<AgentUiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of user messages in the agent panel.
pub fn adjust_agent_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let agent_buffer_font_size = ThemeSettings::get_global(cx).agent_buffer_font_size(cx);
    let adjusted_size = cx
        .try_global::<AgentBufferFontSize>()
        .map_or(agent_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the user message font size in the agent panel to the default value.
pub fn reset_agent_buffer_font_size(cx: &mut App) {
    if cx.has_global::<AgentBufferFontSize>() {
        cx.remove_global::<AgentBufferFontSize>();
        cx.refresh_windows();
    }
}

pub fn adjust_git_commit_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let git_commit_buffer_font_size = ThemeSettings::get_global(cx).git_commit_buffer_font_size(cx);
    let adjusted_size = cx
        .try_global::<GitCommitBufferFontSize>()
        .map_or(git_commit_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(GitCommitBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

pub fn reset_git_commit_buffer_font_size(cx: &mut App) {
    if cx.has_global::<GitCommitBufferFontSize>() {
        cx.remove_global::<GitCommitBufferFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of the markdown preview.
pub fn adjust_markdown_preview_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let markdown_preview_font_size = ThemeSettings::get_global(cx).markdown_preview_font_size(cx);
    let adjusted_size = cx
        .try_global::<MarkdownPreviewFontSize>()
        .map_or(markdown_preview_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(MarkdownPreviewFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the markdown preview font size to the default value.
pub fn reset_markdown_preview_font_size(cx: &mut App) {
    if cx.has_global::<MarkdownPreviewFontSize>() {
        cx.remove_global::<MarkdownPreviewFontSize>();
        cx.refresh_windows();
    }
}

/// Ensures font size is within the valid range.
pub fn clamp_font_size(size: Pixels) -> Pixels {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

/// The defaults Zed's settings store used to produce for this struct.
///
/// Zetta owns these now: it resolves its own configuration in `Config`, and the
/// store existed only to hold this struct and `TerminalSettings` as globals.
/// The values are read out of the store as it behaved before the move, so the
/// rendered result is unchanged.
impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            ui_font_size: clamp_font_size(px(16.)),
            ui_font: Font {
                family: ".ZedSans".into(),
                // Zed's default settings disable `calt` for the UI font only.
                features: FontFeatures::disable_ligatures(),
                fallbacks: None,
                weight: FontWeight(400.),
                style: FontStyle::default(),
            },
            buffer_font: Font {
                family: ".ZedMono".into(),
                features: FontFeatures::default(),
                fallbacks: None,
                weight: FontWeight(400.),
                style: FontStyle::default(),
            },
            buffer_font_size: clamp_font_size(px(15.)),
            buffer_line_height: BufferLineHeight::Comfortable,
            agent_ui_font_family: None,
            agent_ui_font_size: None,
            agent_buffer_font_family: None,
            agent_buffer_font_size: None,
            git_commit_buffer_font_size: None,
            markdown_preview_font_family: None,
            markdown_preview_code_font_family: None,
            markdown_preview_font_size: None,
            markdown_preview_theme: None,
            theme: ThemeSelection::Dynamic {
                mode: ThemeAppearanceMode::System,
                light: ThemeName(settings_content::DEFAULT_LIGHT_THEME.into()),
                dark: ThemeName(settings_content::DEFAULT_DARK_THEME.into()),
            },
            experimental_theme_overrides: None,
            theme_overrides: Default::default(),
            icon_theme: IconThemeSelection::Static(IconThemeName(DEFAULT_ICON_THEME_NAME.into())),
            ui_density: UiDensity::default(),
            unnecessary_code_fade: 0.3,
        }
    }
}

impl gpui::Global for ThemeSettings {}

impl ThemeSettings {
    /// Installs the defaults. Call once during startup, before anything reads
    /// them.
    pub fn init_global(cx: &mut App) {
        cx.set_global(Self::default());
    }

    pub fn get_global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Replaces the global, which notifies every `observe_global::<ThemeSettings>`
    /// watcher — the hook the theme and font-size observers hang off.
    pub fn update_global(cx: &mut App, update: impl FnOnce(&mut Self)) {
        let mut settings = cx.global::<Self>().clone();
        update(&mut settings);
        cx.set_global(settings);
    }
}
