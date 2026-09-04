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
        tab.overlay_buffer = Some(TextField::selected(text));
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
        let current_color = pane
            .overlay_color
            .unwrap_or(self.window_theme(cx).colors().text);
        let mut picker = OverlayStylePicker {
            pane_id,
            section: OverlayPickerSection::FontSize,
            font_size: pane.overlay_font_size.unwrap_or(OverlayFontSize::DEFAULT),
            original_font_size: pane.overlay_font_size,
            hue: 0.,
            saturation: 0.,
            value: 1.,
            original_color: pane.overlay_color,
            preset_index: OverlayStylePicker::preset_index_for_color(current_color),
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

    /// Selects the colour preset at `index` and previews it on the affected
    /// pane; does not commit the picker.
    pub(crate) fn set_overlay_color_preset_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(index) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
            .map(|picker| {
                picker.set_preset_index(index);
                picker.preset_index
            })
        else {
            return;
        };
        let Some(preset) = OVERLAY_COLOR_PRESETS.get(index).copied() else {
            return;
        };
        self.set_overlay_color_preset(preset, cx);
    }

    /// Moves the keyboard-focused colour preset within the six-column grid
    /// and previews the newly focused preset.
    pub(crate) fn adjust_overlay_color_preset(
        &mut self,
        row_delta: isize,
        column_delta: isize,
        cx: &mut Context<Self>,
    ) {
        let Some((index, changed)) = self
            .tabs
            .get_mut(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_mut())
            .map(|picker| {
                let changed = picker.move_preset_cursor(row_delta, column_delta);
                (picker.preset_index, changed)
            })
        else {
            return;
        };
        if !changed {
            return;
        }
        self.set_overlay_color_preset_index(index, cx);
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
        }
        if has_text {
            self.begin_overlay_style_picker(pane_id, window, cx);
        } else {
            self.focus_active(window, cx);
        }
    }
}

impl Zetta {
    /// Floating panel that chooses a pane overlay's font size, colour, and
    /// opacity right after its text is entered from the command palette. The
    /// pane under the panel previews each highlighted value; Enter commits
    /// them all and Escape restores the pane's previous values.
    pub(crate) fn render_overlay_style_picker_overlay(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let picker = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| tab.overlay_style_picker.as_ref())?;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();
        let ctx = OverlayPickerContext {
            picker,
            colors: &colors,
            handle: &handle,
        };
        let cancel_handle = handle.clone();

        Some(
            div()
                .id("overlay-style-backdrop")
                .absolute()
                .inset_0()
                .pt(px(72.))
                .px_4()
                .flex()
                .items_start()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cancel_handle
                        .update(cx, |this, cx| {
                            this.cancel_overlay_style_picker(window, cx);
                        })
                        .ok();
                })
                .child(
                    div()
                        .id("overlay-style-picker")
                        .track_focus(&self.overlay_style_focus)
                        .w_full()
                        .max_w(px(440.))
                        .overflow_hidden()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(overlay_picker_header(ctx))
                        .child(
                            div()
                                .px_4()
                                .py_4()
                                .flex()
                                .flex_col()
                                .gap_3()
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_3()
                                        .child(overlay_font_size_section(ctx))
                                        .child(overlay_opacity_section(ctx)),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_3()
                                        .child(overlay_colour_section(ctx))
                                        .child(overlay_colour_preset_section(ctx)),
                                ),
                        )
                        .child(overlay_picker_hint_bar(ctx))
                        .child(overlay_picker_buttons(ctx)),
                )
                .into_any_element(),
        )
    }
}

/// What every part of the overlay style picker needs: the state it shows, the
/// theme it draws in, and the handle its controls call back through.
///
/// The three travel together through the header, the four sections and the
/// footer, and none of them changes between those, so they travel as one `Copy`
/// bundle rather than as three parameters repeated per builder.
#[derive(Clone, Copy)]
struct OverlayPickerContext<'a> {
    picker: &'a OverlayStylePicker,
    colors: &'a ThemeColors,
    handle: &'a WeakEntity<Zetta>,
}

impl OverlayPickerContext<'_> {
    /// The frame each of the four sections draws: a bordered box that shows
    /// whether it holds the keyboard, and takes it when clicked.
    fn section_box(&self, section: OverlayPickerSection) -> gpui::Div {
        let active = self.picker.section == section;
        let section_handle = self.handle.clone();
        div()
            .px_3()
            .py_3()
            .rounded(px(6.))
            .border_1()
            .cursor_pointer()
            .border_color(if active {
                self.colors.border_focused
            } else {
                self.colors.border
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                section_handle
                    .update(cx, |this, cx| {
                        this.set_overlay_picker_section(section, cx);
                    })
                    .ok();
            })
            .flex_1()
            .min_w_0()
            .flex_col()
            .gap_2()
    }

    /// A section's heading.
    fn section_title(&self, title: &'static str) -> gpui::Div {
        div().text_color(self.colors.text).text_sm().child(title)
    }

    /// Whether `section` is the one the keyboard is on.
    fn is_active(&self, section: OverlayPickerSection) -> bool {
        self.picker.section == section
    }
}

/// The picker's title row, with the swatch and the one-line summary of what
/// applying would set.
fn overlay_picker_header(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let OverlayPickerContext { picker, colors, .. } = ctx;
    h_flex()
        .h_11()
        .px_3()
        .items_center()
        .justify_between()
        .border_b_1()
        .border_color(colors.border)
        .child(
            div()
                .text_color(colors.text)
                .text_sm()
                .child("Overlay style"),
        )
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w_3()
                        .h_3()
                        .rounded_sm()
                        .border_1()
                        .border_color(colors.border)
                        .bg(picker.color()),
                )
                .child(
                    div()
                        .text_color(colors.text_accent)
                        .text_sm()
                        .child(format!(
                            "{} · {} · {}%",
                            picker.font_size.cli_name(),
                            picker.hex_buffer,
                            picker.opacity_percent
                        )),
                ),
        )
}

/// The font-size row.
fn overlay_font_size_section(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let size_options = overlay_size_options(ctx.handle, ctx.colors, ctx.picker);
    ctx.section_box(OverlayPickerSection::FontSize)
        .child(ctx.section_title("Font size"))
        .child(h_flex().gap_1().children(size_options))
}

/// The opacity slider: its track, the filled part, the knob, and the invisible
/// stops that make each 5% step clickable.
fn overlay_opacity_section(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let OverlayPickerContext { picker, colors, .. } = ctx;
    let opacity_fraction = picker.opacity_percent as f32 / 100.;
    let opacity_stops = overlay_opacity_stops(ctx.handle);
    ctx.section_box(OverlayPickerSection::Opacity)
        .child(ctx.section_title("Opacity"))
        .child(
            div()
                .relative()
                .h_5()
                .min_w_0()
                .flex()
                .items_center()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .h_1()
                        .rounded_full()
                        .bg(colors.element_background),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .w(gpui::relative(opacity_fraction))
                        .h_1()
                        .rounded_full()
                        .bg(colors.text_accent),
                )
                .child(
                    div()
                        .absolute()
                        .left(gpui::relative(opacity_fraction))
                        .ml(px(-5.))
                        .size(px(10.))
                        .rounded_full()
                        .border_1()
                        .border_color(colors.border_focused)
                        .bg(colors.text_accent),
                )
                .child(h_flex().absolute().inset_0().children(opacity_stops)),
        )
}

/// The colour section: the hex field, the saturation/brightness square, and the
/// hue strip.
fn overlay_colour_section(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    ctx.section_box(OverlayPickerSection::Color)
        .child(overlay_hex_field(ctx))
        .child(overlay_saturation_value_field(ctx))
        .child(overlay_hue_field(ctx))
}

/// The swatch, the label, and the hex the keyboard types into.
///
/// The caret blinks only while the colour section holds the keyboard, because
/// that is the only time typing reaches the field.
fn overlay_hex_field(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let OverlayPickerContext { picker, colors, .. } = ctx;
    let focused = ctx.is_active(OverlayPickerSection::Color);
    let hex_field_handle = ctx.handle.clone();
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .w(px(30.))
                .h(px(18.))
                .rounded_sm()
                .border_1()
                .border_color(colors.border)
                .bg(picker.color()),
        )
        .child(div().text_color(colors.text).text_sm().child("Colour"))
        .child(
            div()
                .id("overlay-hex-field")
                .flex_1()
                .min_w_0()
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.element_background)
                .cursor_text()
                .on_click(move |_, _, cx| {
                    hex_field_handle
                        .update(cx, |this, cx| {
                            this.set_overlay_picker_section(OverlayPickerSection::Color, cx);
                        })
                        .ok();
                })
                .child(
                    h_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_color(colors.text)
                                .text_sm()
                                .child(picker.hex_buffer.clone()),
                        )
                        .when(focused, |field| {
                            field.child(div().w(px(1.5)).h(px(13.)).bg(colors.text).with_animation(
                                "overlay-hex-caret",
                                Animation::new(Duration::from_millis(500)).repeat(),
                                |caret, progress| {
                                    let visible = (progress * 2.).fract() < 0.5;
                                    caret.opacity(if visible { 1. } else { 0. })
                                },
                            ))
                        }),
                ),
        )
}

/// The saturation/brightness square, with the knob at the selected point.
fn overlay_saturation_value_field(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let OverlayPickerContext { picker, colors, .. } = ctx;
    let sv_rows = overlay_color_grid(ctx.handle, picker.hue);
    div()
        .relative()
        .h(px(152.))
        .w_full()
        .flex()
        .flex_col()
        .min_h_0()
        .rounded_sm()
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .child(v_flex().flex_1().min_h_0().children(sv_rows))
        .child(
            div()
                .absolute()
                .left(gpui::relative(picker.saturation))
                .top(gpui::relative(1. - picker.value))
                .ml(px(-6.))
                .mt(px(-6.))
                .size(px(12.))
                .rounded_full()
                .border_1()
                .border_color(colors.element_selection_background)
                .bg(colors.text_accent),
        )
}

/// The hue strip, with the knob at the selected hue.
fn overlay_hue_field(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let OverlayPickerContext { picker, colors, .. } = ctx;
    let hue_segments = overlay_hue_strip(ctx.handle, picker.saturation, picker.value);
    div()
        .relative()
        .h(px(18.))
        .w_full()
        .flex()
        .rounded_sm()
        .border_1()
        .border_color(colors.border)
        .overflow_hidden()
        .child(h_flex().flex_1().children(hue_segments))
        .child(
            div()
                .absolute()
                .top(px(2.))
                .left(gpui::relative(picker.hue))
                .ml(px(-6.))
                .size(px(12.))
                .rounded_full()
                .border_1()
                .border_color(colors.element_selection_background)
                .bg(colors.text_accent),
        )
}

/// The fixed named colours, as a grid.
fn overlay_colour_preset_section(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let picker = ctx.picker;
    let selected_preset_index = OVERLAY_COLOR_PRESETS
        .iter()
        .position(|preset| preset.hex.eq_ignore_ascii_case(&picker.hex_buffer));
    let focused_preset_index = picker
        .preset_index
        .min(OVERLAY_COLOR_PRESETS.len().saturating_sub(1));
    let preset_rows = overlay_color_presets(
        ctx.handle,
        ctx.colors,
        picker.section,
        selected_preset_index,
        focused_preset_index,
    );
    ctx.section_box(OverlayPickerSection::ColorPresets)
        .child(ctx.section_title("Colour presets"))
        .child(v_flex().gap_1().children(preset_rows))
}

/// The line naming the keys the focused section answers to.
fn overlay_picker_hint_bar(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let colors = ctx.colors;
    div()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(colors.border)
        .child(
            div()
                .text_color(colors.text_muted)
                .text_xs()
                .child(overlay_picker_hint(ctx.picker.section)),
        )
}

/// Cancel and Apply.
fn overlay_picker_buttons(ctx: OverlayPickerContext<'_>) -> gpui::Div {
    let colors = ctx.colors;
    let cancel_button_handle = ctx.handle.clone();
    let apply_handle = ctx.handle.clone();
    h_flex()
        .px_3()
        .py_3()
        .gap_2()
        .justify_end()
        .border_t_1()
        .border_color(colors.border)
        .child(
            Button::new("cancel-overlay-style", "Cancel")
                .style(ButtonStyle::Outlined)
                .color(Color::Custom(colors.text))
                .on_click(move |_, window, cx| {
                    cancel_button_handle
                        .update(cx, |this, cx| {
                            this.cancel_overlay_style_picker(window, cx);
                        })
                        .ok();
                }),
        )
        .child(
            Button::new("apply-overlay-style", "Apply")
                .style(ButtonStyle::Filled)
                .color(Color::Custom(colors.text))
                .on_click(move |_, window, cx| {
                    apply_handle
                        .update(cx, |this, cx| {
                            this.apply_overlay_style_picker(window, cx);
                        })
                        .ok();
                }),
        )
}

/// The font-size row: one option per [`OverlayFontSize`], the current one
/// selected.
fn overlay_size_options(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    picker: &OverlayStylePicker,
) -> Vec<gpui::Stateful<gpui::Div>> {
    OverlayFontSize::ALL
        .iter()
        .enumerate()
        .map(|(index, size)| {
            let size = *size;
            let selected = picker.font_size == size;
            let option_handle = handle.clone();
            div()
                .id(("overlay-size-option", index))
                .flex_1()
                .py_1()
                .rounded(px(4.))
                .cursor_pointer()
                .when(selected, |option| option.bg(colors.element_selected))
                .hover(|option| option.bg(colors.element_hover))
                .text_center()
                .text_color(if selected {
                    colors.text
                } else {
                    colors.text_muted
                })
                .text_sm()
                .on_click(move |_, _, cx| {
                    option_handle
                        .update(cx, |this, cx| {
                            this.set_overlay_font_size(size, cx);
                        })
                        .ok();
                })
                .child(size.cli_name())
        })
        .collect::<Vec<_>>()
}

/// The saturation/value square for the picker's current hue, as ten rows of
/// twelve cells.
fn overlay_color_grid(handle: &WeakEntity<Zetta>, hue: f32) -> Vec<gpui::Div> {
    (0usize..10)
        .map(|row| {
            let row_value = 1. - row as f32 / 9.;
            let row_handle = handle.clone();
            h_flex()
                .flex_1()
                .min_h_0()
                .children((0..12).map(move |column| {
                    let column_saturation = column as f32 / 11.;
                    let cell_color = hsv_to_hsla(hue, column_saturation, row_value);
                    let cell_handle = row_handle.clone();
                    div()
                        .id(("overlay-color-cell", row * 12 + column))
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .h_full()
                        .cursor_pointer()
                        .bg(cell_color)
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cell_handle
                                .update(cx, |this, cx| {
                                    this.set_overlay_color_hsv(
                                        hue,
                                        column_saturation,
                                        row_value,
                                        cx,
                                    );
                                })
                                .ok();
                        })
                }))
        })
        .collect::<Vec<_>>()
}

/// The named colour presets, in rows of `OVERLAY_COLOR_PRESET_COLUMNS`.
///
/// Selected and focused are different things here: a preset is selected when
/// the picker's colour matches it, and focused when the keyboard is on it,
/// which only applies on Enter.
fn overlay_color_presets(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    section: OverlayPickerSection,
    selected_preset_index: Option<usize>,
    focused_preset_index: usize,
) -> Vec<gpui::Div> {
    OVERLAY_COLOR_PRESETS
        .chunks(OVERLAY_COLOR_PRESET_COLUMNS)
        .enumerate()
        .map(|(row_index, presets)| {
            let row_handle = handle.clone();
            h_flex()
                .w_full()
                .gap_1()
                .children(
                    presets
                        .iter()
                        .enumerate()
                        .map(move |(column_index, preset)| {
                            let preset = *preset;
                            let preset_index =
                                row_index * OVERLAY_COLOR_PRESET_COLUMNS + column_index;
                            let selected = selected_preset_index == Some(preset_index);
                            let keyboard_focused = section == OverlayPickerSection::ColorPresets
                                && focused_preset_index == preset_index;
                            let preset_handle = row_handle.clone();
                            div()
                                .id(("overlay-color-preset", preset_index))
                                .h(px(26.))
                                .flex_1()
                                .min_w_0()
                                .px_1()
                                .flex()
                                .items_center()
                                .gap_1()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(if keyboard_focused {
                                    colors.border_focused
                                } else {
                                    colors.border.opacity(0.)
                                })
                                .cursor_pointer()
                                .when(selected, |swatch| swatch.bg(colors.element_selected))
                                .hover(|swatch| swatch.bg(colors.element_hover))
                                .child(
                                    div()
                                        .flex_none()
                                        .size(px(12.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(colors.border)
                                        .bg(preset.color()),
                                )
                                .child(
                                    Label::new(preset.name)
                                        .size(LabelSize::XSmall)
                                        .color(Color::Custom(colors.text))
                                        .truncate(),
                                )
                                .tooltip(Tooltip::text(preset.name))
                                .on_click(move |_, _, cx| {
                                    preset_handle
                                        .update(cx, |this, cx| {
                                            this.set_overlay_color_preset(preset, cx);
                                        })
                                        .ok();
                                })
                        }),
                )
        })
        .collect::<Vec<_>>()
}

/// The hue strip beneath the colour square.
fn overlay_hue_strip(
    handle: &WeakEntity<Zetta>,
    saturation: f32,
    value: f32,
) -> Vec<gpui::Stateful<gpui::Div>> {
    (0usize..12)
        .map(|column| {
            let column_hue = column as f32 / 11.;
            let segment_color = hsv_to_hsla(column_hue, 1., 1.);
            let segment_handle = handle.clone();
            div()
                .id(("overlay-hue-segment", column))
                .flex_1()
                .min_w_0()
                .h_full()
                .cursor_pointer()
                .bg(segment_color)
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    segment_handle
                        .update(cx, |this, cx| {
                            this.set_overlay_color_hsv(column_hue, saturation, value, cx);
                        })
                        .ok();
                })
        })
        .collect::<Vec<_>>()
}

/// The click targets along the opacity slider, one per five percent.
fn overlay_opacity_stops(handle: &WeakEntity<Zetta>) -> Vec<gpui::Stateful<gpui::Div>> {
    (0usize..=20)
        .map(|step| {
            let step_value = step * 5;
            let step_handle = handle.clone();
            div()
                .id(("overlay-opacity-stop", step))
                .h_full()
                .flex_1()
                .cursor_pointer()
                .on_click(move |_, _, cx| {
                    step_handle
                        .update(cx, |this, cx| {
                            this.set_overlay_opacity_percent(step_value, cx);
                        })
                        .ok();
                })
        })
        .collect::<Vec<_>>()
}

/// The key hints for the section the picker is on.
fn overlay_picker_hint(section: OverlayPickerSection) -> &'static str {
    match section {
        OverlayPickerSection::FontSize => {
            "← → size · Home/End ends · Tab switch · Enter apply · Esc cancel"
        }
        OverlayPickerSection::Color => {
            "← → saturation · ↑↓ brightness · ⇧←→ hue · type hex · Tab switch · Enter apply · Esc cancel"
        }
        OverlayPickerSection::ColorPresets => {
            "← →/↑↓ move · Home/End ends · Tab switch · Enter apply · Esc cancel"
        }
        OverlayPickerSection::Opacity => {
            "← → opacity · Home/End ends · Tab switch · Enter apply · Esc cancel"
        }
    }
}

#[cfg(test)]
#[path = "tests/pane_overlay.rs"]
mod tests;
