use super::*;
use crate::settings_ui::keymap::GLOBAL_CONTEXT_LABEL;

/// Owned snapshot of the state needed to render the currently open dropdown's option
/// popover. The popover is always rendered once, as a sibling of the settings dialog
/// content (see `dropdown_popup_widget`), rather than inline at each trigger, because a
/// `deferred`+`anchored` popover positioned inline inside a virtualized `uniform_list`
/// row (the keymap bindings list) does not paint correctly.
#[derive(Clone)]
pub(crate) struct DropdownRenderState {
    pub(crate) dropdown_index: usize,
    pub(crate) dropdown_query: String,
    /// The options, display rows, and measurement row snapshotted when the
    /// dropdown opened or its query last changed (see `SettingsEditor`).
    pub(crate) options: Arc<[String]>,
    pub(crate) rows: Arc<[usize]>,
    pub(crate) widest_row: Option<usize>,
    pub(crate) dropdown_scroll: UniformListScrollHandle,
    pub(crate) dropdown_anchor: Point<Pixels>,
    pub(crate) profile_icon_automatic: Option<ProfileIcon>,
}

/// Every row of the keymap list is forced to this height so `uniform_list`'s
/// single-item height measurement (it only measures one representative row)
/// stays valid across section headers, bindings, and the add-row footers.
pub(crate) const KEYMAP_ROW_HEIGHT: f32 = 56.;

/// Width of the settings dialog's custom scrollbar track. Lists that draw the track over
/// their own rows reserve this much trailing padding so the two never overlap.
pub(crate) const SETTINGS_SCROLLBAR_WIDTH: f32 = 10.;

/// Owned snapshot of everything a keymap row needs to render, cloned once into
/// the `uniform_list` row closure (see [`DropdownRenderState`] for why this
/// can't just borrow `SettingsEditor`).
#[derive(Clone)]
pub(crate) struct KeymapRowRenderContext {
    pub(crate) colors: ThemeColors,
    pub(crate) handle: WeakEntity<Zetta>,
    pub(crate) focused_control: Option<SettingsControl>,
    pub(crate) focused_input: Option<SettingsInput>,
}

/// How much of the form stays visible past a control the keyboard just moved to,
/// so it never sits flush against the edge of the scroll region.
const FOCUS_SCROLL_MARGIN: Pixels = px(10.);

/// Finishes the scroll to the control the keyboard just moved to, from the
/// bounds that control actually laid out at.
///
/// `scroll_settings_control_into_view` can only estimate: it maps a control's
/// position in the tab order onto the scroll range, which is off wherever rows
/// differ in height or sit in a side column. GPUI offers nothing better for a
/// plain `overflow_y_scroll` div — `Window::request_autoscroll` is honoured only
/// by `List`, and `ScrollHandle::scroll_to_item` addresses direct children — so
/// the element reports itself once it has been laid out and corrects the
/// remainder. The correction is skipped unless the offset is still the one the
/// request was made at, which is what keeps it from fighting a wheel scroll.
pub(crate) fn track_focus_scroll(
    element: Div,
    editor: &SettingsEditor,
    controls: &[SettingsControl],
) -> Div {
    let Some((target, requested_offset)) = editor.focus_scroll_request.as_ref() else {
        return element;
    };
    if !controls.iter().any(|candidate| candidate == target) {
        return element;
    }
    let scroll = editor.settings_scroll.clone();
    let requested_offset = *requested_offset;
    element.on_children_prepainted(move |bounds, window, _| {
        let Some(control) = bounds
            .iter()
            .copied()
            .reduce(|left, right| left.union(&right))
        else {
            return;
        };
        let offset = scroll.offset();
        if (offset.y - requested_offset).abs() > px(1.) {
            return;
        }
        let viewport = scroll.bounds();
        let mut target = offset.y;
        if control.top() - FOCUS_SCROLL_MARGIN < viewport.top() {
            target += viewport.top() - control.top() + FOCUS_SCROLL_MARGIN;
        } else if control.bottom() + FOCUS_SCROLL_MARGIN > viewport.bottom() {
            target -= control.bottom() - viewport.bottom() + FOCUS_SCROLL_MARGIN;
        }
        let target = target.clamp(-scroll.max_offset().y, px(0.));
        if (target - offset.y).abs() > px(1.) {
            scroll.set_offset(point(offset.x, target));
            // The scroll region has already been prepainted with the old offset,
            // so the corrected position lands on the next frame.
            window.request_animation_frame();
        }
    })
}

/// A compact, keyboard-reachable button for a [`SettingsControl`]. Clicking it
/// focuses the control first so the dialog's focus ring and its keyboard path
/// stay in agreement.
pub(crate) fn action_button(
    editor: &SettingsEditor,
    id: String,
    label: String,
    control: SettingsControl,
    enabled: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let focused = editor.focused_control.as_ref() == Some(&control);
    let click_handle = handle.clone();
    let click_control = control.clone();
    track_focus_scroll(div(), editor, std::slice::from_ref(&control))
        .id(id)
        .h_8()
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .text_xs()
        .when(enabled, |button| {
            button
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    click_handle
                        .update(cx, |this, cx| {
                            this.focus_settings_control_without_scroll(
                                click_control.clone(),
                                window,
                                cx,
                            );
                            this.activate_settings_control(click_control.clone(), window, cx);
                        })
                        .ok();
                })
        })
        .when(!enabled, |button| button.opacity(0.5))
        .when(focused, |button| button.bg(colors.element_selected))
        .child(label)
        .into_any_element()
}

/// A label-and-control row for the denser forms (pane templates, the project
/// builder), where `SettingsFormWidgets::setting_row`'s two-line description
/// layout would be too tall.
///
/// The row highlights while any of the controls it hosts holds keyboard focus.
/// That is what `setting_row` does for the Configuration page, and it is why
/// tabbing through that page is easy to follow: a dropdown's or text field's own
/// focus ring is a one-pixel border change, and a switch has none at all, so the
/// row is what actually tracks the keyboard. Rows take the controls they host
/// rather than a precomputed flag, because most of them hold two (a field and
/// the button that removes its row).
pub(crate) fn control_row(
    editor: &SettingsEditor,
    label: impl Into<String>,
    controls: &[SettingsControl],
    control: AnyElement,
    colors: &ThemeColors,
) -> AnyElement {
    let focused = controls
        .iter()
        .any(|candidate| editor.focused_control.as_ref() == Some(candidate));
    track_focus_scroll(h_flex(), editor, controls)
        .w_full()
        .min_h(px(42.))
        .gap_3()
        .justify_between()
        .border_b_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border_variant
        })
        .when(focused, |row| row.bg(colors.element_selected))
        .child(div().min_w_0().flex_1().text_xs().child(label.into()))
        .child(div().w(px(300.)).flex_none().child(control))
        .into_any_element()
}

pub(crate) fn text_field(
    id: String,
    field: TextField,
    input: SettingsInput,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    Zetta::text_input_widget(
        id,
        field,
        input,
        editor.focused_input,
        colors.clone(),
        handle.clone(),
    )
}

pub(crate) fn dropdown_field(
    id: String,
    label: String,
    selection: SettingsDropdown,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    Zetta::dropdown_trigger_widget(
        id,
        label,
        selection,
        editor.focused_control == Some(SettingsControl::Dropdown(selection)),
        colors.clone(),
        handle.clone(),
    )
}

impl Zetta {
    /// Just the trigger button; the option popover is rendered separately by
    /// `dropdown_popup_widget`, once per render, as a sibling of the whole settings
    /// dialog content (see [`DropdownRenderState`] for why).
    pub(crate) fn dropdown_trigger_widget(
        id: String,
        label: String,
        selection: SettingsDropdown,
        focused: bool,
        colors: ThemeColors,
        handle: WeakEntity<Self>,
    ) -> gpui::AnyElement {
        let menu_handle = handle.clone();
        ButtonLike::new(id)
            .style(ButtonStyle::Outlined)
            .toggle_state(focused)
            .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
            .full_width()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .child(Label::new(label))
                    .child(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
            )
            .on_click(move |event, window, cx| {
                let anchor = event.position();
                menu_handle
                    .update(cx, |this, cx| {
                        this.focus_settings_control_without_scroll(
                            SettingsControl::Dropdown(selection),
                            window,
                            cx,
                        );
                        this.open_settings_dropdown(selection, anchor, cx);
                    })
                    .ok();
            })
            .into_any_element()
    }

    /// Renders the currently open dropdown's option popover, anchored at the window-space
    /// point captured when it was opened. Called once per render (see [`DropdownRenderState`]).
    pub(crate) fn dropdown_popup_widget(
        selection: SettingsDropdown,
        colors: ThemeColors,
        handle: WeakEntity<Self>,
        state: DropdownRenderState,
    ) -> gpui::AnyElement {
        let id = format!("settings-dropdown-popup-{selection:?}");
        let options = state.options.clone();
        let active_index = state.dropdown_index.min(options.len().saturating_sub(1));
        let dropdown_query = state.dropdown_query.clone();
        let profile_icon_automatic = state.profile_icon_automatic.clone();
        let option_handle = handle.clone();
        // Row indices into `options`, in display order; virtualized below so only the
        // visible rows are ever built regardless of how many options exist.
        let row_indices = state.rows.clone();
        let no_matches = row_indices.is_empty();
        let widest_row = state.widest_row;
        let option_rows = {
            let row_indices = row_indices.clone();
            let list_colors = colors.clone();
            let list_id = id.clone();
            uniform_list(
                format!("{id}-options-list"),
                row_indices.len(),
                move |range, _, _| {
                    range
                        .map(|row| {
                            let index = row_indices[row];
                            let value = options[index].clone();
                            let selected = index == active_index;
                            let icon = Self::profile_icon_dropdown_option(
                                selection,
                                &value,
                                profile_icon_automatic.as_ref(),
                            );
                            let handle = option_handle.clone();
                            div()
                                .id(format!("{list_id}-option-{index}"))
                                .px_2()
                                .py_1()
                                .rounded(px(3.))
                                .cursor_pointer()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .when(selected, |row| row.bg(list_colors.element_selected))
                                .hover(|style| style.bg(list_colors.element_hover))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .when_some(icon, |row, icon| {
                                            row.child(icon.render(IconSize::Small))
                                        })
                                        .child(value.clone()),
                                )
                                .on_click(move |_, _, cx| {
                                    handle
                                        .update(cx, |this, cx| {
                                            this.set_settings_dropdown(
                                                selection,
                                                value.clone(),
                                                cx,
                                            );
                                            if let Some(editor) = this.settings_editor.as_mut() {
                                                editor.open_dropdown = None;
                                            }
                                            cx.notify();
                                        })
                                        .ok();
                                })
                        })
                        .collect::<Vec<_>>()
                },
            )
            // The popover is content-sized, so the list has to derive its own height
            // from its items; the default `Auto` behaviour only works when a parent
            // hands the list a definite height, and here it collapses the list to zero.
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .with_width_from_item(widest_row)
            .max_h(px(260.))
            .track_scroll(&state.dropdown_scroll)
        };
        deferred(
            anchored()
                .position(state.dropdown_anchor)
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .id(format!("{id}-options"))
                        .min_w(px(180.))
                        .max_w(px(560.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(colors.border_focused)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .when(!dropdown_query.is_empty(), |menu| {
                            menu.child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .text_xs()
                                    .text_color(colors.text_muted)
                                    .child(format!("Search: {dropdown_query}")),
                            )
                        })
                        .child(if no_matches {
                            div()
                                .p_1()
                                .child(
                                    div()
                                        .px_2()
                                        .py_1()
                                        .text_color(colors.text_muted)
                                        .child("No matches"),
                                )
                                .into_any_element()
                        } else {
                            option_rows
                                .p_1()
                                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                                .into_any_element()
                        }),
                ),
        )
        .with_priority(crate::app_render::MODAL_POPUP_PAINT_PRIORITY)
        .into_any_element()
    }

    fn profile_icon_dropdown_option(
        selection: SettingsDropdown,
        value: &str,
        automatic: Option<&ProfileIcon>,
    ) -> Option<ProfileIcon> {
        if !matches!(
            selection,
            SettingsDropdown::ProfileIcon(_) | SettingsDropdown::ProfileDraftIcon
        ) {
            return None;
        }
        match value {
            "Automatic" => automatic.cloned(),
            "Zetta" => Some(ProfileIcon::Zetta),
            "Bash" => Some(ProfileIcon::Bash),
            "Zsh" => Some(ProfileIcon::Zsh),
            "Fish" => Some(ProfileIcon::Fish),
            _ => None,
        }
    }

    pub(crate) fn text_input_widget(
        id: String,
        field: TextField,
        input: SettingsInput,
        focused_input: Option<SettingsInput>,
        colors: ThemeColors,
        handle: WeakEntity<Self>,
    ) -> gpui::AnyElement {
        let focused = focused_input == Some(input);
        let centered = match input {
            SettingsInput::Configuration(
                ConfigTextField::FontSize | ConfigTextField::ScrollHistory,
            ) => true,
            #[cfg(feature = "http-server")]
            SettingsInput::Configuration(ConfigTextField::HttpServerPort) => true,
            #[cfg(feature = "tftp-server")]
            SettingsInput::Configuration(ConfigTextField::TftpServerPort) => true,
            _ => false,
        };
        let keymap_global_placeholder = (field.text.is_empty()
            && matches!(input, SettingsInput::Keymap(KeymapTextField::Context(_))))
        .then_some(GLOBAL_CONTEXT_LABEL);
        let cursor = field.cursor.min(field.text.len());
        let (before, after) = field.text.split_at(cursor);
        let input_handle = handle.clone();
        div()
            .id(id)
            .h_9()
            .w_full()
            .min_w(px(180.))
            .px_2()
            .flex()
            .items_center()
            .when(centered, |input| input.justify_center().text_center())
            .overflow_hidden()
            .rounded(px(4.))
            .border_1()
            .border_color(if focused {
                colors.border_focused
            } else {
                colors.border
            })
            .bg(colors.editor_background)
            .cursor_text()
            .when(field.select_all && focused, |input| {
                input.bg(colors.element_selection_background)
            })
            .when(!focused, |input| {
                input.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .when(keymap_global_placeholder.is_some(), |text| {
                            text.text_color(colors.text_placeholder)
                        })
                        .child(
                            keymap_global_placeholder
                                .unwrap_or(field.text.as_str())
                                .to_owned(),
                        ),
                )
            })
            .when(focused, |input| {
                input
                    .child(div().whitespace_nowrap().child(before.to_owned()))
                    .when(!field.select_all, |input| {
                        input.child(
                            div()
                                .flex_none()
                                .w(px(1.))
                                .h(px(16.))
                                .bg(colors.text_accent),
                        )
                    })
                    .child(div().whitespace_nowrap().child(after.to_owned()))
            })
            .on_click(move |_, window, cx| {
                input_handle
                    .update(cx, |this, cx| this.focus_settings_input(input, window, cx))
                    .ok();
            })
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn render_keymap_row(
        row: &KeymapRowData,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        match row {
            KeymapRowData::SectionHeader {
                section_index,
                context,
            } => {
                let section_index = *section_index;
                let colors = ctx.colors.clone();
                let focused = ctx.focused_control
                    == Some(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Context(section_index),
                    )));
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .gap_2()
                    .px_2()
                    .border_t_1()
                    .border_b_1()
                    .border_color(colors.border)
                    .bg(if focused {
                        colors.element_selected
                    } else {
                        colors.editor_background
                    })
                    .child(div().flex_none().text_sm().child("Context"))
                    .child(div().min_w_0().flex_1().child(Self::text_input_widget(
                        format!("settings-keymap-section-{section_index}-context"),
                        context.clone(),
                        SettingsInput::Keymap(KeymapTextField::Context(section_index)),
                        ctx.focused_input,
                        colors.clone(),
                        ctx.handle.clone(),
                    )))
                    .into_any_element()
            }
            KeymapRowData::Binding {
                section_index,
                binding_index,
                keystroke,
                action_name,
                template_name,
                profile_name,
                is_default,
            } => {
                let section_index = *section_index;
                let binding_index = *binding_index;
                let colors = ctx.colors.clone();
                let binding_focused = ctx.focused_control
                    == Some(SettingsControl::Input(SettingsInput::Keymap(
                        KeymapTextField::Keystroke(section_index, binding_index),
                    )))
                    || ctx.focused_control
                        == Some(SettingsControl::RemoveBinding(section_index, binding_index))
                    || ctx.focused_control
                        == Some(SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(
                            section_index,
                            binding_index,
                        )));
                let action_focused = ctx.focused_control
                    == Some(SettingsControl::Dropdown(SettingsDropdown::BindingAction(
                        section_index,
                        binding_index,
                    )));
                let action = Self::dropdown_trigger_widget(
                    format!("settings-binding-{section_index}-{binding_index}-action"),
                    action_name.clone(),
                    SettingsDropdown::BindingAction(section_index, binding_index),
                    action_focused,
                    colors.clone(),
                    ctx.handle.clone(),
                );
                let template = template_name.as_ref().map(|name| {
                    let focused = ctx.focused_control
                        == Some(SettingsControl::Dropdown(
                            SettingsDropdown::BindingTemplate(section_index, binding_index),
                        ));
                    Self::dropdown_trigger_widget(
                        format!("settings-binding-{section_index}-{binding_index}-template"),
                        name.clone(),
                        SettingsDropdown::BindingTemplate(section_index, binding_index),
                        focused,
                        colors.clone(),
                        ctx.handle.clone(),
                    )
                });
                let profile = profile_name.as_ref().map(|name| {
                    let focused = ctx.focused_control
                        == Some(SettingsControl::Dropdown(SettingsDropdown::BindingProfile(
                            section_index,
                            binding_index,
                        )));
                    Self::dropdown_trigger_widget(
                        format!("settings-binding-{section_index}-{binding_index}-profile"),
                        name.clone(),
                        SettingsDropdown::BindingProfile(section_index, binding_index),
                        focused,
                        colors.clone(),
                        ctx.handle.clone(),
                    )
                });
                let capture_handle = ctx.handle.clone();
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .pl_6()
                    .pr_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .when(binding_focused, |row| row.bg(colors.element_selected))
                    .child(
                        h_flex()
                            .w(px(330.))
                            .gap_1()
                            .flex_none()
                            .child(Self::text_input_widget(
                                format!("settings-binding-{section_index}-{binding_index}-key"),
                                keystroke.clone(),
                                SettingsInput::Keymap(KeymapTextField::Keystroke(
                                    section_index,
                                    binding_index,
                                )),
                                ctx.focused_input,
                                colors.clone(),
                                ctx.handle.clone(),
                            ))
                            .child(
                                Button::new(
                                    format!(
                                        "record-settings-binding-{section_index}-{binding_index}"
                                    ),
                                    "Record",
                                )
                                .style(ButtonStyle::Outlined)
                                .size(ButtonSize::Compact)
                                .on_click(move |_, window, cx| {
                                    capture_handle
                                        .update(cx, |this, cx| {
                                            this.start_keymap_capture(
                                                KeymapTextField::Keystroke(
                                                    section_index,
                                                    binding_index,
                                                ),
                                                window,
                                                cx,
                                            )
                                        })
                                        .ok();
                                }),
                            ),
                    )
                    .child(div().min_w_0().flex_1().child(action))
                    .when_some(template, |row, template| {
                        row.child(div().w(px(180.)).flex_none().child(template))
                    })
                    .when_some(profile, |row, profile| {
                        row.child(div().w(px(180.)).flex_none().child(profile))
                    })
                    .child({
                        let is_default = *is_default;
                        let (icon, tooltip_text, control_variant) = if is_default {
                            (
                                IconName::Slash,
                                "Unbind (disable built-in binding)",
                                SettingsControl::UnbindBinding(section_index, binding_index),
                            )
                        } else {
                            (
                                IconName::Trash,
                                "Remove binding",
                                SettingsControl::RemoveBinding(section_index, binding_index),
                            )
                        };
                        let remove_handle = ctx.handle.clone();
                        IconButton::new(
                            format!("unbind-settings-binding-{section_index}-{binding_index}"),
                            icon,
                        )
                        .icon_size(IconSize::Small)
                        .toggle_state(ctx.focused_control == Some(control_variant))
                        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                        .tooltip(Tooltip::text(tooltip_text))
                        .on_click(move |_, _, cx| {
                            remove_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut()
                                        && let Some(section) =
                                            editor.keymap.sections.get_mut(section_index)
                                        && binding_index < section.bindings.len()
                                    {
                                        let binding = section.bindings.remove(binding_index);
                                        if is_default {
                                            // Add to unbind map
                                            let storage_key =
                                                keymap_keystroke_storage(&binding.keystroke.text);
                                            section
                                                .unbind
                                                .insert(storage_key.clone(), binding.action_name());
                                            // Add to unbound_defaults for immediate UI feedback
                                            section.unbound_defaults.push(BindingForm {
                                                keystroke: binding.keystroke,
                                                action: binding.action,
                                            });
                                        }
                                        editor.keymap_dirty = true;
                                        refresh_keymap_cache(editor);
                                        invalidate_controls_cache(editor);
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                    })
                    .into_any_element()
            }
            KeymapRowData::UnboundDefault {
                section_index,
                binding_index,
                keystroke,
                action_name,
            } => {
                let section_index = *section_index;
                let binding_index = *binding_index;
                let colors = ctx.colors.clone();
                let restore_handle = ctx.handle.clone();
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .pl_6()
                    .pr_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        h_flex()
                            .w(px(330.))
                            .gap_1()
                            .flex_none()
                            .child(Self::text_input_widget(
                                format!("settings-unbound-{section_index}-{binding_index}-key"),
                                keystroke.clone(),
                                SettingsInput::Keymap(KeymapTextField::Keystroke(
                                    section_index,
                                    binding_index,
                                )),
                                ctx.focused_input,
                                colors.clone(),
                                ctx.handle.clone(),
                            ))
                            .child(
                                Button::new(
                                    format!("record-unbound-{section_index}-{binding_index}"),
                                    "Record",
                                )
                                .style(ButtonStyle::Outlined)
                                .size(ButtonSize::Compact)
                                .disabled(true)
                                .on_click(move |_, _, _| {}),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .opacity(0.5)
                            .child(action_name.clone()),
                    )
                    .child(
                        IconButton::new(
                            format!("restore-unbound-{section_index}-{binding_index}"),
                            IconName::RotateCw,
                        )
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Restore binding"))
                        .on_click(move |_, _, cx| {
                            restore_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut()
                                        && let Some(section) =
                                            editor.keymap.sections.get_mut(section_index)
                                        && binding_index < section.unbound_defaults.len()
                                    {
                                        let binding =
                                            section.unbound_defaults.remove(binding_index);
                                        // Remove from unbind map
                                        section.unbind.shift_remove(&keymap_keystroke_storage(
                                            &binding.keystroke.text,
                                        ));
                                        // Add back to bindings
                                        section.bindings.push(BindingForm {
                                            keystroke: binding.keystroke,
                                            action: binding.action,
                                        });
                                        editor.keymap_dirty = true;
                                        refresh_keymap_cache(editor);
                                        invalidate_controls_cache(editor);
                                        cx.notify();
                                    }
                                })
                                .ok();
                        }),
                    )
                    .into_any_element()
            }
            KeymapRowData::AddBinding {
                section_index,
                context,
            } => {
                let section_index = *section_index;
                let colors = ctx.colors.clone();
                let add_handle = ctx.handle.clone();
                let focused =
                    ctx.focused_control == Some(SettingsControl::AddBinding(section_index));
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .pl_6()
                    .pr_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new(
                            format!("add-settings-binding-{section_index}"),
                            format!("Add binding for {context}"),
                        )
                        .style(ButtonStyle::Outlined)
                        .toggle_state(focused)
                        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                        .on_click(move |_, _, cx| {
                            add_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut()
                                        && let Some(section) =
                                            editor.keymap.sections.get_mut(section_index)
                                    {
                                        section.bindings.push(BindingForm {
                                            keystroke: TextField::new("ctrl-shift-x"),
                                            action: serde_json::Value::String(
                                                "zetta::NewTab".to_owned(),
                                            ),
                                        });
                                        editor.keymap_dirty = true;
                                        refresh_keymap_cache(editor);
                                        invalidate_controls_cache(editor);
                                        cx.notify();
                                    }
                                })
                                .ok();
                        }),
                    )
                    .into_any_element()
            }
            KeymapRowData::AddSection => {
                let colors = ctx.colors.clone();
                let add_handle = ctx.handle.clone();
                let focused = ctx.focused_control == Some(SettingsControl::AddKeymapSection);
                h_flex()
                    .w_full()
                    .h(px(KEYMAP_ROW_HEIGHT))
                    .pl_6()
                    .pr_2()
                    .border_b_1()
                    .border_color(colors.border_variant)
                    .child(
                        Button::new("add-keymap-section", "Add keymap context")
                            .style(ButtonStyle::Outlined)
                            .toggle_state(focused)
                            .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                            .on_click(move |_, _, cx| {
                                add_handle
                                    .update(cx, |this, cx| {
                                        if let Some(editor) = this.settings_editor.as_mut() {
                                            editor
                                                .keymap
                                                .sections
                                                .push(KeymapSectionForm::new("Zetta > Terminal"));
                                            editor.keymap_dirty = true;
                                            refresh_keymap_cache(editor);
                                            invalidate_controls_cache(editor);
                                            cx.notify();
                                        }
                                    })
                                    .ok();
                            }),
                    )
                    .into_any_element()
            }
        }
    }
}
