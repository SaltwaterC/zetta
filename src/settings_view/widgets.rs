use super::*;

/// Owned snapshot of the state needed to render the currently open dropdown's option
/// popover. The popover is always rendered once, as a sibling of the settings dialog
/// content (see `dropdown_popup_widget`), rather than inline at each trigger, because a
/// `deferred`+`anchored` popover positioned inline inside a virtualized `uniform_list`
/// row (the keymap bindings list) does not paint correctly.
#[derive(Clone)]
pub(crate) struct DropdownRenderState {
    pub(crate) dropdown_index: usize,
    pub(crate) dropdown_query: String,
    pub(crate) dropdown_filtered_options: HashMap<SettingsDropdown, Vec<usize>>,
    pub(crate) dropdown_scroll: UniformListScrollHandle,
    pub(crate) dropdown_anchor: Point<Pixels>,
}

/// Every row of the keymap list is forced to this height so `uniform_list`'s
/// single-item height measurement (it only measures one representative row)
/// stays valid across section headers, bindings, and the add-row footers.
pub(crate) const KEYMAP_ROW_HEIGHT: f32 = 56.;

/// Width of the settings dialog's custom scrollbar track. Lists that draw the track over
/// their own rows reserve this much trailing padding so the two never overlap.
pub(crate) const SETTINGS_SCROLLBAR_WIDTH: f32 = 10.;

/// Owned per-row data for the virtualized keymap list, extracted from
/// `SettingsEditor` once per render since the list's row closure must be
/// `'static` and so cannot hold a borrow of it.
pub(crate) enum KeymapRowData {
    SectionHeader {
        section_index: usize,
        context: TextField,
    },
    Binding {
        section_index: usize,
        binding_index: usize,
        keystroke: TextField,
        action_name: String,
        template_name: Option<String>,
        profile_name: Option<String>,
    },
    AddBinding {
        section_index: usize,
        context: String,
    },
    AddSection,
}

pub(crate) fn build_keymap_row_data(
    editor: &SettingsEditor,
    rows: &[KeymapRow],
) -> Vec<KeymapRowData> {
    rows.iter()
        .filter_map(|row| match *row {
            KeymapRow::SectionHeader(section_index) => {
                let section = editor.keymap.sections.get(section_index)?;
                Some(KeymapRowData::SectionHeader {
                    section_index,
                    context: section.context.clone(),
                })
            }
            KeymapRow::Binding(section_index, binding_index) => {
                let binding = editor
                    .keymap
                    .sections
                    .get(section_index)?
                    .bindings
                    .get(binding_index)?;
                let profile_name = binding.action_usize_parameter("slot").map(|slot| {
                    editor
                        .profile_names
                        .get(slot.saturating_sub(1))
                        .cloned()
                        .unwrap_or_else(|| format!("Profile {slot}"))
                });
                Some(KeymapRowData::Binding {
                    section_index,
                    binding_index,
                    keystroke: binding.keystroke.clone(),
                    action_name: binding.action_name(),
                    template_name: binding.action_parameter("name"),
                    profile_name,
                })
            }
            KeymapRow::AddBinding(section_index) => {
                let context = editor
                    .keymap
                    .sections
                    .get(section_index)
                    .map(|section| section.context.text.clone())
                    .unwrap_or_default();
                Some(KeymapRowData::AddBinding {
                    section_index,
                    context,
                })
            }
            KeymapRow::AddSection => Some(KeymapRowData::AddSection),
        })
        .collect()
}

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
        options: Arc<[String]>,
        selection: SettingsDropdown,
        colors: ThemeColors,
        handle: WeakEntity<Self>,
        state: DropdownRenderState,
    ) -> gpui::AnyElement {
        let id = format!("settings-dropdown-popup-{selection:?}");
        let active_index = state.dropdown_index.min(options.len().saturating_sub(1));
        let dropdown_query = state.dropdown_query.clone();
        let matching_indices = state.dropdown_filtered_options.get(&selection).cloned();
        let option_handle = handle.clone();
        // Row indices into `options`, in display order; virtualized below so only the
        // visible rows are ever built regardless of how many options exist.
        let row_indices: Arc<[usize]> = match matching_indices {
            Some(indices) => indices.into(),
            None => (0..options.len()).collect::<Vec<_>>().into(),
        };
        let no_matches = row_indices.is_empty();
        // `uniform_list` derives the whole list's width from a single measured row, so it has
        // to measure the longest option; measuring the first one leaves every longer option
        // wrapping inside a row whose height is pinned to the measured row's single line.
        let widest_row = row_indices
            .iter()
            .enumerate()
            .max_by_key(|(_, index)| options[**index].chars().count())
            .map(|(row, _)| row);
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
                                .child(value.clone())
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
        .with_priority(1)
        .into_any_element()
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
                        .child(field.text.clone()),
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
                let remove_handle = ctx.handle.clone();
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
                    .child(
                        IconButton::new(
                            format!("remove-settings-binding-{section_index}-{binding_index}"),
                            IconName::Trash,
                        )
                        .icon_size(IconSize::Small)
                        .toggle_state(
                            ctx.focused_control
                                == Some(SettingsControl::RemoveBinding(
                                    section_index,
                                    binding_index,
                                )),
                        )
                        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                        .tooltip(Tooltip::text("Remove binding"))
                        .on_click(move |_, _, cx| {
                            remove_handle
                                .update(cx, |this, cx| {
                                    if let Some(editor) = this.settings_editor.as_mut()
                                        && let Some(section) =
                                            editor.keymap.sections.get_mut(section_index)
                                        && binding_index < section.bindings.len()
                                    {
                                        section.bindings.remove(binding_index);
                                        editor.keymap_dirty = true;
                                        invalidate_keymap_cache(editor);
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
                                        invalidate_keymap_cache(editor);
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
                                            invalidate_keymap_cache(editor);
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
