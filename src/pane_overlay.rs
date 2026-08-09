use super::*;

impl Zetta {
    pub(crate) fn set_pane_overlay(
        &mut self,
        _: &SetPaneOverlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane_id) = self.tabs.get(self.active_tab).map(|tab| tab.active_pane) else {
            return;
        };
        self.begin_pane_overlay_edit(pane_id, window, cx);
    }

    pub(crate) fn reset_pane_overlay(
        &mut self,
        _: &ResetPaneOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_active_pane_overlay(None, None, None, None, cx);
    }

    pub(crate) fn begin_pane_overlay_edit(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(pane) = tab.pane(pane_id) else {
            return;
        };
        let text = pane.overlay_text.clone().unwrap_or_default();
        tab.activate_pane(pane_id);
        tab.editing_overlay_pane = Some(pane_id);
        tab.overlay_cursor = text.len();
        tab.overlay_buffer = Some(text);
        tab.overlay_select_all = true;
        self.rename_focus.focus(window, cx);
        cx.notify();
    }

    /// Sets the active pane's overlay text and style directly, bypassing the
    /// inline edit buffer. Shared by the `overlay` CLI command and its
    /// process-control handler; never touches `config.json`, so the overlay
    /// is lost when the pane closes or the configuration reloads. `text:
    /// None` clears the overlay along with its style. Every call fully
    /// replaces the previous text and style rather than merging with it.
    pub(crate) fn set_active_pane_overlay(
        &mut self,
        text: Option<String>,
        font_size: Option<OverlayFontSize>,
        opacity: Option<f32>,
        color: Option<gpui::Hsla>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return false;
        };
        let active_pane = tab.active_pane;
        let Some(pane) = tab.pane_mut(active_pane) else {
            return false;
        };
        pane.overlay_text = text;
        pane.overlay_font_size = font_size;
        pane.overlay_opacity = opacity;
        pane.overlay_color = color;
        cx.notify();
        true
    }

    /// Whether the overlay-style selector is open for the active tab.
    pub(crate) fn is_picking_overlay_style(&self) -> bool {
        self.tabs
            .get(self.active_tab)
            .is_some_and(|tab| tab.overlay_style_picker.is_some())
    }

    /// Opens the overlay-style selector for `pane_id`, seeded with the pane's
    /// current font size, colour, and opacity (the colour falling back to the
    /// theme's text colour) and focused for keyboard adjustment. Everything
    /// previews changes live on the pane; nothing is committed until
    /// [`Self::apply_overlay_style_picker`] runs.
    pub(crate) fn begin_overlay_style_picker(
        &mut self,
        pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let Some(pane) = tab.pane(pane_id) else {
            return;
        };
        let current_color = pane.overlay_color.unwrap_or(cx.theme().colors().text);
        let mut picker = OverlayStylePicker {
            pane_id,
            section: OverlayPickerSection::FontSize,
            font_size: pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT),
            original_font_size: pane.overlay_font_size,
            hue: 0.,
            saturation: 0.,
            value: 1.,
            original_color: pane.overlay_color,
            opacity_percent: OverlayStylePicker::percent_for_opacity(pane.overlay_opacity),
            original_opacity: pane.overlay_opacity,
            hex_buffer: String::new(),
        };
        let (hue, saturation, value) = overlay_picker_hsv_from_hsla(current_color);
        picker.set_color(hue, saturation, value);
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            tab.overlay_style_picker = Some(picker);
        }
        self.overlay_style_focus.focus(window, cx);
        cx.notify();
    }

    /// Copies the picker's current font size, colour, and opacity to its
    /// pane, previewing the selection live without committing the picker.
    fn preview_overlay_style(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.as_mut() else {
            return;
        };
        let pane_id = picker.pane_id;
        let font_size = picker.font_size;
        let color = picker.color();
        let opacity = picker.opacity_percent as f32 / 100.;
        if let Some(pane) = tab.pane_mut(pane_id) {
            pane.overlay_font_size = Some(font_size);
            pane.overlay_color = Some(color);
            pane.overlay_opacity = Some(opacity);
        }
        cx.notify();
    }

    /// The section of the overlay-style selector the keyboard adjusts.
    pub(crate) fn set_overlay_picker_section(
        &mut self,
        section: OverlayPickerSection,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.section == section {
            return;
        }
        picker.section = section;
        cx.notify();
    }

    /// Cycles the keyboard-adjusted overlay-style section by `delta` steps.
    pub(crate) fn adjust_overlay_picker_section(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let next = picker.section.step(delta);
        if picker.section == next {
            return;
        }
        picker.section = next;
        cx.notify();
    }

    /// Selects the exact overlay font size and previews it on the affected
    /// pane; does not commit the picker.
    pub(crate) fn set_overlay_font_size(
        &mut self,
        font_size: OverlayFontSize,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.font_size == font_size {
            return;
        }
        picker.font_size = font_size;
        self.preview_overlay_style(cx);
    }

    /// Cycles the overlay font size by `delta` sizes.
    pub(crate) fn adjust_overlay_font_size(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(next) = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_ref())
            .map(|picker| picker.font_size.step(delta))
        else {
            return;
        };
        self.set_overlay_font_size(next, cx);
    }

    /// Selects the exact overlay colour in HSV space, normalized like the
    /// picker's hue bar and saturation/brightness square, and previews it on
    /// the affected pane; does not commit the picker.
    pub(crate) fn set_overlay_color_hsv(
        &mut self,
        hue: f32,
        saturation: f32,
        value: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let hue = hue.rem_euclid(1.);
        let saturation = saturation.clamp(0., 1.);
        let value = value.clamp(0., 1.);
        if (picker.hue - hue).abs() < f32::EPSILON
            && (picker.saturation - saturation).abs() < f32::EPSILON
            && (picker.value - value).abs() < f32::EPSILON
        {
            return;
        }
        picker.set_color(hue, saturation, value);
        self.preview_overlay_style(cx);
    }

    /// Selects a fixed named overlay colour and previews it on the affected
    /// pane; does not commit the picker.
    pub(crate) fn set_overlay_color_preset(
        &mut self,
        preset: OverlayColorPreset,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.set_color_preset(preset);
        self.preview_overlay_style(cx);
    }

    /// Rotates the overlay colour's hue by `delta` turns.
    pub(crate) fn adjust_overlay_hue(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_hue(delta);
        self.preview_overlay_style(cx);
    }

    /// Moves the overlay colour's saturation by `delta`.
    pub(crate) fn adjust_overlay_saturation(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_saturation(delta);
        self.preview_overlay_style(cx);
    }

    /// Moves the overlay colour's brightness by `delta`.
    pub(crate) fn adjust_overlay_value(&mut self, delta: f32, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        picker.adjust_value(delta);
        self.preview_overlay_style(cx);
    }

    /// Feeds one hex digit into the overlay colour's hex field; once a full
    /// `#rrggbb` colour is complete it is previewed on the pane.
    pub(crate) fn overlay_hex_input(&mut self, ch: char, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.hex_input(ch) {
            self.preview_overlay_style(cx);
        } else {
            cx.notify();
        }
    }

    /// Backspaces the overlay colour's hex field; a now-complete colour is
    /// applied to the pane.
    pub(crate) fn overlay_hex_backspace(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        if picker.hex_backspace() {
            self.preview_overlay_style(cx);
        } else {
            cx.notify();
        }
    }

    /// Highlights the exact `percent` in the overlay-opacity slider and
    /// previews it on the affected pane; does not commit the picker.
    pub(crate) fn set_overlay_opacity_percent(&mut self, percent: usize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let percent = percent.clamp(0, 100);
        if picker.opacity_percent == percent {
            return;
        }
        picker.opacity_percent = percent;
        self.preview_overlay_style(cx);
    }

    /// Nudges the highlighted overlay opacity by `delta` percentage points.
    pub(crate) fn adjust_overlay_opacity_percent(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(picker) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
        else {
            return;
        };
        let next = (picker.opacity_percent as isize + delta).clamp(0, 100) as usize;
        if next == picker.opacity_percent {
            return;
        }
        picker.opacity_percent = next;
        self.preview_overlay_style(cx);
    }

    /// Commits the picker's font size, colour, and opacity to the pane and
    /// closes the selector.
    pub(crate) fn apply_overlay_style_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.take() else {
            return;
        };
        if let Some(pane) = tab.pane_mut(picker.pane_id) {
            pane.overlay_font_size = Some(picker.font_size);
            pane.overlay_color = Some(picker.color());
            pane.overlay_opacity = Some(picker.opacity_percent as f32 / 100.);
        }
        self.focus_active(window, cx);
    }

    /// Closes the overlay-style selector and restores the pane's font size,
    /// colour, and opacity from before it opened.
    pub(crate) fn cancel_overlay_style_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(self.active_tab) else {
            return;
        };
        let Some(picker) = tab.overlay_style_picker.take() else {
            return;
        };
        if let Some(pane) = tab.pane_mut(picker.pane_id) {
            pane.overlay_font_size = picker.original_font_size;
            pane.overlay_color = picker.original_color;
            pane.overlay_opacity = picker.original_opacity;
        }
        self.focus_active(window, cx);
    }

    /// Applies the overlay's text and proceeds straight to the live style
    /// selector in the same palette-driven flow. Skips the picker when the
    /// entered text was empty (the overlay was cleared).
    pub(crate) fn commit_overlay_text_then_pick_style(
        &mut self,
        pane_id: u64,
        text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let has_text = text.is_some();
        if let Some(tab) = self.tabs.get_mut(self.active_tab) {
            if let Some(pane) = tab.pane_mut(pane_id) {
                pane.overlay_text = text;
            }
            tab.overlay_buffer = None;
            tab.overlay_select_all = false;
        }
        if has_text {
            self.begin_overlay_style_picker(pane_id, window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }
}
