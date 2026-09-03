//! The model behind a pane's overlay: its text, its font-size steps, and the
//! colour the style picker edits.
//!
//! The picker edits in HSV because that is what a saturation/value square and
//! a hue strip are, while everything else — configuration, the control socket,
//! the theme — speaks hex or HSLA, so the conversions in both directions live
//! here with the presets. Rendering is `pane_overlay.rs`.

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

#[cfg(test)]
#[path = "../tests/pane/overlay.rs"]
mod tests;
