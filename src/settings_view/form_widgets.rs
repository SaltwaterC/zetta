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

pub(crate) struct SettingsFormWidgets {
    colors: ThemeColors,
    handle: WeakEntity<Zetta>,
    focused_input: Option<SettingsInput>,
    focused_control: Option<SettingsControl>,
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
        }
    }

    pub(crate) fn scroll_indicator(&self, id: String, scroll: &ScrollHandle) -> gpui::AnyElement {
        let viewport = scroll.bounds().size.height;
        let maximum = scroll.max_offset().y;
        let content_height = viewport + maximum;
        let thumb_fraction = if content_height > px(0.) {
            (viewport / content_height).clamp(0.08, 1.)
        } else {
            1.
        };
        let progress = if maximum > px(0.) {
            (-scroll.offset().y / maximum).clamp(0., 1.)
        } else {
            0.
        };
        let top_fraction = progress * (1. - thumb_fraction);
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
            .bg(self.colors.scrollbar_track_background)
            .cursor_pointer()
            .child(
                div()
                    .absolute()
                    .right(px(2.))
                    .top(gpui::relative(top_fraction))
                    .h(gpui::relative(thumb_fraction))
                    .w(px(6.))
                    .rounded_full()
                    .bg(self.colors.scrollbar_thumb_background),
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
            self.colors.clone(),
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
            self.colors.clone(),
            self.handle.clone(),
        )
    }

    pub(crate) fn setting_row(
        &self,
        label: &'static str,
        description: &'static str,
        focused: bool,
        control: gpui::AnyElement,
    ) -> gpui::AnyElement {
        h_flex()
            .w_full()
            .min_h(px(54.))
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
            .child(div().w(px(330.)).flex_none().child(control))
            .into_any_element()
    }

    pub(crate) fn setting_toggle(
        &self,
        id: &'static str,
        value: bool,
        toggle: SettingsToggle,
    ) -> gpui::AnyElement {
        let toggle_handle = self.handle.clone();
        switch(id, value.into())
            .label(if value { "On" } else { "Off" })
            .full_width(true)
            .aria_label(id)
            .on_click(move |state, window, cx| {
                toggle_handle
                    .update(cx, |this, cx| {
                        this.set_settings_toggle(toggle, state.selected(), window, cx);
                    })
                    .ok();
            })
            .into_any_element()
    }

    pub(crate) fn numeric(
        &self,
        id: &'static str,
        field: TextField,
        setting: NumericSetting,
        input: ConfigTextField,
    ) -> gpui::AnyElement {
        let focused = self.focused_control == Some(SettingsControl::Numeric(setting));
        let decrease_down = self.handle.clone();
        let decrease_up = self.handle.clone();
        let decrease_out = self.handle.clone();
        let increase_down = self.handle.clone();
        let increase_up = self.handle.clone();
        let increase_out = self.handle.clone();
        let colors = &self.colors;
        h_flex()
            .id(id)
            .h_9()
            .w_full()
            .rounded(px(4.))
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
            .child(div().min_w_0().flex_1().child(self.text_input(
                format!("{id}-value"),
                field,
                SettingsInput::Configuration(input),
            )))
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

    pub(crate) fn opacity_slider(&self, opacity: f32, target: OpacityTarget) -> gpui::AnyElement {
        let selected = (opacity.clamp(0., 1.) * 20.).round() as usize;
        let control = match target {
            OpacityTarget::Configuration => SettingsControl::Opacity,
            OpacityTarget::Project => SettingsControl::ProjectOpacity,
        };
        let focused = self.focused_control == Some(control);
        let colors = &self.colors;
        let stops = (0usize..=20)
            .map(|step| {
                let slider_handle = self.handle.clone();
                div()
                    .id(("inactive-opacity-stop", step))
                    .h_full()
                    .flex_1()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        slider_handle
                            .update(cx, |this, cx| {
                                this.set_settings_opacity(target, step as f32 / 20., cx);
                            })
                            .ok();
                    })
            })
            .collect::<Vec<_>>();
        let fraction = selected as f32 / 20.;
        h_flex()
            .w_full()
            .gap_3()
            .rounded(px(4.))
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
                    .child(format!("{}%", selected * 5)),
            )
            .into_any_element()
    }
}
