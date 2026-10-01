//! The shared building blocks of the settings form.
//!
//! These used to be closures inside `render_settings_overlay`, which meant the
//! page and the modals could only be built in one place, in one pass. Holding
//! their captured state in a struct instead lets the page render inside its own
//! view (see `view_boundary`) while the modals keep rendering in the dialog, so
//! scrolling a modal or a dropdown popup no longer rebuilds the page behind it.
//!
//! Callers still take `&impl Fn(..)` parameters: `render_settings_overlay` wraps
//! these methods in closures, keeping the page and modal signatures unchanged.

use super::*;
use crate::settings_editor::{Placeholder, SettingKind};

pub(crate) struct SettingsFormWidgets {
    colors: ThemeColors,
    handle: WeakEntity<Zetta>,
    focused_input: Option<SettingsInput>,
    focused_control: Option<SettingsControl>,
    /// What `scroll_settings_control_into_view` last aimed at, and the offset it
    /// aimed from, so a row can finish that scroll from where it actually laid
    /// out. Snapshotted like the focus above, because this builder renders in its
    /// own view and cannot hold the editor.
    focus_scroll_request: Option<(SettingsControl, Pixels)>,
    settings_scroll: ScrollHandle,
    /// What the font-size field shows while it is empty.
    terminal_font_size_default: f32,
}

impl SettingsFormWidgets {
    pub(crate) fn new(
        editor: &SettingsEditor,
        colors: ThemeColors,
        handle: WeakEntity<Zetta>,
    ) -> Self {
        Self {
            colors,
            handle,
            focused_input: editor.focused_input,
            focused_control: editor.focused_control.clone(),
            focus_scroll_request: editor.focus_scroll_request.clone(),
            settings_scroll: editor.settings_scroll.clone(),
            terminal_font_size_default: editor.terminal_font_size_default,
        }
    }

    /// The scrollbar beside one of the dialog's scroll regions.
    ///
    /// Drawn at paint time from the region's live geometry rather than from
    /// what it measured the frame before, and not drawn at all while there is
    /// nothing to scroll: a full-height thumb beside content that fits — the
    /// Add profile modal's, say — read as a broken control. Painting late is
    /// also what keeps it right on the frame content starts or stops
    /// overflowing, such as when an argument is added.
    pub(crate) fn scroll_indicator(&self, id: String, scroll: &ScrollHandle) -> gpui::AnyElement {
        let paint_scroll = scroll.clone();
        let track_color = self.colors.scrollbar_track_background;
        let thumb_color = self.colors.scrollbar_thumb_background;
        let click_scroll = scroll.clone();
        let click_handle = self.handle.clone();
        let wheel_scroll = scroll.clone();
        let wheel_handle = self.handle.clone();
        div()
            .id(id)
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(SETTINGS_SCROLLBAR_WIDTH))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, _| {
                        let Some(thumb) = scroll_thumb_bounds(&paint_scroll, bounds) else {
                            return;
                        };
                        window.paint_quad(gpui::fill(bounds, track_color));
                        window.paint_quad(gpui::fill(thumb, thumb_color).corner_radii(px(3.)));
                    },
                )
                .size_full(),
            )
            .on_scroll_wheel(move |event, window, cx| {
                let delta = event.delta.pixel_delta(window.line_height());
                let offset = wheel_scroll.offset();
                let minimum = -wheel_scroll.max_offset().y;
                wheel_scroll
                    .set_offset(point(offset.x, (offset.y + delta.y).clamp(minimum, px(0.))));
                wheel_handle.update(cx, |_, cx| cx.notify()).ok();
                cx.stop_propagation();
            })
            .on_click(move |event, _, cx| {
                let bounds = click_scroll.bounds();
                let maximum = click_scroll.max_offset().y;
                if bounds.size.height > px(0.) && maximum > px(0.) {
                    let progress =
                        ((event.position().y - bounds.top()) / bounds.size.height).clamp(0., 1.);
                    let offset = click_scroll.offset();
                    click_scroll.set_offset(point(offset.x, -(maximum * progress)));
                    click_handle.update(cx, |_, cx| cx.notify()).ok();
                }
                cx.stop_propagation();
            })
            .into_any_element()
    }

    pub(crate) fn text_input(
        &self,
        id: String,
        field: TextField,
        input: SettingsInput,
    ) -> gpui::AnyElement {
        Zetta::text_input_widget(
            id,
            field,
            input,
            self.focused_input,
            &self.colors,
            self.handle.clone(),
        )
    }

    pub(crate) fn dropdown(
        &self,
        id: String,
        label: String,
        selection: SettingsDropdown,
    ) -> gpui::AnyElement {
        let focused = self.focused_control == Some(SettingsControl::Dropdown(selection));
        Zetta::dropdown_trigger_widget(
            id,
            label,
            selection,
            focused,
            &self.colors,
            self.handle.clone(),
        )
    }

    /// A labelled row of the Configuration page.
    ///
    /// Takes the control it hosts rather than a precomputed flag, for the same
    /// reason [`super::widgets::control_row`] does: the row is what shows focus,
    /// and it is also what reports its laid-out position so the scroll that
    /// brought the keyboard here can be finished accurately. Passing only a
    /// `bool` left the page with the estimate alone, which maps a control's
    /// position in the *tab* order onto the scroll range — and this page's tab
    /// order does not follow its draw order, so clicking a field near the end of
    /// it scrolled the field out of view.
    pub(crate) fn setting_row(
        &self,
        label: &'static str,
        description: &'static str,
        control_id: SettingsControl,
        control: gpui::AnyElement,
    ) -> gpui::AnyElement {
        let focused = self.focused_control.as_ref() == Some(&control_id);
        super::widgets::track_focus_scroll_from(
            h_flex(),
            self.focus_scroll_request.as_ref(),
            &self.settings_scroll,
            std::slice::from_ref(&control_id),
        )
        .w_full()
        .min_h(crate::ui_tokens::ROW_MIN_HEIGHT)
        .px_2()
        .py_2()
        .gap_4()
        .justify_between()
        .border_b_1()
        .border_color(if focused {
            self.colors.border_focused
        } else {
            self.colors.border_variant
        })
        .when(focused, |row| row.bg(self.colors.element_selected))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_sm().text_color(self.colors.text).child(label))
                .child(
                    div()
                        .text_xs()
                        .text_color(self.colors.text_muted)
                        .child(description),
                ),
        )
        .child(
            div()
                .w(crate::ui_tokens::CONTROL_COLUMN_WIDTH)
                .flex_none()
                .child(control),
        )
        .into_any_element()
    }

    pub(crate) fn setting_toggle(
        &self,
        id: SharedString,
        label: &'static str,
        value: bool,
        toggle: SettingsToggle,
    ) -> gpui::AnyElement {
        toggle_switch(id, label, value, toggle, &self.handle)
    }

    pub(crate) fn numeric(&self, setting: ConfigSetting, field: TextField) -> gpui::AnyElement {
        let id = setting.element_id();
        let focused = self.focused_control == Some(SettingsControl::Numeric(setting));
        let decrease_down = self.handle.clone();
        let decrease_up = self.handle.clone();
        let decrease_out = self.handle.clone();
        let increase_down = self.handle.clone();
        let increase_up = self.handle.clone();
        let increase_out = self.handle.clone();
        let colors = &self.colors;
        h_flex()
            .id(id.clone())
            .h_9()
            .w_full()
            .rounded(crate::ui_tokens::RADIUS_CONTROL)
            .border_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .child(
                div()
                    .id(format!("{id}-decrease"))
                    .h_full()
                    .w_9()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.element_hover))
                    .child("−")
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        decrease_down
                            .update(cx, |this, cx| this.begin_numeric_repeat(setting, -1, cx))
                            .ok();
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                        decrease_up
                            .update(cx, |this, cx| this.end_numeric_repeat(cx))
                            .ok();
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                        decrease_out
                            .update(cx, |this, cx| this.end_numeric_repeat(cx))
                            .ok();
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(Zetta::text_input_widget_with_placeholder(
                        format!("{id}-value"),
                        field,
                        SettingsInput::Configuration(ConfigTextField::Setting(setting)),
                        self.focused_input,
                        self.numeric_placeholder(setting),
                        &self.colors,
                        self.handle.clone(),
                    )),
            )
            .child(
                div()
                    .id(format!("{id}-increase"))
                    .h_full()
                    .w_9()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(colors.element_hover))
                    .child("+")
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        increase_down
                            .update(cx, |this, cx| this.begin_numeric_repeat(setting, 1, cx))
                            .ok();
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                        increase_up
                            .update(cx, |this, cx| this.end_numeric_repeat(cx))
                            .ok();
                    })
                    .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                        increase_out
                            .update(cx, |this, cx| this.end_numeric_repeat(cx))
                            .ok();
                    }),
            )
            .into_any_element()
    }

    /// What an empty stepped field means, for the settings where empty is a
    /// value of its own rather than a mistake.
    fn numeric_placeholder(&self, setting: ConfigSetting) -> Option<SharedString> {
        let SettingKind::Number(spec) = setting.spec().kind else {
            return None;
        };
        Some(match spec.placeholder()? {
            Placeholder::ThemeFontSize => {
                format!("Default ({})", self.terminal_font_size_default).into()
            }
            Placeholder::Text(text) => text.into(),
        })
    }

    pub(crate) fn opacity_slider(&self, opacity: f32, target: OpacityTarget) -> gpui::AnyElement {
        let opacity = opacity.clamp(0., 1.);
        let control = SettingsControl::Opacity(target);
        let focused = self.focused_control.as_ref() == Some(&control);
        let stop_prefix: SharedString = format!("{target:?}-opacity-stop").into();
        let colors = &self.colors;
        let stops = (0usize..=20)
            .map(|step| {
                let slider_handle = self.handle.clone();
                let click_control = control.clone();
                div()
                    .id((stop_prefix.clone(), step))
                    .h_full()
                    .flex_1()
                    .cursor_pointer()
                    .on_click(move |_, window, cx| {
                        slider_handle
                            .update(cx, |this, cx| {
                                // Clicking a slider puts the keyboard on it, as
                                // clicking any other control does.
                                this.focus_settings_control_without_scroll(
                                    click_control.clone(),
                                    window,
                                    cx,
                                );
                                this.set_settings_opacity(target, step as f32 / 20., cx);
                            })
                            .ok();
                    })
            })
            .collect::<Vec<_>>();
        // Drawn at the value itself rather than the nearest stop, so a file's
        // 0.83 shows as 83% where it used to read as 85%.
        let fraction = opacity;
        h_flex()
            .w_full()
            .gap_3()
            .rounded(crate::ui_tokens::RADIUS_CONTROL)
            .border_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border
            })
            .child(
                div()
                    .relative()
                    .h_5()
                    .min_w_0()
                    .flex_1()
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
                            .w(gpui::relative(fraction))
                            .h_1()
                            .rounded_full()
                            .bg(colors.text_accent),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(gpui::relative(fraction))
                            .ml(px(-5.))
                            .size(px(10.))
                            .rounded_full()
                            .border_1()
                            .border_color(colors.border_focused)
                            .bg(colors.text_accent),
                    )
                    .child(h_flex().absolute().inset_0().children(stops)),
            )
            .child(
                div()
                    .w(px(44.))
                    .text_right()
                    .text_sm()
                    .child(format!("{}%", (opacity * 100.).round() as u32)),
            )
            .into_any_element()
    }
}

/// Where the thumb of a scrollbar `track` sits for `scroll`'s current
/// geometry, or `None` when its content fits and there is nothing to show.
pub(crate) fn scroll_thumb_bounds(
    scroll: &ScrollHandle,
    track: gpui::Bounds<Pixels>,
) -> Option<gpui::Bounds<Pixels>> {
    let viewport = scroll.bounds().size.height;
    let maximum = scroll.max_offset().y;
    if maximum <= px(0.) || viewport <= px(0.) {
        return None;
    }
    let thumb_fraction = (viewport / (viewport + maximum)).clamp(0.08, 1.);
    let progress = (-scroll.offset().y / maximum).clamp(0., 1.);
    let height = track.size.height * thumb_fraction;
    let top = track.top() + (track.size.height - height) * progress;
    Some(gpui::Bounds::new(
        point(track.right() - px(8.), top),
        gpui::size(px(6.), height),
    ))
}

#[cfg(test)]
#[path = "../tests/settings_view/form_widgets.rs"]
mod tests;
