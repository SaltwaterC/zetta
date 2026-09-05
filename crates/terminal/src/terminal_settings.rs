use collections::HashMap;
use gpui::{App, FontFallbacks, FontFeatures, FontWeight, Global, Pixels};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// The value types stay in `settings_content`, which is a schema crate with no
// filesystem or git dependencies. What Zetta dropped is `settings` — the store,
// its keymap loader and the `fs`/`git`/`rope`/`text` tree behind them.
pub use settings_content::{
    AlternateScroll, ShowScrollbar, TerminalBell, TerminalBlink, TerminalLineHeight,
};

use task::Shell;
use theme_settings::FontFamilyName;

/// The terminal settings this process runs with.
///
/// A plain gpui global rather than an entry in Zed's `SettingsStore`: Zetta
/// resolves its configuration itself in `Config`, and the store's only remaining
/// job was to hold this struct. `Default` carries the values the store used to
/// produce, so behaviour is unchanged; `apply_config_settings` overwrites the
/// fields Zetta's own configuration owns.
#[derive(Clone, Debug)]
pub struct TerminalSettings {
    pub shell: Shell,
    pub font_size: Option<Pixels>, // todo(settings_refactor) can be non-optional...
    pub font_family: Option<FontFamilyName>,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_features: Option<FontFeatures>,
    pub font_weight: Option<FontWeight>,
    pub line_height: TerminalLineHeight,
    pub env: HashMap<String, String>,
    pub cursor_shape: CursorShape,
    pub blinking: TerminalBlink,
    pub alternate_scroll: AlternateScroll,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    pub keep_selection_on_copy: bool,
    pub open_links_in_mouse_mode: bool,
    pub max_scroll_history_lines: Option<usize>,
    pub scroll_multiplier: f32,
    pub scrollbar: ScrollbarSettings,
    pub minimum_contrast: f32,
    pub path_hyperlink_regexes: Vec<String>,
    pub path_hyperlink_timeout_ms: u64,
    pub bell: TerminalBell,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ScrollbarSettings {
    /// When to show the scrollbar in the terminal.
    ///
    /// Default: inherits editor scrollbar settings
    pub show: Option<ShowScrollbar>,
}

/// The path-like target patterns a terminal recognises out of the box.
///
/// Kept verbatim from the value Zed's default settings produced, because the
/// second pattern is a carefully built multi-line regex and retyping it is how
/// path hyperlinks quietly stop matching.
fn default_path_hyperlink_regexes() -> Vec<String> {
    vec![
        r#"File "(?<path>[^"]+)", line (?<line>[0-9]+)"#.to_owned(),
        r##"(?x)
(?<path>
    (
        # multi-char path: first char (not opening delimiter, space, or box drawing char)
        [^({\[<\"'`\ ─-╿]
        # middle chars: non-space, and colon/paren only if not followed by digit/paren/space
        ([^\ :(]|[:(][^0-9()\ ])*
        # last char: not closing delimiter or colon
        [^()}\]>\"'`.,;:\ ]
    |
        # single-char path: not delimiter, punctuation, space, or box drawing char
        [^(){}\[\]<>\"'`.,;:\ ─-╿]
    )
    # optional line/column suffix (included in path for PathWithPosition::parse_str)
    (:+[0-9]+(:[0-9]+)?|:?\([0-9]+([,:]?[0-9]+)?\))?
)"##
        .to_owned(),
    ]
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            shell: Shell::System,
            font_size: None,
            font_family: None,
            font_fallbacks: None,
            font_features: None,
            font_weight: Some(FontWeight(400.0)),
            line_height: TerminalLineHeight::Standard,
            env: HashMap::default(),
            cursor_shape: CursorShape::Block,
            blinking: TerminalBlink::TerminalControlled,
            alternate_scroll: AlternateScroll::On,
            option_as_meta: false,
            copy_on_select: false,
            keep_selection_on_copy: true,
            open_links_in_mouse_mode: true,
            max_scroll_history_lines: Some(10_000),
            scroll_multiplier: 1.0,
            scrollbar: ScrollbarSettings { show: None },
            minimum_contrast: 45.0,
            path_hyperlink_regexes: default_path_hyperlink_regexes(),
            path_hyperlink_timeout_ms: 1,
            bell: TerminalBell::Off,
        }
    }
}

impl Global for TerminalSettings {}

impl TerminalSettings {
    /// Installs the defaults. Call once during startup, before anything reads
    /// them; `apply_config_settings` then applies the user's configuration.
    pub fn init(cx: &mut App) {
        cx.set_global(Self::default());
    }

    pub fn get_global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    pub fn update_global(cx: &mut App, update: impl FnOnce(&mut Self)) {
        let mut settings = cx.global::<Self>().clone();
        update(&mut settings);
        cx.set_global(settings);
    }

    /// Replaces the settings wholesale, which is what applying a reloaded
    /// configuration does. Notifies every `observe_global::<TerminalSettings>`
    /// watcher, so live panes pick the change up.
    pub fn override_global(settings: Self, cx: &mut App) {
        cx.set_global(settings);
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Cursor is a block like `█`.
    #[default]
    Block,
    /// Cursor is an underscore like `_`.
    Underline,
    /// Cursor is a vertical bar like `⎸`.
    Bar,
    /// Cursor is a hollow box like `▯`.
    Hollow,
}
