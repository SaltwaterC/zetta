//! Drawing the picker. One builder per section, each taking a borrowed
//! bundle of what it shows.

use super::*;

pub(super) fn remote_session_list_panel(
    field: RemoteSessionField,
    colors: &ThemeColors,
    error_color: Hsla,
    error: Option<String>,
    session_count: usize,
    rows: gpui::UniformList,
) -> impl IntoElement {
    div()
        .id("remote-session-list-panel")
        .w_full()
        .rounded(crate::ui_tokens::RADIUS_CONTROL)
        .border_1()
        .border_color(if field == RemoteSessionField::List {
            colors.border_focused
        } else {
            transparent_black()
        })
        .when(session_count > 0, |panel| panel.child(rows))
        .when_some(error, |panel, error| {
            panel.child(div().p_2().min_w_0().child(remote_session_error(
                error,
                error_color,
                LabelSize::Small,
            )))
        })
}

/// The session list: one row per session the remote host is holding.
///
/// Virtualized, because a host that has been up for a while can be holding
/// more sessions than the panel shows at once.
pub(super) fn remote_session_rows(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    picker_scroll: &gpui::UniformListScrollHandle,
    sessions: Vec<zmux::protocol::BackgroundSessionSummary>,
    selected: usize,
) -> gpui::UniformList {
    let row_colors = colors.clone();
    let row_handle = handle.clone();
    uniform_list(
        "remote-session-list",
        sessions.len(),
        move |range: std::ops::Range<usize>, _, _| {
            range
                .map(|index| {
                    let session = &sessions[index];
                    let row_handle = row_handle.clone();
                    let title = if session.title.is_empty() {
                        format!("Session {}", session.id)
                    } else {
                        session.title.clone()
                    };
                    let detail = if session.authentication_required {
                        "Protected session · secret required".to_owned()
                    } else {
                        format!(
                            "{} pane{}{}",
                            session.panes.len(),
                            if session.panes.len() == 1 { "" } else { "s" },
                            if session.held {
                                " · already in use"
                            } else {
                                ""
                            }
                        )
                    };
                    div()
                        .id(("remote-session-row", index))
                        .h_12()
                        .w_full()
                        .px_3()
                        .flex()
                        .flex_col()
                        .justify_center()
                        .cursor_pointer()
                        .border_1()
                        .border_color(if index == selected {
                            row_colors.border_focused
                        } else {
                            transparent_black()
                        })
                        .when(index == selected, |row| row.bg(row_colors.element_selected))
                        .hover(|style| style.bg(row_colors.element_hover))
                        .on_click(move |_, window, cx| {
                            row_handle
                                .update(cx, |this, cx| {
                                    if let Some(picker) = this.remote_session_picker.as_mut() {
                                        picker.field = RemoteSessionField::List;
                                        picker.selected = index;
                                    }
                                    this.select_remote_session(index, window, cx);
                                })
                                .ok();
                        })
                        .child(div().text_sm().child(title))
                        .child(
                            div()
                                .text_xs()
                                .text_color(row_colors.text_muted)
                                .child(detail),
                        )
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(px(280.))
    .track_scroll(picker_scroll)
    .on_scroll(|_, _, cx| cx.stop_propagation())
}

/// The host suggestions under the target field, from the user's SSH
/// configuration.
pub(super) fn remote_session_suggestion_rows(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    suggestion_scroll: &gpui::UniformListScrollHandle,
    suggestions: Vec<String>,
    selected: Option<usize>,
) -> gpui::UniformList {
    let row_colors = colors.clone();
    let row_handle = handle.clone();
    uniform_list(
        "remote-session-suggestions",
        suggestions.len(),
        move |range: std::ops::Range<usize>, _, _| {
            range
                .map(|index| {
                    let suggestion = suggestions[index].clone();
                    let suggestion_handle = row_handle.clone();
                    let suggestion_label = suggestion.clone();
                    div()
                        .id(("remote-session-suggestion", index))
                        .debug_selector(|| format!("remote-session-suggestion-{index}"))
                        .h_7()
                        .w_full()
                        .px_2()
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(row_colors.text_muted)
                        .border_1()
                        .border_color(if selected == Some(index) {
                            row_colors.border_focused
                        } else {
                            transparent_black()
                        })
                        .when(selected == Some(index), |row| {
                            row.bg(row_colors.element_selected)
                        })
                        .hover(|style| style.bg(row_colors.element_hover))
                        .on_click(move |_, _, cx| {
                            suggestion_handle
                                .update(cx, |this, cx| {
                                    if let Some(picker) = this.remote_session_picker.as_mut() {
                                        picker.target = TextField::new(suggestion.clone());
                                        picker.field = RemoteSessionField::Port;
                                        picker.reset_suggestion_navigation();
                                        picker.invalidate_results();
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                        .child(suggestion_label)
                })
                .collect::<Vec<_>>()
        },
    )
    .with_sizing_behavior(ListSizingBehavior::Infer)
    .max_h(REMOTE_SESSION_SUGGESTION_VIEWPORT_HEIGHT)
    .track_scroll(suggestion_scroll)
    .on_scroll(|_, _, cx| cx.stop_propagation())
}

pub(super) fn remote_session_suggestion_list(
    rows: gpui::UniformList,
    suggestion_scroll: &gpui::UniformListScrollHandle,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Div {
    div().relative().w_full().child(rows).child(
        div()
            .id("remote-session-suggestions-scrollbar-layer")
            .debug_selector(|| "remote-session-suggestions-scrollbar".to_owned())
            .absolute()
            .inset_0()
            .custom_scrollbars(
                Scrollbars::always_visible(ScrollAxes::Vertical)
                    .tracked_scroll_handle(suggestion_scroll)
                    .id("remote-session-suggestions-scrollbar-state"),
                window,
                cx,
            ),
    )
}

/// One of the picker's two text fields — the SSH target and the port.
///
/// A shared builder rather than two spellings: they differ only in what they
/// hold and which field selecting them focuses.
pub(super) fn remote_session_field(
    id: &'static str,
    value: TextField,
    selected_field: RemoteSessionField,
    placeholder: &'static str,
    field: RemoteSessionField,
    colors: &ThemeColors,
    click_handle: WeakEntity<Zetta>,
) -> AnyElement {
    let focused = field == selected_field;
    boxed_text_field(
        id,
        &value,
        focused,
        Some(placeholder.into()),
        FieldMask::Plain,
        colors,
    )
    .debug_selector(move || id.to_owned())
    .flex_1()
    .min_w_0()
    .cursor_text()
    .on_click(move |_, _, cx| {
        click_handle
            .update(cx, |this, cx| {
                if let Some(picker) = this.remote_session_picker.as_mut() {
                    if selected_field == RemoteSessionField::Target {
                        picker.reset_suggestion_navigation();
                    }
                    picker.field = selected_field;
                    cx.notify();
                }
            })
            .ok();
    })
    .into_any_element()
}

/// What the picker says it is about to do, which depends on what carries the
/// panes. Control traffic is OpenSSH either way, and that is the part a user
/// has to have configured.
pub(super) fn remote_session_description(transport: RemotePaneTransport) -> &'static str {
    if transport.is_zosh() {
        "Sessions are found through your normal OpenSSH configuration, and each pane is then \
         carried over Zosh. Remote sessions must be shared."
    } else {
        "Connect through your normal OpenSSH configuration. Remote sessions must be shared."
    }
}

/// What the protocol row needs. A bundle rather than a parameter list: it is
/// the picker's own state plus the two things every row here is built from.
pub(super) struct RemoteSessionTransportRow<'a, F> {
    pub(super) transport: RemotePaneTransport,
    pub(super) keep_alive: TextField,
    pub(super) forward_agent: bool,
    pub(super) field: RemoteSessionField,
    pub(super) colors: &'a ThemeColors,
    pub(super) field_widget: &'a F,
    pub(super) handle: &'a WeakEntity<Zetta>,
}

/// The protocol choice, and the keep-alive interval that only Zosh has.
pub(super) fn remote_session_transport_row<F>(
    row: RemoteSessionTransportRow<'_, F>,
) -> impl IntoElement
where
    F: Fn(
        &'static str,
        TextField,
        RemoteSessionField,
        &'static str,
        WeakEntity<Zetta>,
    ) -> AnyElement,
{
    let RemoteSessionTransportRow {
        transport,
        keep_alive,
        forward_agent,
        field,
        colors,
        field_widget,
        handle,
    } = row;
    h_flex()
        .w_full()
        .gap_2()
        .items_center()
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Panes over"),
        )
        .child(remote_session_protocol_control(
            transport,
            field == RemoteSessionField::Protocol,
            colors,
            handle,
        ))
        .child(remote_session_agent_control(
            forward_agent,
            field == RemoteSessionField::ForwardAgent,
            colors,
            handle,
        ))
        .when(transport.is_zosh(), |row| {
            row.child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child("Keep-alive"),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(120.))
                    .debug_selector(|| "remote-session-keep-alive-control".to_owned())
                    .child(field_widget(
                        "remote-session-keep-alive",
                        keep_alive,
                        RemoteSessionField::KeepAlive,
                        KEEP_ALIVE_PLACEHOLDER,
                        handle.clone(),
                    )),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.text_muted)
                    .child("ms"),
            )
        })
}

pub(super) fn remote_session_agent_control(
    enabled: bool,
    focused: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> impl IntoElement {
    let handle = handle.clone();
    div()
        .id("remote-session-forward-agent")
        .debug_selector(|| "remote-session-forward-agent".to_owned())
        .flex_none()
        .h_9()
        .px_2()
        .flex()
        .items_center()
        .rounded(crate::ui_tokens::RADIUS_CONTROL)
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .text_xs()
        .cursor_pointer()
        .tooltip(Tooltip::text(
            "Enable SSH agent forwarding for remote panes",
        ))
        .hover(|style| style.bg(colors.element_hover))
        .on_click(move |_, _, cx| {
            handle
                .update(cx, |this, cx| {
                    if let Some(picker) = this.remote_session_picker.as_mut() {
                        picker.forward_agent = !picker.forward_agent;
                        picker.field = RemoteSessionField::ForwardAgent;
                        cx.notify();
                    }
                })
                .ok();
        })
        .child(if enabled { "Agent: On" } else { "Agent: Off" })
}

/// The placeholder names what an empty field means, which is not "nothing":
/// Mosh still has its own three-second heartbeat.
pub(super) const KEEP_ALIVE_PLACEHOLDER: &str = "Off";

/// Two buttons that behave as one control: the protocol the panes take.
pub(super) fn remote_session_protocol_control(
    transport: RemotePaneTransport,
    focused: bool,
    colors: &ThemeColors,
    handle: &WeakEntity<Zetta>,
) -> impl IntoElement {
    h_flex()
        .flex_none()
        .h_9()
        .debug_selector(|| "remote-session-protocol".to_owned())
        .rounded(crate::ui_tokens::RADIUS_CONTROL)
        .border_1()
        .border_color(if focused {
            colors.border_focused
        } else {
            colors.border
        })
        .children(
            [RemotePaneTransport::SSH, RemotePaneTransport::ZOSH].map(|name| {
                let selected = transport.name() == name;
                let handle = handle.clone();
                div()
                    .id(SharedString::from(format!(
                        "remote-session-protocol-{name}"
                    )))
                    .debug_selector(move || format!("remote-session-protocol-{name}"))
                    .h_full()
                    .px_3()
                    .flex()
                    .items_center()
                    .text_xs()
                    .cursor_pointer()
                    .when(selected, |option| {
                        option.bg(colors.element_selected).text_color(colors.text)
                    })
                    .when(!selected, |option| option.text_color(colors.text_muted))
                    .hover(|style| style.bg(colors.element_hover))
                    .on_click(move |_, _, cx| {
                        handle
                            .update(cx, |this, cx| {
                                let Some(picker) = this.remote_session_picker.as_mut() else {
                                    return;
                                };
                                if picker.transport.name() != name {
                                    picker.toggle_transport();
                                }
                                picker.field = RemoteSessionField::Protocol;
                                cx.notify();
                            })
                            .ok();
                    })
                    .child(if name == RemotePaneTransport::SSH {
                        "SSH"
                    } else {
                        "Zosh"
                    })
            }),
        )
}

/// The SSH target and port fields, side by side.
pub(super) fn remote_session_fields(
    field_widget: &impl Fn(
        &'static str,
        TextField,
        RemoteSessionField,
        &'static str,
        WeakEntity<Zetta>,
    ) -> AnyElement,
    target: TextField,
    port: TextField,
    handle: &WeakEntity<Zetta>,
) -> impl IntoElement {
    let target_handle = handle.clone();
    let port_handle = handle.clone();
    h_flex()
        .w_full()
        .gap_2()
        .child(field_widget(
            "remote-session-target",
            target,
            RemoteSessionField::Target,
            "SSH target or alias",
            target_handle,
        ))
        .child(div().flex_none().w(px(120.)).child(field_widget(
            "remote-session-port",
            port,
            RemoteSessionField::Port,
            "Port",
            port_handle,
        )))
}

/// The choices that turn the remote-session picker into a create form. They
/// are still picker fields, so the same keyboard tab order and visible focus
/// treatment work for both the list and the create action.
pub(super) struct RemoteSessionCreateOptions<'a> {
    pub(super) profile: String,
    pub(super) template: String,
    pub(super) profiles_loading: bool,
    pub(super) profile_available: bool,
    pub(super) profile_error: Option<String>,
    pub(super) field: RemoteSessionField,
    pub(super) colors: &'a ThemeColors,
    pub(super) error_color: Hsla,
    pub(super) handle: &'a WeakEntity<Zetta>,
}

pub(super) fn remote_session_create_options(
    options: RemoteSessionCreateOptions<'_>,
) -> impl IntoElement {
    let RemoteSessionCreateOptions {
        profile,
        template,
        profiles_loading,
        profile_available,
        profile_error,
        field,
        colors,
        error_color,
        handle,
    } = options;
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(colors.text_muted)
                .child("Create a new session")
                .when(profiles_loading, |label| {
                    label.child(" · discovering profiles…")
                }),
        )
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .child(remote_session_dropdown(RemoteSessionDropdownChoice {
                    id: "remote-session-profile",
                    label: "Profile",
                    value: profile,
                    disabled: !profile_available,
                    field: RemoteSessionField::Profile,
                    focused: field == RemoteSessionField::Profile,
                    colors,
                    handle: handle.clone(),
                }))
                .child(remote_session_dropdown(RemoteSessionDropdownChoice {
                    id: "remote-session-template",
                    label: "Template",
                    value: template,
                    disabled: false,
                    field: RemoteSessionField::Template,
                    focused: field == RemoteSessionField::Template,
                    colors,
                    handle: handle.clone(),
                })),
        )
        .when_some(profile_error, |panel, error| {
            panel.child(remote_session_error(error, error_color, LabelSize::XSmall))
        })
}

pub(super) fn remote_session_error(
    error: String,
    error_color: Hsla,
    size: LabelSize,
) -> impl IntoElement {
    div().min_w_0().child(
        Label::new(error)
            .size(size)
            .color(Color::Custom(error_color))
            .line_clamp(3),
    )
}

pub(super) struct RemoteSessionDropdownChoice<'a> {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) disabled: bool,
    pub(super) field: RemoteSessionField,
    pub(super) focused: bool,
    pub(super) colors: &'a ThemeColors,
    pub(super) handle: WeakEntity<Zetta>,
}

pub(super) fn remote_session_dropdown(choice: RemoteSessionDropdownChoice<'_>) -> impl IntoElement {
    let RemoteSessionDropdownChoice {
        id,
        label,
        value,
        disabled,
        field,
        focused,
        colors,
        handle,
    } = choice;
    let dropdown = match field {
        RemoteSessionField::Profile => RemoteSessionDropdown::Profile,
        RemoteSessionField::Template => RemoteSessionDropdown::Template,
        _ => unreachable!(),
    };
    let menu_handle = handle;
    let button = ButtonLike::new(id)
        .style(ButtonStyle::Outlined)
        .toggle_state(focused)
        .selected_style(ButtonStyle::OutlinedCustom(colors.border_focused))
        .full_width()
        .height(px(40.).into())
        .disabled(disabled)
        .aria_value(value.clone())
        .aria_expanded(focused)
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_start()
                        .child(div().text_xs().text_color(colors.text_muted).child(label))
                        .child(
                            div()
                                .min_w_0()
                                .max_w_full()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_sm()
                                .child(value),
                        ),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(IconSize::XSmall)
                        .color(Color::Custom(colors.text_muted)),
                ),
        )
        .on_click(move |event, _window, cx| {
            menu_handle
                .update(cx, |this, cx| {
                    if let Some(picker) = this.remote_session_picker.as_mut()
                        && picker.open_dropdown(dropdown, event.position())
                    {
                        cx.notify();
                    }
                })
                .ok();
        });
    div()
        .flex_1()
        .min_w_0()
        .debug_selector(move || format!("{id}-trigger"))
        .child(button)
}

pub(super) fn remote_session_dropdown_popup(
    dropdown: RemoteSessionDropdown,
    colors: ThemeColors,
    state: SearchableDropdownRenderState,
    handle: WeakEntity<Zetta>,
) -> AnyElement {
    let menu_handle = handle;
    let on_select = move |choice: DropdownChoice, cx: &mut App| {
        menu_handle
            .update(cx, |this, cx| {
                if let Some(picker) = this.remote_session_picker.as_mut() {
                    picker.commit_dropdown(choice.value);
                }
                cx.notify();
            })
            .ok();
    };
    searchable_dropdown_popup(
        format!("remote-session-dropdown-{dropdown:?}"),
        colors,
        state,
        |_, _| None,
        on_select,
    )
}

/// What the picker's action row needs to decide which buttons are live.
pub(super) struct RemoteSessionActions<'a> {
    pub(super) loading: bool,
    pub(super) attaching: bool,
    pub(super) session_count: usize,
    pub(super) selected: usize,
    pub(super) field: RemoteSessionField,
    pub(super) create_available: bool,
    pub(super) creating: bool,
    pub(super) colors: &'a ThemeColors,
    pub(super) cancel_handle: WeakEntity<Zetta>,
    pub(super) load_handle: WeakEntity<Zetta>,
    pub(super) attach_handle: WeakEntity<Zetta>,
    pub(super) create_handle: WeakEntity<Zetta>,
}

/// Cancel, Load, Create and Attach, with the session actions live only
/// while their respective background operations are available.
pub(super) fn remote_session_actions(actions: RemoteSessionActions<'_>) -> impl IntoElement {
    let RemoteSessionActions {
        loading,
        attaching,
        session_count,
        selected,
        field,
        create_available,
        creating,
        colors,
        cancel_handle,
        load_handle,
        attach_handle,
        create_handle,
    } = actions;
    let instruction = if creating {
        "Creating remote session…".to_owned()
    } else if attaching {
        "Attaching the selected session…".to_owned()
    } else if loading {
        "Loading remote sessions…".to_owned()
    } else if session_count == 0 {
        "Enter or Load loads sessions".to_owned()
    } else {
        "Enter or Attach attaches the selected session".to_owned()
    };
    let footer = format!(
        "{instruction} · {}",
        key_hints(&[
            ("Tab", "next"),
            ("↑↓", "choose"),
            ("←→", "change"),
            (&format!("{PRIMARY_MODIFIER}+Enter"), "create"),
            (&format!("{ALT_MODIFIER}+Enter"), "attach"),
            ("Esc", "cancel"),
        ]),
    );
    // One primary action: attaching once there is something to attach to,
    // creating until then.
    let attach_is_primary = session_count > 0;
    let action = |selector: &'static str, button: AnyElement| {
        div()
            .debug_selector(move || selector.to_owned())
            .child(button)
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            hint_line(footer, colors)
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .debug_selector(|| "remote-session-action-help".to_owned()),
        )
        .child(
            dialog_buttons()
                .w_full()
                .flex_none()
                .debug_selector(|| "remote-session-actions".to_owned())
                .child(action(
                    "remote-session-cancel-action",
                    DialogButton::new("cancel-remote-session", "Cancel", ButtonRole::Secondary)
                        .key_tooltip("Cancel", SurfaceKey::Escape)
                        .focused(field == RemoteSessionField::Cancel)
                        .render(colors, move |_, window, cx| {
                            cancel_handle
                                .update(cx, |this, cx| {
                                    if let Some(picker) = this.remote_session_picker.as_mut() {
                                        picker.field = RemoteSessionField::Cancel;
                                    }
                                    this.dismiss_remote_session_picker(window, cx);
                                })
                                .ok();
                        }),
                ))
                .child(action(
                    "remote-session-load-action",
                    DialogButton::new("load-remote-sessions", "Load", ButtonRole::Secondary)
                        .focused(field == RemoteSessionField::Load)
                        .loading(loading)
                        .render(colors, move |_, window, cx| {
                            load_handle
                                .update(cx, |this, cx| {
                                    if let Some(picker) = this.remote_session_picker.as_mut() {
                                        picker.field = RemoteSessionField::Load;
                                    }
                                    this.load_remote_sessions(window, cx);
                                })
                                .ok();
                        }),
                ))
                .child(action(
                    "remote-session-create-action",
                    DialogButton::new(
                        "create-remote-session",
                        "Create",
                        if attach_is_primary {
                            ButtonRole::Secondary
                        } else {
                            ButtonRole::Primary
                        },
                    )
                    .focused(field == RemoteSessionField::Create)
                    .loading(creating)
                    .enabled(create_available)
                    .render(colors, move |_, window, cx| {
                        create_handle
                            .update(cx, |this, cx| {
                                if let Some(picker) = this.remote_session_picker.as_mut() {
                                    picker.field = RemoteSessionField::Create;
                                }
                                this.create_remote_session(window, cx);
                            })
                            .ok();
                    }),
                ))
                .child(action(
                    "remote-session-attach-action",
                    DialogButton::new(
                        "attach-remote-session",
                        "Attach",
                        if attach_is_primary {
                            ButtonRole::Primary
                        } else {
                            ButtonRole::Secondary
                        },
                    )
                    .focused(field == RemoteSessionField::Attach)
                    .loading(attaching)
                    .enabled(!loading && session_count > 0)
                    .render(colors, move |_, window, cx| {
                        attach_handle
                            .update(cx, |this, cx| {
                                if let Some(picker) = this.remote_session_picker.as_mut() {
                                    picker.field = RemoteSessionField::Attach;
                                }
                                this.select_remote_session(selected, window, cx);
                            })
                            .ok();
                    }),
                )),
        )
}

impl Zetta {
    pub(crate) fn render_remote_session_overlay(
        &self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let picker = self.remote_session_picker.as_ref()?;
        let target = picker.target.clone();
        let port = picker.port.clone();
        let transport = picker.transport;
        let keep_alive = picker.keep_alive.clone();
        let field = picker.field;
        let error = picker.error.clone();
        let loading = picker.loading;
        let session_count = picker.sessions.len();
        let selected = picker.selected.min(session_count.saturating_sub(1));
        let sessions = picker.sessions.clone();
        let suggestions = picker.visible_suggestions();
        let suggestion_selected = picker
            .suggestion_navigation
            .as_ref()
            .map(|navigation| navigation.selected);
        let suggestion_scroll = picker.suggestion_scroll.clone();
        let picker_scroll = picker.scroll.clone();
        let profiles_loading = picker.profiles_loading;
        let profile_available = !profiles_loading && !picker.profiles.is_empty();
        let open_dropdown = picker.open_dropdown;
        let dropdown_state = picker.dropdown.render_state();
        let profile_value = picker
            .profiles
            .get(picker.selected_profile)
            .cloned()
            .unwrap_or_else(|| {
                if profiles_loading {
                    "Loading…".to_owned()
                } else {
                    "Load remote profiles".to_owned()
                }
            });
        let template_value = picker.templates.get(picker.selected_template).map_or_else(
            || "No templates configured".to_owned(),
            |template| template.label().to_owned(),
        );
        let profile_error = picker.profile_error.clone();
        let can_create = picker.can_create_action();
        let creating = picker.creating;
        let attaching = picker.loading && picker.attach_generation.is_some();
        let dropdown_popup = open_dropdown.map(|dropdown| {
            remote_session_dropdown_popup(dropdown, colors.clone(), dropdown_state, handle.clone())
        });
        let rows = remote_session_rows(handle, colors, &picker_scroll, sessions, selected);

        let cancel_handle = handle.clone();
        let load_handle = handle.clone();
        let attach_handle = handle.clone();
        let create_handle = handle.clone();
        let has_suggestions = !suggestions.is_empty();
        let suggestion_rows = remote_session_suggestion_rows(
            handle,
            colors,
            &suggestion_scroll,
            suggestions,
            suggestion_selected,
        );
        let suggestion_rows =
            remote_session_suggestion_list(suggestion_rows, &suggestion_scroll, window, cx);

        let session_list =
            remote_session_list_panel(field, colors, error_color, error, session_count, rows);

        let field_widget = |id: &'static str,
                            value: TextField,
                            selected_field: RemoteSessionField,
                            placeholder: &'static str,
                            click_handle: WeakEntity<Self>| {
            remote_session_field(
                id,
                value,
                selected_field,
                placeholder,
                field,
                colors,
                click_handle,
            )
        };
        Some(
            modal_backdrop(
                "remote-session-backdrop",
                Placement::Centered,
                BackdropClick::Swallow,
            )
            .child(
                dialog_panel("remote-session-picker", DIALOG_WIDTH_LARGE, colors)
                    .debug_selector(|| "remote-session-picker".to_owned())
                    .track_focus(&self.remote_session_focus)
                    .child(dialog_title("Open remote session", colors))
                    .child(
                        div()
                            .min_h(px(45.))
                            .text_sm()
                            .text_color(colors.text_muted)
                            .debug_selector(|| "remote-session-description".to_owned())
                            .child(remote_session_description(transport)),
                    )
                    .child(remote_session_fields(&field_widget, target, port, handle))
                    .child(remote_session_transport_row(RemoteSessionTransportRow {
                        transport,
                        keep_alive,
                        forward_agent: picker.forward_agent,
                        field,
                        colors,
                        field_widget: &field_widget,
                        handle,
                    }))
                    .child(remote_session_create_options(RemoteSessionCreateOptions {
                        profile: profile_value,
                        template: template_value,
                        profiles_loading,
                        profile_available,
                        profile_error,
                        field,
                        colors,
                        error_color,
                        handle,
                    }))
                    .when(has_suggestions, |panel| {
                        panel.child(
                            suggestion_rows
                                .when(field != RemoteSessionField::Target, |suggestions| {
                                    suggestions.invisible()
                                }),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(Label::new("Shared sessions").size(LabelSize::Small))
                            .child(div().text_xs().text_color(colors.text_muted).child(
                                if loading {
                                    "Loading…".to_owned()
                                } else {
                                    format!(
                                        "{session_count} session{}",
                                        if session_count == 1 { "" } else { "s" }
                                    )
                                },
                            )),
                    )
                    .child(session_list)
                    .child(remote_session_actions(RemoteSessionActions {
                        loading,
                        attaching,
                        session_count,
                        selected,
                        field,
                        colors,
                        cancel_handle,
                        load_handle,
                        attach_handle,
                        create_handle,
                        create_available: can_create,
                        creating,
                    })),
            )
            .when_some(dropdown_popup, |backdrop, popup| backdrop.child(popup))
            .into_any_element(),
        )
    }
}
