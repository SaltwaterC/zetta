use super::*;

pub(crate) const MAX_PANES_PER_TAB: usize = 64;
pub(crate) const MAX_CONCURRENT_MULTI_COMMAND_SPAWNS: usize = 4;
pub(crate) const TERMINAL_SPAWN_NOTIFY_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const PANE_OUTPUT_DEFAULT_FILENAME: &str = "terminal-output.txt";
pub(crate) const PANE_SPLIT_RATIO_SCALE: u16 = 1_000;
pub(crate) const DEFAULT_PANE_SPLIT_RATIO: u16 = PANE_SPLIT_RATIO_SCALE / 2;
const PANE_ROTATION_AREA_EPSILON: f64 = 1e-9;

pub(crate) fn terminal_size_label(columns: usize, rows: usize) -> String {
    format!("{columns} × {rows}")
}

pub(crate) fn can_add_panes(current: usize, additional: usize) -> bool {
    current
        .checked_add(additional)
        .is_some_and(|total| total <= MAX_PANES_PER_TAB)
}

pub(crate) fn begin_coalesced_notification(pending: &mut bool) -> bool {
    if *pending {
        false
    } else {
        *pending = true;
        true
    }
}

pub(crate) fn begin_pane_output_save(in_progress: &mut bool) -> bool {
    if *in_progress {
        false
    } else {
        *in_progress = true;
        true
    }
}

pub(crate) fn finish_pane_output_save(in_progress: &mut bool) {
    *in_progress = false;
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn prepare_pane_launches<T>(
    pane_ids: impl IntoIterator<Item = u64>,
    mut prepare: impl FnMut(u64) -> T,
) -> Vec<(u64, T)> {
    pane_ids
        .into_iter()
        .map(|pane_id| (pane_id, prepare(pane_id)))
        .collect()
}

pub(crate) fn pane_layout_from_configured_template(
    templates: &HashMap<String, PaneSplitTemplate>,
    name: &str,
    pane_ids: &mut impl Iterator<Item = u64>,
) -> Option<PaneLayout> {
    templates
        .get(name)
        .map(|template| PaneLayout::from_template(template, pane_ids))
}

#[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zetta)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenProfile {
    pub(crate) slot: usize,
}

/// Named font-size steps for a pane's overlay text, matching the values
/// accepted by `zetta overlay --size`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayFontSize {
    Small,
    Base,
    Large,
    ExtraLarge,
    ExtraExtraLarge,
    ExtraExtraExtraLarge,
}

impl OverlayFontSize {
    pub(crate) const DEFAULT: Self = Self::ExtraLarge;

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "sm" => Some(Self::Small),
            "base" => Some(Self::Base),
            "lg" => Some(Self::Large),
            "xl" => Some(Self::ExtraLarge),
            "2xl" => Some(Self::ExtraExtraLarge),
            "3xl" => Some(Self::ExtraExtraExtraLarge),
            _ => None,
        }
    }

    pub(crate) fn cli_name(self) -> &'static str {
        match self {
            Self::Small => "sm",
            Self::Base => "base",
            Self::Large => "lg",
            Self::ExtraLarge => "xl",
            Self::ExtraExtraLarge => "2xl",
            Self::ExtraExtraExtraLarge => "3xl",
        }
    }

    pub(crate) const CLI_NAMES: [&'static str; 6] = ["sm", "base", "lg", "xl", "2xl", "3xl"];

    /// All sizes in ascending order, matching [`Self::CLI_NAMES`].
    pub(crate) const ALL: [Self; 6] = [
        Self::Small,
        Self::Base,
        Self::Large,
        Self::ExtraLarge,
        Self::ExtraExtraLarge,
        Self::ExtraExtraExtraLarge,
    ];

    /// The index of this size in [`Self::ALL`].
    pub(crate) fn index(self) -> usize {
        match self {
            Self::Small => 0,
            Self::Base => 1,
            Self::Large => 2,
            Self::ExtraLarge => 3,
            Self::ExtraExtraLarge => 4,
            Self::ExtraExtraExtraLarge => 5,
        }
    }

    /// The size `delta` positions away in [`Self::ALL`], wrapping around the
    /// ends.
    pub(crate) fn step(self, delta: isize) -> Self {
        let next = (self.index() as isize + delta).rem_euclid(Self::ALL.len() as isize) as usize;
        Self::ALL[next]
    }
}

/// Default opacity for a pane overlay, from `0.0` to `1.0`. Mirrors the
/// `overlay --opacity` CLI default of `85`, applied when a pane has no
/// explicit `overlay_opacity`.
pub(crate) const DEFAULT_OVERLAY_OPACITY: f32 = 0.85;

/// The number of columns in the overlay colour-preset grid.
pub(crate) const OVERLAY_COLOR_PRESET_COLUMNS: usize = 6;

/// Which control inside the overlay-style selector the keyboard is adjusting.
/// Tab (and shift-Tab) cycle through them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayPickerSection {
    /// The overlay's font size.
    FontSize,
    /// The overlay's text colour: hue/saturation/brightness plus a hex field.
    Color,
    /// The overlay's fixed named colour presets.
    ColorPresets,
    /// The overlay's opacity.
    Opacity,
}

impl OverlayPickerSection {
    /// Sections in the order used by Tab navigation, matching the picker's
    /// visual grid from left to right and top to bottom.
    pub(crate) const ALL: [Self; 4] = [
        Self::FontSize,
        Self::Opacity,
        Self::Color,
        Self::ColorPresets,
    ];

    /// The section `delta` positions away in [`Self::ALL`], wrapping around
    /// the ends.
    pub(crate) fn step(self, delta: isize) -> Self {
        let next = (self.index() as isize + delta).rem_euclid(Self::ALL.len() as isize) as usize;
        Self::ALL[next]
    }

    fn index(self) -> usize {
        match self {
            Self::FontSize => 0,
            Self::Opacity => 1,
            Self::Color => 2,
            Self::ColorPresets => 3,
        }
    }
}

/// The style selector shown after entering a pane overlay's text from the
/// command palette. Tracks the pane being styled plus its font size, colour,
/// and opacity (all previewed live on the pane) and the values the pane had
/// when the picker opened so cancel can restore them.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OverlayStylePicker {
    pub(crate) pane_id: u64,
    /// The keyboard-focused control.
    pub(crate) section: OverlayPickerSection,
    /// The highlighted font size.
    pub(crate) font_size: OverlayFontSize,
    /// The pane's `overlay_font_size` when the picker opened, restored on
    /// cancel.
    pub(crate) original_font_size: Option<OverlayFontSize>,
    /// The selected colour's hue, in turns from `0` to `1`.
    pub(crate) hue: f32,
    /// The selected colour's saturation, from `0` to `1`.
    pub(crate) saturation: f32,
    /// The selected colour's brightness (HSV value), from `0` to `1`.
    pub(crate) value: f32,
    /// The pane's `overlay_color` when the picker opened, restored on cancel.
    pub(crate) original_color: Option<gpui::Hsla>,
    /// The keyboard-focused colour preset.
    pub(crate) preset_index: usize,
    /// The highlighted opacity, from `0` to `100`, in `5`% steps.
    pub(crate) opacity_percent: usize,
    /// The pane's `overlay_opacity` when the picker opened, restored on
    /// cancel.
    pub(crate) original_opacity: Option<f32>,
    /// The selected colour as a `#rrggbb` string, typed or derived.
    pub(crate) hex_buffer: String,
}

impl OverlayStylePicker {
    /// Returns the preset matching `color`, or the first preset when the
    /// colour is custom.
    pub(crate) fn preset_index_for_color(color: gpui::Hsla) -> usize {
        let hex = overlay_color_to_hex(color);
        OVERLAY_COLOR_PRESETS
            .iter()
            .position(|preset| preset.hex.eq_ignore_ascii_case(&hex))
            .unwrap_or_default()
    }

    /// Sets the focused preset, clamping it to the available grid.
    pub(crate) fn set_preset_index(&mut self, index: usize) {
        self.preset_index = index.min(OVERLAY_COLOR_PRESETS.len().saturating_sub(1));
    }

    /// Moves the focused preset by rows and columns, clamping at every grid
    /// edge rather than wrapping into the adjacent row.
    pub(crate) fn move_preset_cursor(&mut self, row_delta: isize, column_delta: isize) -> bool {
        let last_index = OVERLAY_COLOR_PRESETS.len().saturating_sub(1);
        let index = self.preset_index.min(last_index);
        let row = index / OVERLAY_COLOR_PRESET_COLUMNS;
        let column = index % OVERLAY_COLOR_PRESET_COLUMNS;
        let last_row = last_index / OVERLAY_COLOR_PRESET_COLUMNS;
        let row = (row as isize + row_delta).clamp(0, last_row as isize) as usize;
        let column = (column as isize + column_delta)
            .clamp(0, (OVERLAY_COLOR_PRESET_COLUMNS - 1) as isize) as usize;
        let next_index = (row * OVERLAY_COLOR_PRESET_COLUMNS + column).min(last_index);
        let changed = self.preset_index != next_index;
        self.preset_index = next_index;
        changed
    }

    /// The highlighted percentage for a pane's opacity, snapped to the
    /// slider's `5`% steps and falling back to the default opacity when the
    /// pane has none.
    pub(crate) fn percent_for_opacity(opacity: Option<f32>) -> usize {
        let percent_fraction = opacity.unwrap_or(DEFAULT_OVERLAY_OPACITY).clamp(0., 1.) * 100.;
        ((percent_fraction / 5.).round() as usize * 5).min(100)
    }

    /// The selected colour as an opaque [`gpui::Hsla`].
    pub(crate) fn color(&self) -> gpui::Hsla {
        hsv_to_hsla(self.hue, self.saturation, self.value)
    }

    /// Replaces the selected colour with the given HSV values and refreshes
    /// the hex buffer from it.
    pub(crate) fn set_color(&mut self, hue: f32, saturation: f32, value: f32) {
        self.hue = hue.rem_euclid(1.);
        self.saturation = saturation.clamp(0., 1.);
        self.value = value.clamp(0., 1.);
        self.refresh_hex();
    }

    /// Replaces the selected colour with a fixed named preset and refreshes
    /// both the HSV state and the hex buffer.
    pub(crate) fn set_color_preset(&mut self, preset: OverlayColorPreset) {
        let (hue, saturation, value) = overlay_picker_hsv_from_hsla(preset.color());
        self.set_color(hue, saturation, value);
        if let Some(index) = OVERLAY_COLOR_PRESETS
            .iter()
            .position(|candidate| *candidate == preset)
        {
            self.preset_index = index;
        }
    }

    /// Rotates the selected colour's hue by `delta` turns.
    pub(crate) fn adjust_hue(&mut self, delta: f32) {
        self.set_color(self.hue + delta, self.saturation, self.value);
    }

    /// Moves the selected colour's saturation by `delta`.
    pub(crate) fn adjust_saturation(&mut self, delta: f32) {
        self.set_color(self.hue, self.saturation + delta, self.value);
    }

    /// Moves the selected colour's brightness by `delta`.
    pub(crate) fn adjust_value(&mut self, delta: f32) {
        self.set_color(self.hue, self.saturation, self.value + delta);
    }

    /// Appends a hex digit to the hex buffer; once the buffer holds a
    /// complete colour — a short `#rgb` or a full `#rrggbb` — it is applied
    /// to the selection. A full `#rrggbb` entry is replaced by a fresh one
    /// when a further digit is typed, but ongoing typing towards a six-digit
    /// code continues through the short-form commit. Returns `true` when the
    /// buffer is now a complete colour.
    pub(crate) fn hex_input(&mut self, ch: char) -> bool {
        if !ch.is_ascii_hexdigit() {
            return false;
        }
        if self.hex_buffer.len() >= 7 {
            self.hex_buffer.clear();
            self.hex_buffer.push('#');
        }
        self.hex_buffer.push(ch);
        if self.hex_buffer.len() == 4 || self.hex_buffer.len() == 7 {
            if let Some(hsla) = overlay_color_from_hex(&self.hex_buffer) {
                self.apply_hsla(hsla);
                return true;
            }
            self.hex_buffer.pop();
        }
        false
    }

    /// Removes the last digit from the hex buffer, keeping the leading `#`.
    /// Returns `true` when a complete colour remains.
    pub(crate) fn hex_backspace(&mut self) -> bool {
        if self.hex_buffer.len() <= 1 {
            return false;
        }
        self.hex_buffer.pop();
        self.hex_buffer.len() >= 4 && overlay_color_from_hex(&self.hex_buffer).is_some()
    }

    /// Applies `hsla` without touching the hex buffer, so a typed short
    /// `#rgb` keeps its literal form while the code is built up digit by
    /// digit.
    fn apply_hsla(&mut self, hsla: gpui::Hsla) {
        let (hue, saturation, value) = hsla_to_hsv(hsla);
        self.hue = hue;
        self.saturation = saturation;
        self.value = value;
    }

    /// Replaces the hex buffer with the selected colour's `#rrggbb` form.
    pub(crate) fn refresh_hex(&mut self) {
        self.hex_buffer = overlay_color_to_hex(self.color());
    }
}

/// Converts a colour from the HSLA space GPUI stores overlays in to the HSV
/// space the picker's hue bar and saturation/brightness square edit in.
/// Hue is in turns (`0` to `1`), as in [`gpui::Hsla`].
pub(crate) fn hsla_to_hsv(hsla: gpui::Hsla) -> (f32, f32, f32) {
    let value = hsla.l + hsla.s * hsla.l.min(1. - hsla.l);
    let saturation = if value <= 0. {
        0.
    } else {
        2. * (1. - hsla.l / value)
    };
    (
        hsla.h.rem_euclid(1.),
        saturation.clamp(0., 1.),
        value.clamp(0., 1.),
    )
}

/// Converts HSV hue (in turns), saturation, and brightness to an opaque
/// [`gpui::Hsla`].
pub(crate) fn hsv_to_hsla(hue: f32, saturation: f32, value: f32) -> gpui::Hsla {
    let lightness = value * (1. - saturation / 2.);
    let hsl_saturation = if lightness <= 0. || lightness >= 1. {
        0.
    } else {
        (value - lightness) / lightness.min(1. - lightness)
    };
    gpui::hsla(
        hue,
        hsl_saturation.clamp(0., 1.),
        lightness.clamp(0., 1.),
        1.,
    )
}

/// A fixed, named overlay-colour preset shared by the CLI, process control,
/// picker, and shell completions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlayColorPreset {
    pub(crate) name: &'static str,
    pub(crate) hex: &'static str,
}

/// The standard named colours accepted by `zetta overlay --color` and shown
/// in the overlay-style picker. Keep this catalogue ordered for both the
/// picker layout and shell completion output.
pub(crate) const OVERLAY_COLOR_PRESETS: [OverlayColorPreset; 12] = [
    OverlayColorPreset {
        name: "black",
        hex: "#000000",
    },
    OverlayColorPreset {
        name: "white",
        hex: "#ffffff",
    },
    OverlayColorPreset {
        name: "gray",
        hex: "#808080",
    },
    OverlayColorPreset {
        name: "red",
        hex: "#ff0000",
    },
    OverlayColorPreset {
        name: "orange",
        hex: "#ffa500",
    },
    OverlayColorPreset {
        name: "yellow",
        hex: "#ffff00",
    },
    OverlayColorPreset {
        name: "green",
        hex: "#008000",
    },
    OverlayColorPreset {
        name: "cyan",
        hex: "#00ffff",
    },
    OverlayColorPreset {
        name: "blue",
        hex: "#0000ff",
    },
    OverlayColorPreset {
        name: "purple",
        hex: "#800080",
    },
    OverlayColorPreset {
        name: "magenta",
        hex: "#ff00ff",
    },
    OverlayColorPreset {
        name: "pink",
        hex: "#ffc0cb",
    },
];

impl OverlayColorPreset {
    /// Returns the preset as an opaque GPUI colour.
    pub(crate) fn color(self) -> gpui::Hsla {
        overlay_color_from_hex(self.hex).expect("overlay colour presets must be valid hex")
    }
}

/// Parses an overlay colour name or hex string into a GPUI colour. Named
/// presets are matched case-insensitively after surrounding whitespace is
/// removed; all existing GPUI hex formats, including alpha forms, remain
/// accepted as the fallback.
pub(crate) fn overlay_color_from_value(value: &str) -> Option<gpui::Hsla> {
    let trimmed = value.trim();
    if let Some(preset) = OVERLAY_COLOR_PRESETS
        .iter()
        .find(|preset| preset.name.eq_ignore_ascii_case(trimmed))
    {
        return Some(preset.color());
    }
    overlay_color_from_hex(trimmed)
}

/// Parses a hex colour string into a GPUI colour. The leading `#` is
/// optional, and the underlying parser accepts `#rgb`, `#rgba`, `#rrggbb`,
/// and `#rrggbbaa` forms.
pub(crate) fn overlay_color_from_hex(value: &str) -> Option<gpui::Hsla> {
    let rgba = gpui::Rgba::try_from(normalize_overlay_color_hex(value).as_str()).ok()?;
    Some(rgba.into())
}

/// The hue the colour picker's square and bar edit when the current overlay
/// colour is achromatic. Keeps a freshly opened picker showing a vivid
/// palette even when the pane is styled with the near-white theme text colour.
pub(crate) const DEFAULT_PICKER_HUE: f32 = 0.6;

/// The HSV seed for a picker opened on `hsla`; when the colour is achromatic
/// its hue is meaningless, so it is swapped for [`DEFAULT_PICKER_HUE`] while
/// the saturation and brightness are preserved (keeping the previewed colour
/// unchanged).
pub(crate) fn overlay_picker_hsv_from_hsla(hsla: gpui::Hsla) -> (f32, f32, f32) {
    let (hue, saturation, value) = hsla_to_hsv(hsla);
    if saturation < 0.05 {
        (DEFAULT_PICKER_HUE, saturation, value)
    } else {
        (hue, saturation, value)
    }
}

/// Formats a colour as a `#rrggbb` string, dropping any alpha.
pub(crate) fn overlay_color_to_hex(hsla: gpui::Hsla) -> String {
    let rgba = hsla.to_rgb();
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgba.r.clamp(0., 1.) * 255.).round() as u8,
        (rgba.g.clamp(0., 1.) * 255.).round() as u8,
        (rgba.b.clamp(0., 1.) * 255.).round() as u8,
    )
}

/// Raw `zetta overlay` request values, before `color` is resolved from its
/// named-or-hex value into a color and `opacity` from a 0-100 percentage into a
/// 0.0-1.0 fraction. Shared by the CLI parser and the process-control client
/// so neither has to thread four separate parameters around.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PaneOverlayRequest {
    pub(crate) text: Option<String>,
    pub(crate) font_size: Option<OverlayFontSize>,
    /// A percentage from `0` to `100`.
    pub(crate) opacity: Option<u8>,
    /// A named preset or an `rgb`, `rgba`, `rrggbb`, or `rrggbbaa` hex color,
    /// with or without a leading `#`.
    pub(crate) color: Option<String>,
}

/// Adds back the leading `#` that [`gpui::Rgba`]'s hex parser requires. The
/// `zetta overlay --color` value never needs one from the user, since `#`
/// starts a comment in most shells and would otherwise always need quoting.
pub(crate) fn normalize_overlay_color_hex(value: &str) -> String {
    format!("#{}", value.trim_start_matches('#'))
}

pub(crate) struct TerminalPane {
    pub(crate) id: u64,
    pub(crate) label_number: usize,
    pub(crate) generated_label: Option<String>,
    pub(crate) custom_label: Option<String>,
    /// Free-form text shown over this pane's terminal content. Ephemeral:
    /// never written to `config.json`, so it is lost when the pane closes.
    pub(crate) overlay_text: Option<String>,
    /// Font size for `overlay_text`; falls back to [`OverlayFontSize::DEFAULT`].
    pub(crate) overlay_font_size: Option<OverlayFontSize>,
    /// Opacity for `overlay_text`, from `0.0` to `1.0`; falls back to a
    /// partly transparent default.
    pub(crate) overlay_opacity: Option<f32>,
    /// Text color for `overlay_text`; falls back to the theme's text color.
    pub(crate) overlay_color: Option<gpui::Hsla>,
    pub(crate) profile: Profile,
    /// Environment overrides requested by a pane split template. Keeping the
    /// unexpanded overrides on the pane lets template application determine
    /// whether the active terminal actually needs to be restarted.
    pub(crate) environment_overrides: HashMap<String, String>,
    pub(crate) terminal: Option<Entity<Terminal>>,
    pub(crate) view: Option<Entity<TerminalView>>,
    pub(crate) error: Option<String>,
    /// Set when the original interactive shell has exited while stacked
    /// entries are still retaining this pane's region.
    pub(crate) base_exited: bool,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) pending_command: Option<String>,
    /// Automatically detected linked-worktree name for this pane's
    /// interactive-shell directory.
    pub(crate) detected_worktree_title: Option<String>,
    /// Shell directory associated with the current detection generation.
    pub(crate) worktree_detection_directory: Option<PathBuf>,
    pub(crate) worktree_detection_generation: u64,
    /// Whether the current detection directory came from an authoritative
    /// shell CWD (or the shell-owned process CWD), so a non-worktree result
    /// may clear the detected title.
    pub(crate) worktree_detection_can_clear: bool,
    /// Command terminals that share this pane's layout region. They are
    /// intentionally not part of [`PaneLayout`]: only the selected entry is
    /// expanded, while the others occupy compact status rows.
    pub(crate) stack: PaneStack,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PaneStackSelection {
    #[default]
    Base,
    Stacked(u64),
}

/// The command entries associated with one interactive terminal pane.
pub(crate) struct PaneStack {
    pub(crate) entries: Vec<StackedPane>,
    pub(crate) selected: PaneStackSelection,
}

impl Default for PaneStack {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: PaneStackSelection::Base,
        }
    }
}

impl PaneStack {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn selected_is_base(&self) -> bool {
        matches!(self.selected, PaneStackSelection::Base)
    }

    pub(crate) fn select_after_base_exit(&mut self) {
        let selected_is_valid = match self.selected {
            PaneStackSelection::Base => false,
            PaneStackSelection::Stacked(id) => self.entries.iter().any(|entry| entry.id == id),
        };
        if !selected_is_valid {
            self.selected = self
                .entries
                .first()
                .map(|entry| PaneStackSelection::Stacked(entry.id))
                .unwrap_or(PaneStackSelection::Base);
        }
    }

    pub(crate) fn selected_entry(&self) -> Option<&StackedPane> {
        let PaneStackSelection::Stacked(id) = self.selected else {
            return None;
        };
        self.entries.iter().find(|entry| entry.id == id)
    }

    pub(crate) fn selected_view(
        &self,
        base: Option<&Entity<TerminalView>>,
    ) -> Option<Entity<TerminalView>> {
        self.selected_entry()
            .and_then(|entry| entry.view.clone())
            .or_else(|| self.selected_is_base().then(|| base.cloned()).flatten())
    }

    pub(crate) fn selected_terminal(
        &self,
        base: Option<&Entity<Terminal>>,
    ) -> Option<Entity<Terminal>> {
        self.selected_entry()
            .and_then(|entry| entry.terminal.clone())
            .or_else(|| self.selected_is_base().then(|| base.cloned()).flatten())
    }

    pub(crate) fn select(&mut self, selection: PaneStackSelection) -> bool {
        let valid = match selection {
            PaneStackSelection::Base => true,
            PaneStackSelection::Stacked(id) => self.entries.iter().any(|entry| entry.id == id),
        };
        if valid {
            self.selected = selection;
        }
        valid
    }

    pub(crate) fn cycle(&mut self, forward: bool) -> Option<PaneStackSelection> {
        if self.entries.is_empty() {
            self.selected = PaneStackSelection::Base;
            return None;
        }

        let current = match self.selected {
            PaneStackSelection::Base => 0,
            PaneStackSelection::Stacked(id) => self
                .entries
                .iter()
                .position(|entry| entry.id == id)
                .map(|index| index + 1)
                .unwrap_or(0),
        };
        let count = self.entries.len() + 1;
        let next = if forward {
            (current + 1) % count
        } else {
            current.checked_sub(1).unwrap_or(count - 1)
        };
        self.selected = if next == 0 {
            PaneStackSelection::Base
        } else {
            PaneStackSelection::Stacked(self.entries[next - 1].id)
        };
        Some(self.selected)
    }

    pub(crate) fn cycle_without_base(&mut self, forward: bool) -> Option<PaneStackSelection> {
        if self.entries.is_empty() {
            self.selected = PaneStackSelection::Base;
            return None;
        }

        let current = match self.selected {
            PaneStackSelection::Stacked(id) => self.entries.iter().position(|entry| entry.id == id),
            PaneStackSelection::Base => None,
        };
        let next = match current {
            Some(index) if forward => (index + 1) % self.entries.len(),
            Some(index) => index.checked_sub(1).unwrap_or(self.entries.len() - 1),
            None if forward => 0,
            None => self.entries.len() - 1,
        };

        self.selected = PaneStackSelection::Stacked(self.entries[next].id);
        Some(self.selected)
    }

    pub(crate) fn push(&mut self, entry: StackedPane) -> bool {
        if self.entries.len() >= MAX_PANES_PER_TAB.saturating_sub(1) {
            return false;
        }
        let id = entry.id;
        self.entries.push(entry);
        self.selected = PaneStackSelection::Stacked(id);
        true
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<StackedPane> {
        let index = self.entries.iter().position(|entry| entry.id == id)?;
        let was_selected = self.selected == PaneStackSelection::Stacked(id);
        let entry = self.entries.remove(index);
        if was_selected {
            self.selected = if let Some(entry) = self.entries.get(index) {
                PaneStackSelection::Stacked(entry.id)
            } else if let Some(entry) = self.entries.get(index.saturating_sub(1)) {
                PaneStackSelection::Stacked(entry.id)
            } else {
                PaneStackSelection::Base
            };
        } else if let PaneStackSelection::Stacked(selected) = self.selected
            && !self.entries.iter().any(|entry| entry.id == selected)
        {
            self.selected = PaneStackSelection::Base;
        }
        Some(entry)
    }

    pub(crate) fn repair_selection(&mut self) {
        if let PaneStackSelection::Stacked(id) = self.selected
            && !self.entries.iter().any(|entry| entry.id == id)
        {
            self.selected = self
                .entries
                .last()
                .map(|entry| PaneStackSelection::Stacked(entry.id))
                .unwrap_or(PaneStackSelection::Base);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum StackedPaneState {
    #[default]
    Starting,
    Running,
    Completed,
    Failed,
}

pub(crate) struct StackedPane {
    pub(crate) id: u64,
    pub(crate) command: String,
    pub(crate) profile: Profile,
    pub(crate) terminal: Option<Entity<Terminal>>,
    pub(crate) view: Option<Entity<TerminalView>>,
    pub(crate) state: StackedPaneState,
    pub(crate) exit_code: Option<i32>,
    pub(crate) error: Option<String>,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
}

impl StackedPane {
    pub(crate) fn new(
        id: u64,
        command: String,
        profile: Profile,
        working_directory: Option<PathBuf>,
        wsl_directory: Option<String>,
    ) -> Self {
        Self {
            id,
            command,
            profile,
            terminal: None,
            view: None,
            state: StackedPaneState::Starting,
            exit_code: None,
            error: None,
            working_directory,
            wsl_directory,
        }
    }
}

pub(crate) struct TerminalSpawnSettings {
    pub(crate) cursor_shape: terminal::terminal_settings::CursorShape,
    pub(crate) alternate_scroll: terminal::terminal_settings::AlternateScroll,
    pub(crate) max_scroll_history_lines: Option<usize>,
    pub(crate) path_hyperlink_regexes: Vec<String>,
    pub(crate) path_hyperlink_timeout_ms: u64,
}

pub(crate) struct QueuedTerminalLaunch {
    pub(crate) tab_id: u64,
    pub(crate) pane_id: u64,
    pub(crate) profile: Profile,
    pub(crate) working_directory: Option<PathBuf>,
    pub(crate) wsl_directory: Option<String>,
    pub(crate) wsl_cwd_file: Option<PathBuf>,
    pub(crate) terminal_theme: Option<Arc<Theme>>,
    pub(crate) settings: Arc<TerminalSpawnSettings>,
}

pub(crate) struct BoundedLaunchQueue<T> {
    pending: VecDeque<T>,
    in_flight: usize,
    limit: usize,
}

impl<T> BoundedLaunchQueue<T> {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "a launch queue must allow at least one launch");
        Self {
            pending: VecDeque::new(),
            in_flight: 0,
            limit,
        }
    }

    pub(crate) fn extend(&mut self, launches: impl IntoIterator<Item = T>) {
        self.pending.extend(launches);
    }

    pub(crate) fn pop_ready(&mut self) -> Option<T> {
        if self.in_flight >= self.limit {
            return None;
        }
        let launch = self.pending.pop_front()?;
        self.in_flight += 1;
        Some(launch)
    }

    pub(crate) fn complete(&mut self) {
        self.in_flight = self
            .in_flight
            .checked_sub(1)
            .expect("only an in-flight launch can complete");
    }
}

impl TerminalSpawnSettings {
    pub(crate) fn current(cx: &App) -> Self {
        let settings = TerminalSettings::get_global(cx);
        Self {
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes: settings.path_hyperlink_regexes.clone(),
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
        }
    }

    pub(crate) fn path_hyperlink_regexes(&mut self, final_spawn: bool) -> Vec<String> {
        clone_or_take_for_final_spawn(&mut self.path_hyperlink_regexes, final_spawn)
    }
}

pub(crate) fn clone_or_take_for_final_spawn<T: Clone + Default>(
    value: &mut T,
    final_spawn: bool,
) -> T {
    if final_spawn {
        std::mem::take(value)
    } else {
        value.clone()
    }
}

impl TerminalPane {
    pub(crate) fn new(id: u64, profile: Profile) -> Self {
        Self {
            id,
            label_number: 0,
            generated_label: None,
            custom_label: None,
            overlay_text: None,
            overlay_font_size: None,
            overlay_opacity: None,
            overlay_color: None,
            profile,
            environment_overrides: HashMap::new(),
            terminal: None,
            view: None,
            error: None,
            base_exited: false,
            wsl_cwd_file: None,
            pending_command: None,
            detected_worktree_title: None,
            worktree_detection_directory: None,
            worktree_detection_generation: 0,
            worktree_detection_can_clear: false,
            stack: PaneStack::default(),
        }
    }

    pub(crate) fn with_label_number(mut self, label_number: usize) -> Self {
        self.label_number = label_number;
        self
    }

    pub(crate) fn with_generated_label(mut self, label: String) -> Self {
        self.generated_label = Some(label);
        self
    }

    pub(crate) fn with_pending_command(mut self, command: Option<String>) -> Self {
        self.pending_command = command;
        self
    }

    pub(crate) fn with_wsl_cwd_file(mut self, file: Option<PathBuf>) -> Self {
        self.wsl_cwd_file = file;
        self
    }

    pub(crate) fn with_environment_overrides(
        mut self,
        environment: HashMap<String, String>,
    ) -> Self {
        self.environment_overrides = environment;
        self
    }

    pub(crate) fn label(&self) -> String {
        self.custom_label
            .clone()
            .or_else(|| self.generated_label.clone())
            .unwrap_or_else(|| format!("Pane {}", self.label_number))
    }

    pub(crate) fn wsl_working_directory(&self, cx: &App) -> Option<String> {
        if let Some(directory) = self.terminal.as_ref().and_then(|terminal| {
            terminal
                .read(cx)
                .reported_working_directory()
                .map(str::to_owned)
        }) {
            return Some(directory);
        }

        let path = self.wsl_cwd_file.as_ref()?;
        let directory = fs::read_to_string(path).ok()?;
        let directory = directory.trim_end_matches(['\r', '\n']);
        (directory.starts_with('/') && !directory.contains(['\r', '\n', '\0']))
            .then(|| directory.to_owned())
    }

    pub(crate) fn working_directory(&self, cx: &App) -> Option<PathBuf> {
        let terminal = self.terminal.as_ref()?.read(cx);
        if let Some((root, _)) = msys2_profile(&self.profile.command)
            && let Some(directory) = terminal.reported_working_directory()
            && let Some(directory) = msys2_path_to_windows(&root, directory)
        {
            return Some(directory);
        }
        terminal.working_directory()
    }

    pub(crate) fn selected_view(&self) -> Option<Entity<TerminalView>> {
        self.stack.selected_view(self.view.as_ref())
    }

    pub(crate) fn selected_terminal(&self) -> Option<Entity<Terminal>> {
        self.stack.selected_terminal(self.terminal.as_ref())
    }

    /// Returns the stack entry represented by the focused terminal view. The
    /// model selection normally tracks focus, but keeping this fallback makes
    /// close actions reliable while a newly spawned stack view is replacing
    /// the host view's focus.
    pub(crate) fn focused_stack_selection(&self, window: &Window, cx: &App) -> PaneStackSelection {
        if let Some(entry) = self.stack.entries.iter().find(|entry| {
            entry
                .view
                .as_ref()
                .is_some_and(|view| view.focus_handle(cx).is_focused(window))
        }) {
            return PaneStackSelection::Stacked(entry.id);
        }
        if matches!(self.stack.selected, PaneStackSelection::Stacked(id) if self
            .stack
            .entries
            .iter()
            .any(|entry| entry.id == id))
        {
            return self.stack.selected;
        }
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.focus_handle(cx).is_focused(window))
        {
            PaneStackSelection::Base
        } else {
            self.stack.selected
        }
    }

    pub(crate) fn all_terminals(&self) -> impl Iterator<Item = &Entity<Terminal>> {
        self.terminal.iter().chain(
            self.stack
                .entries
                .iter()
                .filter_map(|entry| entry.terminal.as_ref()),
        )
    }

    pub(crate) fn all_views(&self) -> impl Iterator<Item = &Entity<TerminalView>> {
        self.view.iter().chain(
            self.stack
                .entries
                .iter()
                .filter_map(|entry| entry.view.as_ref()),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitPosition {
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaneRotationDirection {
    Clockwise,
    CounterClockwise,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PaneRegion {
    pub(crate) id: u64,
    pub(crate) left: f32,
    pub(crate) right: f32,
    pub(crate) top: f32,
    pub(crate) bottom: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneResizeBoundary {
    /// The fraction of the complete pane layout occupied by the split along
    /// the axis being resized.
    pub(crate) parent_fraction: f32,
    /// Whether the pane being resized is in the split's first child. Arrow
    /// keys move a screen-directional edge, so the second child uses the
    /// inverse size delta.
    pub(crate) active_is_first: bool,
    pub(crate) sibling_panes: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneLayout {
    Pane(u64),
    Split {
        axis: SplitAxis,
        /// The first child's share of the available extent on `axis`, scaled
        /// by [`PANE_SPLIT_RATIO_SCALE`]. An integer keeps layouts comparable
        /// and avoids accumulating floating point drift while resizing.
        first_ratio: u16,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

pub(crate) fn background_pane_layout(layout: &PaneLayout) -> BackgroundPaneLayout {
    match layout {
        PaneLayout::Pane(pane_id) => BackgroundPaneLayout::Pane { pane_id: *pane_id },
        PaneLayout::Split {
            axis,
            first,
            second,
            ..
        } => BackgroundPaneLayout::Split {
            axis: match axis {
                SplitAxis::Horizontal => "horizontal",
                SplitAxis::Vertical => "vertical",
            }
            .to_owned(),
            first: Box::new(background_pane_layout(first)),
            second: Box::new(background_pane_layout(second)),
        },
    }
}

impl PaneLayout {
    pub(crate) fn rotate_pane(
        &mut self,
        active_pane: u64,
        direction: PaneRotationDirection,
    ) -> bool {
        let Some(active_area) = self.pane_area(active_pane, 1.) else {
            return false;
        };
        let Some(path) = self.rotation_target(active_pane, active_area, 1.) else {
            return false;
        };
        self.rotate_at_path(&path, direction);
        true
    }

    fn pane_area(&self, pane_id: u64, area: f64) -> Option<f64> {
        match self {
            Self::Pane(id) => (*id == pane_id).then_some(area),
            Self::Split {
                first_ratio,
                first,
                second,
                ..
            } => {
                let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
                first
                    .pane_area(pane_id, first_area)
                    .or_else(|| second.pane_area(pane_id, area - first_area))
            }
        }
    }

    fn rotation_target(&self, active_pane: u64, active_area: f64, area: f64) -> Option<Vec<bool>> {
        if self.has_four_equal_panes(area) || self.is_two_pane_split() {
            return Some(Vec::new());
        }

        let Self::Split {
            first_ratio,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };

        let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
        let (child, child_area, sibling_area, child_is_first) = if first.contains_pane(active_pane)
        {
            (first, first_area, area - first_area, true)
        } else if second.contains_pane(active_pane) {
            (second, area - first_area, first_area, false)
        } else {
            return None;
        };

        if let Some(mut path) = child.rotation_target(active_pane, active_area, child_area) {
            path.insert(0, child_is_first);
            return Some(path);
        }

        // A pane can rotate with the complete subtree on the other side only
        // when it is at least as large as that subtree. This is what makes a
        // focused large pane dominate a three-pane layout while allowing a
        // focused small pane to rotate its local equal split instead.
        (active_area + PANE_ROTATION_AREA_EPSILON >= sibling_area).then_some(Vec::new())
    }

    fn is_two_pane_split(&self) -> bool {
        matches!(
            self,
            Self::Split {
                first,
                second,
                ..
            } if matches!(first.as_ref(), Self::Pane(_)) && matches!(second.as_ref(), Self::Pane(_))
        )
    }

    fn has_four_equal_panes(&self, area: f64) -> bool {
        let mut areas = Vec::with_capacity(4);
        self.collect_leaf_areas(area, &mut areas);
        areas.len() == 4
            && areas
                .iter()
                .all(|candidate| (*candidate - areas[0]).abs() <= PANE_ROTATION_AREA_EPSILON)
    }

    fn collect_leaf_areas(&self, area: f64, areas: &mut Vec<f64>) {
        match self {
            Self::Pane(_) => areas.push(area),
            Self::Split {
                first_ratio,
                first,
                second,
                ..
            } => {
                let first_area = area * f64::from(*first_ratio) / f64::from(PANE_SPLIT_RATIO_SCALE);
                first.collect_leaf_areas(first_area, areas);
                second.collect_leaf_areas(area - first_area, areas);
            }
        }
    }

    fn rotate_at_path(&mut self, path: &[bool], direction: PaneRotationDirection) {
        if let Some((first, second)) = path.split_first() {
            if let Self::Split {
                first: first_child,
                second: second_child,
                ..
            } = self
            {
                if *first {
                    first_child.rotate_at_path(second, direction);
                } else {
                    second_child.rotate_at_path(second, direction);
                }
            }
            return;
        }

        self.rotate_geometry(direction);
    }

    fn rotate_geometry(&mut self, direction: PaneRotationDirection) {
        let Self::Split {
            axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return;
        };

        first.rotate_geometry(direction);
        second.rotate_geometry(direction);

        let reverse_children = matches!(
            (direction, *axis),
            (PaneRotationDirection::Clockwise, SplitAxis::Horizontal)
                | (PaneRotationDirection::CounterClockwise, SplitAxis::Vertical)
        );
        *axis = match axis {
            SplitAxis::Horizontal => SplitAxis::Vertical,
            SplitAxis::Vertical => SplitAxis::Horizontal,
        };
        if reverse_children {
            std::mem::swap(first, second);
            *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
        }
    }

    fn remap_pane_ids(&mut self, pane_ids: &HashMap<u64, u64>) {
        match self {
            Self::Pane(pane_id) => *pane_id = pane_ids[pane_id],
            Self::Split { first, second, .. } => {
                first.remap_pane_ids(pane_ids);
                second.remap_pane_ids(pane_ids);
            }
        }
    }

    pub(crate) fn tiled(pane_ids: &[u64]) -> Option<Self> {
        fn build(pane_ids: &[u64], axis: SplitAxis) -> PaneLayout {
            if let [pane_id] = pane_ids {
                return PaneLayout::Pane(*pane_id);
            }
            let midpoint = if pane_ids.len() == 3 {
                1
            } else {
                pane_ids.len().div_ceil(2)
            };
            let next_axis = match axis {
                SplitAxis::Horizontal => SplitAxis::Vertical,
                SplitAxis::Vertical => SplitAxis::Horizontal,
            };
            PaneLayout::Split {
                axis,
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(build(&pane_ids[..midpoint], next_axis)),
                second: Box::new(build(&pane_ids[midpoint..], next_axis)),
            }
        }

        (!pane_ids.is_empty()).then(|| build(pane_ids, SplitAxis::Vertical))
    }

    pub(crate) fn split(
        &mut self,
        target: u64,
        axis: SplitAxis,
        new_pane: u64,
        position: SplitPosition,
    ) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                let (first, second) = match position {
                    SplitPosition::Before => (new_pane, target),
                    SplitPosition::After => (target, new_pane),
                };
                *self = Self::Split {
                    axis,
                    first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                    first: Box::new(Self::Pane(first)),
                    second: Box::new(Self::Pane(second)),
                };
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_pane, position)
                    || second.split(target, axis, new_pane, position)
            }
        }
    }

    pub(crate) fn replace(&mut self, target: u64, replacement: PaneLayout) -> bool {
        let mut replacement = Some(replacement);
        self.replace_inner(target, &mut replacement)
    }

    pub(crate) fn replace_inner(
        &mut self,
        target: u64,
        replacement: &mut Option<PaneLayout>,
    ) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = replacement
                    .take()
                    .expect("a pane layout replacement must only be consumed once");
                true
            }
            Self::Pane(_) => false,
            Self::Split { first, second, .. } => {
                first.replace_inner(target, replacement)
                    || second.replace_inner(target, replacement)
            }
        }
    }

    pub(crate) fn from_template(
        template: &PaneSplitTemplate,
        pane_ids: &mut impl Iterator<Item = u64>,
    ) -> Self {
        match template {
            PaneSplitTemplate::Pane(_) => Self::Pane(
                pane_ids
                    .next()
                    .expect("pane template and allocated IDs must have equal lengths"),
            ),
            PaneSplitTemplate::Split {
                axis,
                first,
                second,
            } => Self::Split {
                axis: match axis {
                    PaneSplitAxis::Horizontal => SplitAxis::Horizontal,
                    PaneSplitAxis::Vertical => SplitAxis::Vertical,
                },
                first_ratio: DEFAULT_PANE_SPLIT_RATIO,
                first: Box::new(Self::from_template(first, pane_ids)),
                second: Box::new(Self::from_template(second, pane_ids)),
            },
        }
    }

    pub(crate) fn without(self, target: u64) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != target).then_some(Self::Pane(id)),
            Self::Split {
                axis,
                first_ratio,
                first,
                second,
            } => match (first.without(target), second.without(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(layout), None) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            },
        }
    }

    pub(crate) fn without_all(&self, targets: &HashSet<u64>) -> Option<Self> {
        match self {
            Self::Pane(id) => (!targets.contains(id)).then_some(Self::Pane(*id)),
            Self::Split {
                axis,
                first_ratio,
                first,
                second,
            } => match (first.without_all(targets), second.without_all(targets)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis: *axis,
                    first_ratio: *first_ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(layout), None) | (None, Some(layout)) => Some(layout),
                (None, None) => None,
            },
        }
    }

    pub(crate) fn first_pane(&self) -> u64 {
        match self {
            Self::Pane(id) => *id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    pub(crate) fn regions(&self) -> Vec<PaneRegion> {
        let mut regions = Vec::new();
        self.collect_regions(0., 0., 1., 1., &mut regions);
        regions
    }

    pub(crate) fn collect_regions(
        &self,
        left: f32,
        top: f32,
        width: f32,
        height: f32,
        regions: &mut Vec<PaneRegion>,
    ) {
        match self {
            Self::Pane(id) => regions.push(PaneRegion {
                id: *id,
                left,
                right: left + width,
                top,
                bottom: top + height,
            }),
            Self::Split {
                axis: SplitAxis::Horizontal,
                first_ratio,
                first,
                second,
            } => {
                let first_height = height * Self::ratio_fraction(*first_ratio);
                first.collect_regions(left, top, width, first_height, regions);
                second.collect_regions(
                    left,
                    top + first_height,
                    width,
                    height - first_height,
                    regions,
                );
            }
            Self::Split {
                axis: SplitAxis::Vertical,
                first_ratio,
                first,
                second,
            } => {
                let first_width = width * Self::ratio_fraction(*first_ratio);
                first.collect_regions(left, top, first_width, height, regions);
                second.collect_regions(
                    left + first_width,
                    top,
                    width - first_width,
                    height,
                    regions,
                );
            }
        }
    }

    pub(crate) fn ratio_fraction(first_ratio: u16) -> f32 {
        f32::from(first_ratio) / f32::from(PANE_SPLIT_RATIO_SCALE)
    }

    /// Finds the closest split on `axis` that can resize `pane_id` against
    /// its sibling subtree.
    pub(crate) fn resize_boundary(
        &self,
        pane_id: u64,
        axis: SplitAxis,
    ) -> Option<PaneResizeBoundary> {
        self.resize_boundary_inner(pane_id, axis, 1.)
    }

    fn resize_boundary_inner(
        &self,
        pane_id: u64,
        axis: SplitAxis,
        parent_fraction: f32,
    ) -> Option<PaneResizeBoundary> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        let first_fraction = Self::ratio_fraction(*first_ratio);
        if first.contains_pane(pane_id) {
            return first
                .resize_boundary_inner(pane_id, axis, parent_fraction * first_fraction)
                .or_else(|| {
                    (*split_axis == axis).then(|| PaneResizeBoundary {
                        parent_fraction,
                        active_is_first: true,
                        sibling_panes: second.pane_ids(),
                    })
                });
        }
        if second.contains_pane(pane_id) {
            return second
                .resize_boundary_inner(pane_id, axis, parent_fraction * (1. - first_fraction))
                .or_else(|| {
                    (*split_axis == axis).then(|| PaneResizeBoundary {
                        parent_fraction,
                        active_is_first: false,
                        sibling_panes: first.pane_ids(),
                    })
                });
        }
        None
    }

    /// Adjust the closest resize boundary for a pane. A positive delta grows
    /// the pane and a negative delta shrinks it. The delta is expressed as a
    /// fraction of that split's available size.
    pub(crate) fn adjust_resize_boundary(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> bool {
        self.adjust_resize_boundary_inner(pane_id, axis, delta)
            .unwrap_or(false)
    }

    /// Returns the panes on both sides of the split identified by its first
    /// pane in each child. The pair uniquely identifies a split in a layout.
    pub(crate) fn split_panes(
        &self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
    ) -> Option<(Vec<u64>, Vec<u64>)> {
        let Self::Split {
            axis: split_axis,
            first,
            second,
            ..
        } = self
        else {
            return None;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Some((first.pane_ids(), second.pane_ids()));
        }
        first
            .split_panes(first_pane, second_pane, axis)
            .or_else(|| second.split_panes(first_pane, second_pane, axis))
    }

    pub(crate) fn split_ratio(
        &self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
    ) -> Option<f32> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Some(Self::ratio_fraction(*first_ratio));
        }
        first
            .split_ratio(first_pane, second_pane, axis)
            .or_else(|| second.split_ratio(first_pane, second_pane, axis))
    }

    /// Adjusts one exact split, rather than the nearest matching split for a
    /// pane. Mouse gutters use this to avoid changing a nested parallel split.
    pub(crate) fn adjust_split_ratio(
        &mut self,
        first_pane: u64,
        second_pane: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> bool {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return false;
        };
        if *split_axis == axis
            && first.first_pane() == first_pane
            && second.first_pane() == second_pane
        {
            return Self::adjust_first_ratio(first_ratio, delta);
        }
        first.adjust_split_ratio(first_pane, second_pane, axis, delta)
            || second.adjust_split_ratio(first_pane, second_pane, axis, delta)
    }

    fn adjust_resize_boundary_inner(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        delta: f32,
    ) -> Option<bool> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if first.contains_pane(pane_id) {
            if let Some(result) = first.adjust_resize_boundary_inner(pane_id, axis, delta) {
                return Some(result);
            }
            if *split_axis == axis {
                return Some(Self::adjust_first_ratio(first_ratio, delta));
            }
        } else if second.contains_pane(pane_id) {
            if let Some(result) = second.adjust_resize_boundary_inner(pane_id, axis, delta) {
                return Some(result);
            }
            if *split_axis == axis {
                return Some(Self::adjust_first_ratio(first_ratio, -delta));
            }
        }
        None
    }

    fn adjust_first_ratio(first_ratio: &mut u16, delta: f32) -> bool {
        let delta = (delta * f32::from(PANE_SPLIT_RATIO_SCALE)).round() as i32;
        if delta == 0 {
            return false;
        }
        let ratio = i32::from(*first_ratio);
        let clamped = (ratio + delta).clamp(1, i32::from(PANE_SPLIT_RATIO_SCALE - 1));
        if clamped == ratio {
            return false;
        }
        *first_ratio = clamped as u16;
        true
    }

    fn contains_pane(&self, pane_id: u64) -> bool {
        match self {
            Self::Pane(id) => *id == pane_id,
            Self::Split { first, second, .. } => {
                first.contains_pane(pane_id) || second.contains_pane(pane_id)
            }
        }
    }

    fn pane_ids(&self) -> Vec<u64> {
        match self {
            Self::Pane(id) => vec![*id],
            Self::Split { first, second, .. } => first
                .pane_ids()
                .into_iter()
                .chain(second.pane_ids())
                .collect(),
        }
    }

    /// Moves `active_pane` one step in `direction`, swapping it (and, when
    /// nested, its whole subtree) with the sibling subtree that occupies the
    /// nearest ancestor split on the matching axis. Returns `false` if there
    /// is no such ancestor (the pane is already at that edge of the layout).
    pub(crate) fn move_pane(&mut self, active_pane: u64, direction: PaneDirection) -> bool {
        let (axis, toward_first) = match direction {
            PaneDirection::Left => (SplitAxis::Vertical, true),
            PaneDirection::Right => (SplitAxis::Vertical, false),
            PaneDirection::Up => (SplitAxis::Horizontal, true),
            PaneDirection::Down => (SplitAxis::Horizontal, false),
        };
        self.move_pane_inner(active_pane, axis, toward_first)
            .unwrap_or(false)
    }

    fn move_pane_inner(
        &mut self,
        pane_id: u64,
        axis: SplitAxis,
        toward_first: bool,
    ) -> Option<bool> {
        let Self::Split {
            axis: split_axis,
            first_ratio,
            first,
            second,
        } = self
        else {
            return None;
        };
        if first.contains_pane(pane_id) {
            if let Some(handled) = first.move_pane_inner(pane_id, axis, toward_first) {
                return Some(handled);
            }
            (*split_axis == axis && !toward_first).then(|| {
                std::mem::swap(first, second);
                *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
                true
            })
        } else if second.contains_pane(pane_id) {
            if let Some(handled) = second.move_pane_inner(pane_id, axis, toward_first) {
                return Some(handled);
            }
            (*split_axis == axis && toward_first).then(|| {
                std::mem::swap(first, second);
                *first_ratio = PANE_SPLIT_RATIO_SCALE - *first_ratio;
                true
            })
        } else {
            None
        }
    }

    /// Swaps the positions of two panes anywhere in the layout, regardless of
    /// their split ancestry. Mouse-driven pane move drops a pane onto an
    /// arbitrary target, unlike the directional keyboard move above, so it
    /// cannot rely on a shared axis-matching ancestor.
    pub(crate) fn swap_panes(&mut self, first: u64, second: u64) -> bool {
        if first == second || !self.contains_pane(first) || !self.contains_pane(second) {
            return false;
        }
        self.swap_panes_inner(first, second);
        true
    }

    fn swap_panes_inner(&mut self, first: u64, second: u64) {
        match self {
            Self::Pane(id) => {
                if *id == first {
                    *id = second;
                } else if *id == second {
                    *id = first;
                }
            }
            Self::Split {
                first: first_child,
                second: second_child,
                ..
            } => {
                first_child.swap_panes_inner(first, second);
                second_child.swap_panes_inner(first, second);
            }
        }
    }

    pub(crate) fn adjacent_pane(&self, active: u64, direction: PaneDirection) -> Option<u64> {
        let regions = self.regions();
        let source = regions.iter().find(|region| region.id == active)?;
        let source_x = (source.left + source.right) / 2.;
        let source_y = (source.top + source.bottom) / 2.;

        regions
            .iter()
            .filter(|candidate| candidate.id != active)
            .filter_map(|candidate| {
                let candidate_x = (candidate.left + candidate.right) / 2.;
                let candidate_y = (candidate.top + candidate.bottom) / 2.;
                let (primary, perpendicular) = match direction {
                    PaneDirection::Left if candidate_x < source_x => {
                        (source_x - candidate_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Right if candidate_x > source_x => {
                        (candidate_x - source_x, (source_y - candidate_y).abs())
                    }
                    PaneDirection::Up if candidate_y < source_y => {
                        (source_y - candidate_y, (source_x - candidate_x).abs())
                    }
                    PaneDirection::Down if candidate_y > source_y => {
                        (candidate_y - source_y, (source_x - candidate_x).abs())
                    }
                    _ => return None,
                };
                Some((primary + perpendicular * 2., candidate.id))
            })
            .min_by(|(left_score, _), (right_score, _)| left_score.total_cmp(right_score))
            .map(|(_, id)| id)
    }
}

pub(crate) enum TabClosePolicy {
    Close,
    Background {
        authentication: Option<SessionAuthentication>,
    },
}

impl TabClosePolicy {
    pub(crate) fn background_authentication(&self) -> Option<Option<SessionAuthentication>> {
        match self {
            Self::Close => None,
            Self::Background { authentication } => Some(authentication.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TabAttention {
    pub(crate) summary: String,
    pub(crate) body: Option<String>,
}

impl TabAttention {
    pub(crate) fn tooltip_text(&self) -> String {
        match &self.body {
            Some(body) if !body.is_empty() => format!("{}\n{}", self.summary, body),
            _ => self.summary.clone(),
        }
    }
}

pub(crate) struct Tab {
    pub(crate) id: u64,
    /// Stable within this Zetta process. Unlike `id`, this survives moving a
    /// tab between visible and background storage and any pane/tab ID remap.
    pub(crate) attention_id: u64,
    pub(crate) attention: Option<TabAttention>,
    pub(crate) panes: Vec<TerminalPane>,
    pub(crate) pane_indices: HashMap<u64, usize>,
    pub(crate) next_pane_label: usize,
    pub(crate) layout: PaneLayout,
    pub(crate) active_pane: u64,
    pub(crate) focus_history: Vec<u64>,
    pub(crate) maximized_pane: Option<u64>,
    pub(crate) minimized_panes: Vec<u64>,
    pub(crate) selected_minimized_pane: Option<u64>,
    pub(crate) broadcast_input: bool,
    pub(crate) silent_mode: bool,
    pub(crate) close_policy: TabClosePolicy,
    /// A title entered through the tab rename UI. This is the highest-priority
    /// title source and is intentionally separate from process/worktree state.
    pub(crate) custom_title: Option<String>,
    /// A worktree title supplied by `wt new`. CWD detection may provide a
    /// fresher title, but terminal-side title updates can never replace this
    /// seed; `wt done` explicitly clears it.
    pub(crate) worktree_seed_title: Option<String>,
    /// A lower-priority title supplied by process control. It remains
    /// available outside a worktree, but is masked by the active worktree
    /// title.
    pub(crate) process_title: Option<String>,
    pub(crate) icon: Option<IconName>,
    /// Session-only visual pinning. Pinned tabs stay in the leading tab-bar
    /// prefix and are independent from the keep-running close policy.
    pub(crate) pinned: bool,
    pub(crate) renaming_pane: Option<u64>,
    pub(crate) rename_buffer: Option<String>,
    pub(crate) rename_cursor: usize,
    pub(crate) rename_select_all: bool,
    pub(crate) editing_overlay_pane: Option<u64>,
    pub(crate) overlay_buffer: Option<String>,
    pub(crate) overlay_cursor: usize,
    pub(crate) overlay_select_all: bool,
    /// Live overlay-style selector, opened right after the overlay's text is
    /// entered from the command palette. Combines font size, colour, and
    /// opacity.
    pub(crate) overlay_style_picker: Option<OverlayStylePicker>,
}

impl Tab {
    pub(crate) fn reassign_ids(&mut self, tab_id: u64, next_pane_id: &mut u64) {
        self.id = tab_id;
        let pane_ids = self
            .panes
            .iter_mut()
            .map(|pane| {
                let old_id = pane.id;
                pane.id = *next_pane_id;
                *next_pane_id += 1;
                (old_id, pane.id)
            })
            .collect::<HashMap<_, _>>();
        if !pane_ids.is_empty() {
            for pane in &mut self.panes {
                for entry in &mut pane.stack.entries {
                    let old_id = entry.id;
                    entry.id = *next_pane_id;
                    *next_pane_id += 1;
                    if pane.stack.selected == PaneStackSelection::Stacked(old_id) {
                        pane.stack.selected = PaneStackSelection::Stacked(entry.id);
                    }
                }
                pane.stack.repair_selection();
            }
        }
        self.pane_indices = self
            .panes
            .iter()
            .enumerate()
            .map(|(index, pane)| (pane.id, index))
            .collect();
        self.layout.remap_pane_ids(&pane_ids);
        self.active_pane = pane_ids[&self.active_pane];
        self.focus_history = self
            .focus_history
            .iter()
            .filter_map(|pane_id| pane_ids.get(pane_id).copied())
            .collect();
        self.maximized_pane = self
            .maximized_pane
            .and_then(|pane_id| pane_ids.get(&pane_id).copied());
        self.minimized_panes = self
            .minimized_panes
            .iter()
            .filter_map(|pane_id| pane_ids.get(pane_id).copied())
            .collect();
        self.selected_minimized_pane = self
            .selected_minimized_pane
            .and_then(|pane_id| pane_ids.get(&pane_id).copied());
        self.renaming_pane = None;
        self.rename_buffer = None;
        self.editing_overlay_pane = None;
        self.overlay_buffer = None;
        self.overlay_style_picker = None;
    }

    pub(crate) fn displayed_pane_label(&self, id: u64) -> Option<String> {
        let pane = self.pane(id)?;
        if self.renaming_pane != Some(id) {
            return Some(pane.label());
        }
        let buffer = self.rename_buffer.as_ref()?;
        if self.rename_select_all {
            return Some(buffer.clone());
        }
        let cursor = self.rename_cursor.min(buffer.len());
        let (before, after) = buffer.split_at(cursor);
        Some(format!("{before}|{after}"))
    }

    /// The pane's overlay text: the committed `overlay_text` normally, or the
    /// in-progress edit buffer (with a `|` cursor marker) while it is being
    /// edited. `None` means no overlay should be shown for this pane.
    pub(crate) fn displayed_pane_overlay(&self, id: u64) -> Option<String> {
        let pane = self.pane(id)?;
        if self.editing_overlay_pane != Some(id) {
            return pane.overlay_text.clone();
        }
        let buffer = self.overlay_buffer.as_ref()?;
        if self.overlay_select_all {
            if buffer.is_empty() {
                return Some("|".to_owned());
            }
            return Some(buffer.clone());
        }
        let cursor = self.overlay_cursor.min(buffer.len());
        let (before, after) = buffer.split_at(cursor);
        Some(format!("{before}|{after}"))
    }

    pub(crate) fn pane(&self, id: u64) -> Option<&TerminalPane> {
        self.pane_indices
            .get(&id)
            .and_then(|index| self.panes.get(*index))
    }

    pub(crate) fn pane_mut(&mut self, id: u64) -> Option<&mut TerminalPane> {
        let index = *self.pane_indices.get(&id)?;
        self.panes.get_mut(index)
    }

    pub(crate) fn push_pane(&mut self, mut pane: TerminalPane) {
        if pane.label_number == 0 {
            pane.label_number = self.next_pane_label;
        }
        self.next_pane_label = self.next_pane_label.max(pane.label_number + 1);
        self.pane_indices.insert(pane.id, self.panes.len());
        self.panes.push(pane);
    }

    pub(crate) fn apply_generated_labels(
        &mut self,
        pane_labels: impl IntoIterator<Item = (u64, Option<String>)>,
    ) {
        for (pane_id, generated_label) in pane_labels {
            if let Some(pane) = self.pane_mut(pane_id) {
                pane.generated_label = generated_label;
            }
        }
    }

    pub(crate) fn remove_pane(&mut self, id: u64) -> Option<TerminalPane> {
        let index = self.pane_indices.remove(&id)?;
        let pane = self.panes.remove(index);
        for (index, pane) in self.panes.iter().enumerate().skip(index) {
            self.pane_indices.insert(pane.id, index);
        }
        Some(pane)
    }

    pub(crate) fn active_pane(&self) -> Option<&TerminalPane> {
        self.pane(self.active_pane)
    }

    pub(crate) fn active_profile(&self) -> Option<&Profile> {
        self.active_pane().map(|pane| &pane.profile)
    }

    pub(crate) fn activate_pane(&mut self, id: u64) {
        if self.pane(id).is_none() {
            return;
        }
        self.focus_history.retain(|pane_id| *pane_id != id);
        self.focus_history.push(id);
        self.active_pane = id;
    }

    pub(crate) fn activate_stack_entry(&mut self, pane_id: u64, entry: PaneStackSelection) {
        let Some(pane) = self.pane_mut(pane_id) else {
            return;
        };
        if !pane.stack.select(entry) {
            return;
        }
        self.activate_pane(pane_id);
    }

    pub(crate) fn active_view(&self) -> Option<Entity<TerminalView>> {
        self.active_pane().and_then(TerminalPane::selected_view)
    }

    pub(crate) fn active_terminal(&self) -> Option<Entity<Terminal>> {
        self.active_pane().and_then(TerminalPane::selected_terminal)
    }

    pub(crate) fn visible_layout(&self) -> Option<PaneLayout> {
        if let Some(pane_id) = self.maximized_pane {
            return self.pane(pane_id).map(|_| PaneLayout::Pane(pane_id));
        }

        if self.minimized_panes.is_empty() {
            return Some(self.layout.clone());
        }
        let minimized = self.minimized_panes.iter().copied().collect::<HashSet<_>>();
        self.layout.without_all(&minimized)
    }

    pub(crate) fn pane_is_visible(&self, pane_id: u64) -> bool {
        self.pane(pane_id).is_some()
            && self
                .maximized_pane
                .map_or(!self.minimized_panes.contains(&pane_id), |maximized| {
                    maximized == pane_id
                })
    }

    pub(crate) fn toggle_maximize(&mut self, pane_id: u64) -> bool {
        if self.pane(pane_id).is_none() {
            return false;
        }
        let pane_was_minimized = self.minimized_panes.contains(&pane_id);
        let visible_pane_count = self
            .panes
            .len()
            .saturating_sub(self.minimized_panes.len())
            .saturating_add(usize::from(pane_was_minimized));
        if self.maximized_pane != Some(pane_id) && visible_pane_count < 2 {
            return false;
        }
        self.minimized_panes.retain(|id| *id != pane_id);
        self.repair_minimized_selection();
        self.maximized_pane = (self.maximized_pane != Some(pane_id)).then_some(pane_id);
        self.activate_pane(pane_id);
        true
    }

    pub(crate) fn minimize(&mut self, pane_id: u64) -> bool {
        if self.pane(pane_id).is_none()
            || self.minimized_panes.contains(&pane_id)
            || self.panes.len().saturating_sub(self.minimized_panes.len()) <= 1
        {
            return false;
        }

        self.maximized_pane = None;
        self.minimized_panes.push(pane_id);
        self.selected_minimized_pane = Some(pane_id);
        let fallback = self
            .focus_history
            .iter()
            .rev()
            .copied()
            .find(|id| *id != pane_id && !self.minimized_panes.contains(id))
            .or_else(|| {
                self.layout
                    .regions()
                    .into_iter()
                    .map(|region| region.id)
                    .find(|id| !self.minimized_panes.contains(id))
            })
            .expect("minimizing is only allowed when another pane remains visible");
        self.activate_pane(fallback);
        true
    }

    pub(crate) fn restore_minimized(&mut self, pane_id: u64) -> bool {
        let Some(index) = self.minimized_panes.iter().position(|id| *id == pane_id) else {
            return false;
        };
        self.minimized_panes.remove(index);
        if self.selected_minimized_pane == Some(pane_id) {
            self.selected_minimized_pane = if self.minimized_panes.is_empty() {
                None
            } else {
                Some(self.minimized_panes[index % self.minimized_panes.len()])
            };
        } else {
            self.repair_minimized_selection();
        }
        self.maximized_pane = None;
        self.activate_pane(pane_id);
        true
    }

    pub(crate) fn restore_last_minimized(&mut self) -> bool {
        self.selected_minimized_pane
            .filter(|pane_id| self.minimized_panes.contains(pane_id))
            .or_else(|| self.minimized_panes.last().copied())
            .is_some_and(|pane_id| self.restore_minimized(pane_id))
    }

    pub(crate) fn select_previous_minimized(&mut self) -> bool {
        self.select_adjacent_minimized(false)
    }

    pub(crate) fn select_next_minimized(&mut self) -> bool {
        self.select_adjacent_minimized(true)
    }

    fn select_adjacent_minimized(&mut self, forward: bool) -> bool {
        if self.minimized_panes.is_empty() {
            self.selected_minimized_pane = None;
            return false;
        }
        let index = self
            .selected_minimized_pane
            .and_then(|pane_id| self.minimized_panes.iter().position(|id| *id == pane_id))
            .map(|index| {
                if forward {
                    (index + 1) % self.minimized_panes.len()
                } else {
                    index
                        .checked_sub(1)
                        .unwrap_or(self.minimized_panes.len() - 1)
                }
            })
            .unwrap_or_else(|| {
                if forward {
                    0
                } else {
                    self.minimized_panes.len() - 1
                }
            });
        self.selected_minimized_pane = Some(self.minimized_panes[index]);
        true
    }

    fn repair_minimized_selection(&mut self) {
        if !self
            .selected_minimized_pane
            .is_some_and(|pane_id| self.minimized_panes.contains(&pane_id))
        {
            self.selected_minimized_pane = self.minimized_panes.last().copied();
        }
    }

    pub(crate) fn restore_focus_after_close(&mut self, closed: u64, fallback: u64) {
        if self.renaming_pane == Some(closed) {
            self.renaming_pane = None;
            self.rename_buffer = None;
            self.rename_select_all = false;
        }
        if self.maximized_pane == Some(closed) {
            self.maximized_pane = None;
        }
        self.minimized_panes.retain(|pane_id| *pane_id != closed);
        let surviving = self.panes.iter().map(|pane| pane.id).collect::<Vec<_>>();
        if !surviving.is_empty() && surviving.len() == self.minimized_panes.len() {
            let restored = self
                .minimized_panes
                .pop()
                .expect("a surviving minimized pane must be available");
            self.activate_pane(restored);
        }
        self.repair_minimized_selection();
        if self.panes.len() == 1 {
            self.maximized_pane = None;
        }
        self.focus_history
            .retain(|pane_id| *pane_id != closed && surviving.contains(pane_id));

        if self.active_pane != closed
            && surviving.contains(&self.active_pane)
            && !self.minimized_panes.contains(&self.active_pane)
        {
            return;
        }
        let next = self
            .focus_history
            .iter()
            .rev()
            .copied()
            .find(|pane_id| !self.minimized_panes.contains(pane_id))
            .or_else(|| self.visible_layout().map(|layout| layout.first_pane()))
            .or(self.selected_minimized_pane)
            .or_else(|| surviving.first().copied())
            .unwrap_or(fallback);
        self.activate_pane(next);
    }

    /// Chrome shared across a tab's panes (tab bar, pane borders, error
    /// banners) always follows the active pane's *configured* profile theme,
    /// never a pane's non-persistent theme override — an ephemeral,
    /// single-pane preview should not repaint the rest of the tab.
    pub(crate) fn theme(&self, cx: &App) -> Arc<Theme> {
        self.active_profile()
            .and_then(|profile| resolve_profile_theme(profile, cx).ok().flatten())
            .unwrap_or_else(|| cx.theme().clone())
    }
}

#[cfg(test)]
#[path = "tests/pane.rs"]
mod tests;
