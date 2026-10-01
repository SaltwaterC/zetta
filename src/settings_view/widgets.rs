use super::*;
pub(crate) use crate::searchable_dropdown::{
    DropdownChoice, SearchableDropdownRenderState, searchable_dropdown_popup,
};
use crate::settings_editor::SettingKind;
use crate::settings_ui::keymap::GLOBAL_CONTEXT_LABEL;
use crate::ui_tokens::{
    CONTROL_COLUMN_WIDTH, DENSE_ROW_MIN_HEIGHT, DISABLED_OPACITY, RADIUS_CONTROL,
};

/// Owned snapshot of the state needed to render the currently open dropdown's option
/// popover. The popover is always rendered once, as a sibling of the settings dialog
/// content (see `dropdown_popup_widget`), rather than inline at each trigger, because a
/// `deferred`+`anchored` popover positioned inline inside a virtualized `uniform_list`
/// row (the keymap bindings list) does not paint correctly.
#[derive(Clone)]
pub(crate) struct DropdownRenderState {
    pub(crate) dropdown: SearchableDropdownRenderState,
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

/// [`KeymapRowData::Binding`]'s fields, borrowed, so the row builder takes one
/// parameter rather than six adjacent indices, strings and flags.
///
/// `action_name` stays a separate parameter: it is the only one of them the
/// dropdown needs owned.
struct KeymapBindingRow<'a> {
    section_index: usize,
    binding_index: usize,
    keystroke: &'a TextField,
    template_name: Option<&'a str>,
    profile_name: Option<&'a str>,
    is_default: bool,
}

/// How much of the form stays visible past a control the keyboard just moved to,
/// so it never sits flush against the edge of the scroll region.
const FOCUS_SCROLL_MARGIN: Pixels = px(10.);

/// The click handler of a settings control: focus it without scrolling (the
/// pointer is already on it), then run it exactly as the keyboard would.
///
/// Every button in the dialog goes through this rather than mutating the form
/// in its own closure. The two used to be written separately and drifted: the
/// keyboard's copy of the keymap edits skipped the cache refresh, and its
/// removal of a built-in binding deleted it where the button disabled it.
pub(crate) fn activate_on_click(
    handle: &WeakEntity<Zetta>,
    control: SettingsControl,
) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let handle = handle.clone();
    move |_, window, cx| {
        cx.stop_propagation();
        handle
            .update(cx, |this, cx| {
                this.focus_settings_control_without_scroll(control.clone(), window, cx);
                this.activate_settings_control(control.clone(), window, cx);
            })
            .ok();
    }
}

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
    track_focus_scroll_from(
        element,
        editor.focus_scroll_request.as_ref(),
        &editor.settings_scroll,
        controls,
    )
}

/// [`track_focus_scroll`] for a builder that snapshots what it needs rather than
/// holding the editor — see [`super::form_widgets::SettingsFormWidgets`], which
/// renders the Configuration page in its own view.
pub(crate) fn track_focus_scroll_from(
    element: Div,
    request: Option<&(SettingsControl, Pixels)>,
    settings_scroll: &ScrollHandle,
    controls: &[SettingsControl],
) -> Div {
    let Some((target, requested_offset)) = request else {
        return element;
    };
    if !controls.iter().any(|candidate| candidate == target) {
        return element;
    }
    let scroll = settings_scroll.clone();
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

/// A compact, keyboard-reachable button for a [`SettingsControl`]: a
/// [`DialogButton`] that clicks through [`activate_on_click`], so the focus
/// ring and the keyboard path stay in agreement, and that reports its bounds
/// for the scroll that brought focus to it.
pub(crate) struct SettingsButton {
    id: String,
    label: String,
    control: SettingsControl,
    role: ButtonRole,
    enabled: bool,
    loading: bool,
    /// What the button says once a first press has armed it; see
    /// `SettingsEditor::armed_control`.
    confirm_label: Option<String>,
}

impl SettingsButton {
    pub(crate) fn new(id: String, label: impl Into<String>, control: SettingsControl) -> Self {
        Self {
            id,
            label: label.into(),
            control,
            role: ButtonRole::Secondary,
            enabled: true,
            loading: false,
            confirm_label: None,
        }
    }

    /// Work this button started is in flight.
    pub(crate) fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    /// For the one action a section exists for, such as installing a theme.
    pub(crate) fn primary(mut self) -> Self {
        self.role = ButtonRole::Primary;
        self
    }

    /// For an action that cannot be undone from the dialog: the first press
    /// arms the button, which then says `label`, and only a second press on it
    /// acts. The control's activation is what enforces this; the label is how
    /// the button shows it.
    pub(crate) fn confirm_with(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = Some(label.into());
        self
    }

    /// For a button that throws something away: a split, a pane, a project.
    pub(crate) fn destructive(mut self) -> Self {
        self.role = ButtonRole::Destructive;
        self
    }

    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn render(
        self,
        editor: &SettingsEditor,
        colors: &ThemeColors,
        handle: &WeakEntity<Zetta>,
    ) -> AnyElement {
        let focused = editor.focused_control.as_ref() == Some(&self.control);
        let armed = editor.armed_control.as_ref() == Some(&self.control);
        let label = match self.confirm_label {
            Some(confirm) if armed => confirm,
            _ => self.label,
        };
        track_focus_scroll(div(), editor, std::slice::from_ref(&self.control))
            .flex_none()
            .child(
                DialogButton::new(SharedString::from(self.id), label, self.role)
                    .compact(true)
                    .enabled(self.enabled)
                    .loading(self.loading)
                    .focused(focused)
                    .render(colors, activate_on_click(handle, self.control)),
            )
            .into_any_element()
    }
}

/// [`SettingsButton`] in the secondary role, the one most of the forms' buttons
/// have.
pub(crate) fn action_button(
    editor: &SettingsEditor,
    id: String,
    label: String,
    control: SettingsControl,
    enabled: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    SettingsButton::new(id, label, control)
        .enabled(enabled)
        .render(editor, colors, handle)
}

/// The button that removes one entry of a list — an argument, an environment
/// variable, a stacked command — named for what it removes in its tooltip and
/// its accessible label. These used to be a bare `×` that said neither.
pub(crate) fn settings_remove_button(
    editor: &SettingsEditor,
    id: String,
    control: SettingsControl,
    what: &str,
    enabled: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let focused = editor.focused_control.as_ref() == Some(&control);
    let button = if enabled {
        remove_button(
            SharedString::from(id),
            what,
            focused,
            colors,
            activate_on_click(handle, control.clone()),
        )
    } else {
        // Read-only forms keep the button's place so the row does not reflow
        // between an editable template and a built-in one.
        div()
            .size_6()
            .flex_none()
            .opacity(DISABLED_OPACITY)
            .into_any_element()
    };
    track_focus_scroll(div(), editor, std::slice::from_ref(&control))
        .flex_none()
        .child(button)
        .into_any_element()
}

/// A label-and-control row for the denser forms (pane templates, the project
/// builder), where `SettingsFormWidgets::setting_row`'s two-line layout would
/// be too tall — unless the row has something to explain, which goes in
/// `description` under the label rather than into the label itself.
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
    described_control_row(editor, label, None, controls, control, colors)
}

/// [`control_row`] with a line under the label saying what the value means:
/// its unit, its range, or what leaving it empty does.
pub(crate) fn described_control_row(
    editor: &SettingsEditor,
    label: impl Into<String>,
    description: Option<&str>,
    controls: &[SettingsControl],
    control: AnyElement,
    colors: &ThemeColors,
) -> AnyElement {
    let focused = controls
        .iter()
        .any(|candidate| editor.focused_control.as_ref() == Some(candidate));
    track_focus_scroll(h_flex(), editor, controls)
        .w_full()
        .min_h(DENSE_ROW_MIN_HEIGHT)
        .py_1()
        .gap_3()
        .justify_between()
        .border_b_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border_variant
        })
        .when(focused, |row| row.bg(colors.element_selected))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_xs().child(label.into()))
                .when_some(description, |label, description| {
                    label.child(
                        div()
                            .text_xs()
                            .text_color(colors.text_muted)
                            .child(description.to_owned()),
                    )
                }),
        )
        .child(div().w(CONTROL_COLUMN_WIDTH).flex_none().child(control))
        .into_any_element()
}

/// A section's heading on a settings page: its title, and a line saying what
/// the section is for.
///
/// Every page's headings used to be spelled out where they were drawn, with
/// three different spacings and two colours for the same role, and two of the
/// Configuration page's five groups had none at all.
pub(crate) fn section_heading(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    colors: &ThemeColors,
) -> AnyElement {
    v_flex()
        .pt_4()
        .pb_2()
        .child(
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(colors.text)
                .child(title.into()),
        )
        .when_some(description, |heading, description| {
            heading.child(
                div()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child(description),
            )
        })
        .into_any_element()
}

/// The row an Add button sits in at the foot of a list: left-aligned, under
/// the rows it adds to, and scrolled to like any other control.
///
/// Half the pages right-aligned these and half left-aligned them.
pub(crate) fn add_row(
    button: AnyElement,
    editor: &SettingsEditor,
    controls: &[SettingsControl],
) -> AnyElement {
    track_focus_scroll(h_flex(), editor, controls)
        .w_full()
        .py_2()
        .child(button)
        .into_any_element()
}

/// The frame of a card in a settings list — a theme extension, a profile, a
/// project, a template — with the border and fill that report keyboard focus
/// inside it.
///
/// Four builders used to draw this with two radii, and two of them never showed
/// focus at all.
pub(crate) fn card_frame(focused: bool, colors: &ThemeColors) -> Div {
    div()
        .mb_2()
        .p_3()
        .rounded(RADIUS_CONTROL)
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .bg(if focused {
            colors.element_selected
        } else {
            colors.editor_background
        })
}

/// A card's title and the muted lines under it.
pub(crate) fn card_text(
    title: impl Into<SharedString>,
    lines: impl IntoIterator<Item = SharedString>,
    colors: &ThemeColors,
) -> Div {
    div()
        .min_w_0()
        .flex_1()
        .child(div().text_sm().text_color(colors.text).child(title.into()))
        .children(lines.into_iter().map(|line| {
            div()
                .mt_1()
                .text_xs()
                .text_color(colors.text_muted)
                .child(line)
        }))
}

/// A settings switch. On means the thing its label names is on — every switch
/// in the dialog reads that way now, including the ones whose file key is a
/// `hide_…` — and `label` is what a screen reader announces, where the switches
/// used to announce their element ids.
pub(crate) fn toggle_switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    value: bool,
    toggle: SettingsToggle,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    let toggle_handle = handle.clone();
    switch(id, value.into())
        .label(if value { "On" } else { "Off" })
        .full_width(true)
        .aria_label(label)
        .on_click(move |state, window, cx| {
            toggle_handle
                .update(cx, |this, cx| {
                    this.set_settings_toggle(toggle, state.selected(), window, cx);
                })
                .ok();
        })
        .into_any_element()
}

/// A row control that opens a picker rather than editing in place — the font
/// family, a tab icon. Drawn as the dropdown triggers are, since to a reader it
/// is one: the value it holds and a chevron. The Configuration page and the
/// project builder each had a copy of this, drawn differently from the
/// dropdowns beside them.
pub(crate) fn picker_trigger(
    id: impl Into<ElementId>,
    control: SettingsControl,
    value: impl IntoElement,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    ButtonLike::new(id)
        .style(ButtonStyle::Outlined)
        .toggle_state(editor.focused_control.as_ref() == Some(&control))
        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
        .full_width()
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(colors.text)
                        .child(value),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(IconSize::XSmall)
                        .color(Color::Custom(colors.text_muted)),
                ),
        )
        .on_click(activate_on_click(handle, control))
        .into_any_element()
}

/// One environment variable of a list, as two rows: its name, then its value
/// with the button that removes the pair. That is the order the keyboard
/// reaches them in; the remove button used to be drawn beside the name while
/// being tabbed after the value.
///
/// Four lists share this — a project's environment, a project command's, a
/// template's and a template pane's.
pub(crate) struct EnvironmentPair<'a> {
    /// What the rows are labelled, such as `Variable 3`.
    pub(crate) label: String,
    /// The prefix the rows' element ids are built from.
    pub(crate) id: String,
    pub(crate) name: &'a TextField,
    pub(crate) name_input: SettingsInput,
    pub(crate) value: &'a TextField,
    pub(crate) value_input: SettingsInput,
    pub(crate) remove: SettingsControl,
    /// A built-in template's pairs are shown, not edited.
    pub(crate) editable: bool,
}

pub(crate) fn environment_pair_rows(
    pair: EnvironmentPair<'_>,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> [AnyElement; 2] {
    let EnvironmentPair {
        label,
        id,
        name,
        name_input,
        value,
        value_input,
        remove,
        editable,
    } = pair;
    let field = |suffix: &str, text: &TextField, input: SettingsInput| {
        editable_field(
            format!("{id}-{suffix}"),
            text,
            input,
            editable,
            editor,
            colors,
            handle,
        )
    };
    [
        control_row(
            editor,
            format!("{label} · name"),
            &[SettingsControl::Input(name_input)],
            field("name", name, name_input),
            colors,
        ),
        control_row(
            editor,
            format!("{label} · value"),
            &[SettingsControl::Input(value_input), remove.clone()],
            h_flex()
                .gap_1()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .child(field("value", value, value_input)),
                )
                .child(settings_remove_button(
                    editor,
                    format!("{id}-remove"),
                    remove,
                    "environment variable",
                    editable,
                    colors,
                    handle,
                ))
                .into_any_element(),
            colors,
        ),
    ]
}

/// [`text_field`], or the same value shown read-only when the form cannot be
/// edited. A built-in template's fields used to be live: clicking one focused
/// it and typing into it was silently dropped.
pub(crate) fn editable_field(
    id: String,
    field: &TextField,
    input: SettingsInput,
    editable: bool,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    if editable {
        text_field(id, field.clone(), input, editor, colors, handle)
    } else {
        read_only_field(SharedString::from(id), field.text.clone(), colors)
            .w_full()
            .into_any_element()
    }
}

/// [`dropdown_field`], or its value shown read-only when the form cannot be
/// edited: a built-in template's dropdowns used to open and then ignore the
/// choice.
pub(crate) fn editable_dropdown(
    id: String,
    label: String,
    selection: SettingsDropdown,
    editable: bool,
    editor: &SettingsEditor,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    if editable {
        dropdown_field(id, label, selection, editor, colors, handle)
    } else {
        read_only_field(SharedString::from(id), label, colors)
            .w_full()
            .into_any_element()
    }
}

/// A profile's arguments: one field per argument, each with the button that
/// removes it, then the button that adds one. The same list for a configured
/// profile, the Add profile draft and a project's override.
///
/// `scroll` is the region the list scrolls in — the page's, or the Add profile
/// modal's own — so a focused argument is scrolled into view in the right one.
pub(crate) fn argument_list(
    editor: &SettingsEditor,
    target: ProfileTarget,
    arguments: &[TextField],
    id_prefix: &str,
    scroll: &ScrollHandle,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_1()
        .children(arguments.iter().enumerate().map(|(argument, value)| {
            let input = target.argument_input(argument);
            let remove = SettingsControl::RemoveProfileArgument(target, argument);
            track_focus_scroll_from(
                h_flex().w_full().gap_1(),
                editor.focus_scroll_request.as_ref(),
                scroll,
                &[SettingsControl::Input(input), remove.clone()],
            )
            .child(div().min_w_0().flex_1().child(text_field(
                format!("{id_prefix}-argument-{argument}"),
                value.clone(),
                input,
                editor,
                colors,
                handle,
            )))
            .child(settings_remove_button(
                editor,
                format!("{id_prefix}-remove-argument-{argument}"),
                remove,
                "argument",
                true,
                colors,
                handle,
            ))
        }))
        .child(
            h_flex().child(
                SettingsButton::new(
                    format!("{id_prefix}-add-argument"),
                    "Add argument",
                    SettingsControl::AddProfileArgument(target),
                )
                .render(editor, colors, handle),
            ),
        )
        .into_any_element()
}

/// What a list shows when it has nothing in it yet.
pub(crate) fn empty_state(text: impl Into<SharedString>, colors: &ThemeColors) -> AnyElement {
    div()
        .py_4()
        .text_sm()
        .text_color(colors.text_muted)
        .child(text.into())
        .into_any_element()
}

/// Whether a field holds one of the stepped numbers, which are centred between
/// their `−` and `+` buttons.
fn settings_input_is_numeric(input: SettingsInput) -> bool {
    matches!(
        input,
        SettingsInput::Configuration(ConfigTextField::Setting(setting))
            if matches!(setting.spec().kind, SettingKind::Number(_))
    )
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
        colors,
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
        colors,
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
        colors: &ThemeColors,
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
                    .child(Label::new(label).color(Color::Custom(colors.text)))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(IconSize::XSmall)
                            .color(Color::Custom(colors.text_muted)),
                    ),
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
        let profile_icon_automatic = state.profile_icon_automatic.clone();
        let leading = move |value: &str, _colors: &ThemeColors| {
            Self::profile_icon_dropdown_option(selection, value, profile_icon_automatic.as_ref())
                .map(|icon| icon.render(IconSize::Small).into_any_element())
        };
        let menu_handle = handle.clone();
        let on_select = move |choice: DropdownChoice, cx: &mut App| {
            menu_handle
                .update(cx, |this, cx| {
                    this.set_settings_dropdown(selection, choice, cx);
                    if let Some(editor) = this.settings_editor.as_mut() {
                        editor.clear_dropdown();
                    }
                    cx.notify();
                })
                .ok();
        };
        searchable_dropdown_popup(id, colors, state.dropdown, leading, on_select)
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
        colors: &ThemeColors,
        handle: WeakEntity<Self>,
    ) -> gpui::AnyElement {
        let placeholder = match input {
            SettingsInput::Keymap(KeymapTextField::Context(_)) => Some(GLOBAL_CONTEXT_LABEL),
            SettingsInput::KeymapSearch => Some("Search bindings…"),
            SettingsInput::ThemeSearch => Some("Search Zed themes…"),
            SettingsInput::FontSearch => Some("Search fonts…"),
            _ => None,
        }
        .map(SharedString::new_static);
        Self::text_input_widget_with_placeholder(
            id,
            field,
            input,
            focused_input,
            placeholder,
            colors,
            handle,
        )
    }

    /// [`Self::text_input_widget`] with what an empty field shows instead of
    /// nothing — the value that applies while the setting is unset.
    pub(crate) fn text_input_widget_with_placeholder(
        id: String,
        field: TextField,
        input: SettingsInput,
        focused_input: Option<SettingsInput>,
        placeholder: Option<SharedString>,
        colors: &ThemeColors,
        handle: WeakEntity<Self>,
    ) -> gpui::AnyElement {
        let focused = focused_input == Some(input);
        let input_handle = handle.clone();
        boxed_text_field(id, &field, focused, placeholder, FieldMask::Plain, colors)
            .w_full()
            .min_w(px(180.))
            // The size the dropdowns and pickers beside it draw their values
            // at, whatever the row around it inherits.
            .text_sm()
            .when(settings_input_is_numeric(input), |input| {
                input.justify_center()
            })
            .cursor_text()
            .on_click(move |_, window, cx| {
                input_handle
                    .update(cx, |this, cx| this.focus_settings_input(input, window, cx))
                    .ok();
            })
            .into_any_element()
    }

    /// One row of the Keymap page. Each kind of row is its own builder below;
    /// this only decides which.
    pub(crate) fn render_keymap_row(
        row: &KeymapRowData,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        match row {
            KeymapRowData::SectionHeader {
                section_index,
                context,
            } => Self::render_keymap_section_header(*section_index, context, ctx),
            KeymapRowData::Binding {
                section_index,
                binding_index,
                keystroke,
                action_name,
                template_name,
                profile_name,
                is_default,
            } => Self::render_keymap_binding_row(
                KeymapBindingRow {
                    section_index: *section_index,
                    binding_index: *binding_index,
                    keystroke,
                    template_name: template_name.as_deref(),
                    profile_name: profile_name.as_deref(),
                    is_default: *is_default,
                },
                action_name,
                ctx,
            ),
            KeymapRowData::UnboundDefault {
                section_index,
                binding_index,
                keystroke,
                action_name,
            } => Self::render_keymap_unbound_row(
                *section_index,
                *binding_index,
                keystroke,
                action_name,
                ctx,
            ),
            KeymapRowData::AddBinding {
                section_index,
                context,
            } => Self::render_keymap_add_binding_row(*section_index, context, ctx),
            KeymapRowData::AddSection => Self::render_keymap_add_section_row(ctx),
        }
    }

    /// A keymap context heading, which is itself the editable context string.
    fn render_keymap_section_header(
        section_index: usize,
        context: &TextField,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        let colors = &ctx.colors;
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
                colors,
                ctx.handle.clone(),
            )))
            .into_any_element()
    }

    /// One binding: its keystroke, the action it runs, the optional template and
    /// profile that scope it, and the button that unbinds or removes it.
    fn render_keymap_binding_row(
        row: KeymapBindingRow<'_>,
        action_name: &str,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        let KeymapBindingRow {
            section_index,
            binding_index,
            keystroke,
            template_name,
            profile_name,
            is_default,
        } = row;
        let colors = &ctx.colors;
        // The row shows focus for every control it hosts, as `control_row`
        // does; the dropdowns' and the button's own rings are a pixel wide.
        let binding_focused = ctx.focused_control.as_ref().is_some_and(|control| {
            matches!(
                control,
                SettingsControl::Input(SettingsInput::Keymap(KeymapTextField::Keystroke(s, b)))
                | SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(s, b))
                | SettingsControl::Dropdown(
                    SettingsDropdown::BindingAction(s, b)
                        | SettingsDropdown::BindingTemplate(s, b)
                        | SettingsDropdown::BindingProfile(s, b)
                )
                | SettingsControl::RemoveBinding(s, b)
                | SettingsControl::UnbindBinding(s, b)
                    if *s == section_index && *b == binding_index
            )
        });
        let action_focused = ctx.focused_control
            == Some(SettingsControl::Dropdown(SettingsDropdown::BindingAction(
                section_index,
                binding_index,
            )));
        let action = Self::dropdown_trigger_widget(
            format!("settings-binding-{section_index}-{binding_index}-action"),
            action_name.to_owned(),
            SettingsDropdown::BindingAction(section_index, binding_index),
            action_focused,
            colors,
            ctx.handle.clone(),
        );
        let template = template_name.map(|name| {
            let focused = ctx.focused_control
                == Some(SettingsControl::Dropdown(
                    SettingsDropdown::BindingTemplate(section_index, binding_index),
                ));
            Self::dropdown_trigger_widget(
                format!("settings-binding-{section_index}-{binding_index}-template"),
                name.to_owned(),
                SettingsDropdown::BindingTemplate(section_index, binding_index),
                focused,
                colors,
                ctx.handle.clone(),
            )
        });
        let profile = profile_name.map(|name| {
            let focused = ctx.focused_control
                == Some(SettingsControl::Dropdown(SettingsDropdown::BindingProfile(
                    section_index,
                    binding_index,
                )));
            Self::dropdown_trigger_widget(
                format!("settings-binding-{section_index}-{binding_index}-profile"),
                name.to_owned(),
                SettingsDropdown::BindingProfile(section_index, binding_index),
                focused,
                colors,
                ctx.handle.clone(),
            )
        });
        let capture_control = SettingsControl::CaptureKeymap(KeymapTextField::Keystroke(
            section_index,
            binding_index,
        ));
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
                    .w(CONTROL_COLUMN_WIDTH)
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
                        colors,
                        ctx.handle.clone(),
                    ))
                    .child(
                        Button::new(
                            format!("record-settings-binding-{section_index}-{binding_index}"),
                            "Record",
                        )
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Compact)
                        .color(Color::Custom(colors.text))
                        .toggle_state(ctx.focused_control == Some(capture_control.clone()))
                        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                        .on_click(activate_on_click(&ctx.handle, capture_control.clone())),
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
                IconButton::new(
                    format!("unbind-settings-binding-{section_index}-{binding_index}"),
                    icon,
                )
                .icon_size(IconSize::Small)
                .icon_color(Color::Custom(colors.icon))
                .selected_icon_color(Color::Custom(colors.icon))
                .toggle_state(ctx.focused_control.as_ref() == Some(&control_variant))
                .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                .tooltip(Tooltip::text(tooltip_text))
                .on_click(activate_on_click(&ctx.handle, control_variant))
            })
            .into_any_element()
    }

    /// A built-in binding the user has disabled: shown greyed out, with the
    /// button that puts it back.
    fn render_keymap_unbound_row(
        section_index: usize,
        unbound_index: usize,
        keystroke: &TextField,
        action_name: &str,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        let colors = &ctx.colors;
        let restore = SettingsControl::RestoreBinding(section_index, unbound_index);
        let focused = ctx.focused_control.as_ref() == Some(&restore);
        // Shown, not edited: a disabled binding has no field of its own, and
        // reusing a live binding's `Keystroke` input here (as this row once
        // did) focused whichever live binding shared its index.
        h_flex()
            .w_full()
            .h(px(KEYMAP_ROW_HEIGHT))
            .pl_6()
            .pr_2()
            .gap_2()
            .border_b_1()
            .border_color(colors.border_variant)
            .when(focused, |row| row.bg(colors.element_selected))
            .child(
                div()
                    .w(CONTROL_COLUMN_WIDTH)
                    .flex_none()
                    .px_2()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(colors.text_muted)
                    .line_through()
                    .child(keystroke.text.clone()),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_color(colors.text_muted)
                    .child(action_name.to_owned()),
            )
            .child(
                IconButton::new(
                    format!("restore-unbound-{section_index}-{unbound_index}"),
                    IconName::RotateCw,
                )
                .icon_size(IconSize::Small)
                .icon_color(Color::Custom(colors.icon))
                .selected_icon_color(Color::Custom(colors.icon))
                .toggle_state(focused)
                .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                .tooltip(Tooltip::text("Restore built-in binding"))
                .on_click(activate_on_click(&ctx.handle, restore)),
            )
            .into_any_element()
    }

    /// The button that appends a binding to a keymap context.
    fn render_keymap_add_binding_row(
        section_index: usize,
        context: &str,
        ctx: &KeymapRowRenderContext,
    ) -> gpui::AnyElement {
        let colors = &ctx.colors;
        let focused = ctx.focused_control == Some(SettingsControl::AddBinding(section_index));
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
                .color(Color::Custom(colors.text))
                .selected_label_color(Color::Custom(colors.text))
                .toggle_state(focused)
                .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                .on_click(activate_on_click(
                    &ctx.handle,
                    SettingsControl::AddBinding(section_index),
                )),
            )
            .into_any_element()
    }

    /// The button that appends a whole keymap context.
    fn render_keymap_add_section_row(ctx: &KeymapRowRenderContext) -> gpui::AnyElement {
        let colors = &ctx.colors;
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
                    .color(Color::Custom(colors.text))
                    .selected_label_color(Color::Custom(colors.text))
                    .toggle_state(focused)
                    .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
                    .on_click(activate_on_click(
                        &ctx.handle,
                        SettingsControl::AddKeymapSection,
                    )),
            )
            .into_any_element()
    }
}

#[cfg(test)]
#[path = "../tests/settings_view/widgets.rs"]
mod tests;
