//! The title bar's session controls: reconnect, keep-running, broadcast
//! input and silent mode, and which of them the current width leaves room for.

use super::*;

/// What deciding the title bar's session controls depends on.
pub(super) struct TitleBarSessionInputs<'a> {
    pub(super) window: &'a Window,
    pub(super) compact_mode: bool,
    pub(super) background_sessions: Arc<[(u64, u64, String, String)]>,
    pub(super) right_window_controls: AnyElement,
    pub(super) handle: &'a WeakEntity<Zetta>,
}

/// The session-related half of the title bar: whether its controls carry labels
/// at this width, and the reconnect control's two possible placements.
pub(super) struct TitleBarSessionControls {
    pub(super) show_title_bar_control_labels: bool,
    pub(super) show_title_bar_buttons: bool,
    pub(super) reconnect_control: Option<AnyElement>,
    pub(super) right_title_bar_controls: AnyElement,
}

/// The Broadcast toggle, whose tooltip explains what typing into a broadcast
/// pane does to the others.
pub(super) fn broadcast_input_control(
    broadcast_input: bool,
    show_title_bar_control_labels: bool,
    compact_mode: bool,
    title_bar_background: gpui::Hsla,
) -> AnyElement {
    let button = Button::new(
        "toggle-broadcast-input",
        if show_title_bar_control_labels {
            "Broadcast"
        } else {
            ""
        },
    )
    .start_icon(
        Icon::new(IconName::Keyboard)
            .size(IconSize::Small)
            .color(if broadcast_input {
                Color::Selected
            } else {
                Color::Default
            }),
    )
    .style(ButtonStyle::Subtle)
    .size(ButtonSize::Large)
    .toggle_state(broadcast_input)
    .aria_label("Broadcast input to all panes")
    .tooltip(Tooltip::for_action_title(
        if broadcast_input {
            "Broadcast input is on"
        } else {
            "Broadcast input to all panes"
        },
        &ToggleBroadcastInput,
    ))
    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleBroadcastInput), cx));

    if compact_mode {
        render_compact_tab_neighbor_control(button, title_bar_background)
    } else {
        button.into_any_element()
    }
}

/// What the Silent toggle needs: its own state, the system's do-not-disturb, and
/// whether Zetta is allowed to read the latter.
pub(super) struct SilentControl {
    pub(super) silent_mode: bool,
    pub(super) system_silent: bool,
    pub(super) focus_status_access: FocusStatusAccess,
    pub(super) show_title_bar_control_labels: bool,
    pub(super) compact_mode: bool,
    pub(super) title_bar_background: gpui::Hsla,
}

/// The Silent toggle. Its tooltip is the only place that says whether the state
/// came from Zetta or from the system.
pub(super) fn silent_mode_control(control: SilentControl) -> AnyElement {
    let SilentControl {
        silent_mode,
        system_silent,
        focus_status_access,
        show_title_bar_control_labels,
        compact_mode,
        title_bar_background,
    } = control;
    let silent_tooltip_title = if system_silent {
        "Silent mode is controlled by Do Not Disturb"
    } else if cfg!(target_os = "macos") {
        focus_status_access.tooltip()
    } else {
        "Silence terminal bells and notification sounds"
    };
    let silent_tooltip = Tooltip::for_action_title(silent_tooltip_title, &ToggleSilentMode);
    let button = Button::new(
        "toggle-silent-mode",
        if show_title_bar_control_labels {
            "Silent"
        } else {
            ""
        },
    )
    .start_icon(
        Icon::new(title_bar_silent_icon(silent_mode))
            .size(IconSize::Small)
            .color(if silent_mode {
                Color::Selected
            } else {
                Color::Default
            }),
    )
    .style(ButtonStyle::Subtle)
    .size(ButtonSize::Large)
    .toggle_state(silent_mode)
    .disabled(system_silent)
    .aria_label(if silent_mode {
        "Silent mode is on"
    } else {
        "Silent mode is off"
    })
    .tooltip(silent_tooltip)
    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSilentMode), cx));

    if compact_mode {
        render_compact_tab_neighbor_control(button, title_bar_background)
    } else {
        button.into_any_element()
    }
}

impl Zetta {
    /// The session-related half of the title bar's controls.
    ///
    /// Separate because these four values are decided together from the
    /// window's width and how many background sessions there are, and the
    /// reconnect control's placement depends on both.
    pub(super) fn title_bar_session_controls(
        &self,
        inputs: TitleBarSessionInputs<'_>,
    ) -> TitleBarSessionControls {
        let TitleBarSessionInputs {
            window,
            compact_mode,
            background_sessions,
            right_window_controls,
            handle,
        } = inputs;
        let background_session_count = background_sessions.len();
        let show_title_bar_control_labels = title_bar_shows_control_labels(
            window.viewport_size().width,
            background_session_count > 0,
            self.launch_config.hide_title_bar_labels,
            compact_mode,
        );
        let show_title_bar_buttons =
            title_bar_buttons_visible(compact_mode, self.launch_config.hide_title_bar_buttons);
        let reconnect_control =
            (show_title_bar_buttons && background_session_count > 0).then(|| {
                self.render_reconnect_control(
                    show_title_bar_control_labels,
                    &background_sessions,
                    handle,
                )
            });
        let right_reconnect_control = title_bar_background_indicator_on_right(
            compact_mode,
            self.launch_config.hide_title_bar_buttons,
            background_session_count,
        )
        .then(|| {
            // This control is outside the regular controls row, so it must
            // block the draggable title-bar hitbox on platforms that use
            // native hit testing for client-side decorations.
            div()
                .occlude()
                .child(self.render_reconnect_control(false, &background_sessions, handle))
                .into_any_element()
        });

        // Keep a reconnect control immediately next to the native/client window controls. The
        // group owns the trailing auto-margin so the two controls cannot be separated by free
        // space when the title-bar controls are hidden or compact mode is enabled.
        let right_title_bar_controls = h_flex()
            .id("title-bar-right-controls")
            .h_full()
            .flex_none()
            .ml_auto()
            .when_some(right_reconnect_control, |controls, reconnect_control| {
                controls.child(reconnect_control)
            })
            .when(title_bar_app_label_visible(compact_mode), |controls| {
                controls.child(
                    div().flex_none().px_2().child(
                        Label::new("Zetta")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
            })
            .child(right_window_controls)
            .into_any_element();

        TitleBarSessionControls {
            show_title_bar_control_labels,
            show_title_bar_buttons,
            reconnect_control,
            right_title_bar_controls,
        }
    }

    /// The background-session reconnect control: a plain button for a single
    /// session, a menu of sessions when more than one can be reconnected.
    pub(super) fn render_reconnect_control(
        &self,
        show_label: bool,
        sessions: &Arc<[ProcessBackgroundSessionEntry]>,
        handle: &WeakEntity<Self>,
    ) -> AnyElement {
        let session_count = sessions.len();
        if session_count == 1 {
            return Button::new("reconnect-session", reconnect_control_label(show_label))
                .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                .style(ButtonStyle::Subtle)
                .size(ButtonSize::Large)
                .aria_label("Reconnect background session")
                .tooltip(Tooltip::for_action_title(
                    "Reconnect background session",
                    &ReconnectSession,
                ))
                .on_click(|_, window, cx| window.dispatch_action(Box::new(ReconnectSession), cx))
                .into_any_element();
        }

        let entries = sessions.to_vec();
        let menu_handle = handle.clone();
        let dismiss_handle = handle.clone();
        PopoverMenu::new("reconnect-session-menu")
            .with_handle(self.reconnect_menu_handle.clone())
            .trigger_with_tooltip(
                Button::new("reconnect-session", reconnect_control_label(show_label))
                    .start_icon(Icon::new(IconName::RotateCw).size(IconSize::Small))
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Large)
                    .aria_label("Choose background session to reconnect"),
                Tooltip::for_action_title(
                    format!("Choose background session to reconnect ({session_count})"),
                    &ReconnectSession,
                ),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let entries = entries.clone();
                let menu_handle = menu_handle.clone();
                let menu = ui::ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for (runner_id, session_id, title, details) in &entries {
                        let runner_id = *runner_id;
                        let session_id = *session_id;
                        let title = title.clone();
                        let details = details.clone();
                        let handle = menu_handle.clone();
                        menu = menu.custom_entry(
                            move |_, _| {
                                v_flex()
                                    .gap_0p5()
                                    .child(Label::new(title.clone()))
                                    .child(
                                        Label::new(details.clone())
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                    .into_any_element()
                            },
                            move |window, cx| {
                                handle
                                    .update(cx, |this, cx| {
                                        this.reconnect_background_session(
                                            runner_id, session_id, window, cx,
                                        );
                                    })
                                    .ok();
                            },
                        );
                    }
                    menu
                });
                // As the Profile and application menus do: dismissing the menu
                // with Esc used to leave focus nowhere.
                restore_focus_on_dismiss(&menu, dismiss_handle.clone(), window, cx);
                Some(menu)
            })
            .into_any_element()
    }

    /// Everything in the title bar that is not the window's own controls: the
    /// menus, the session buttons, and the broadcast and silent toggles.
    pub(super) fn render_title_bar_controls(&self, layout: TitleBarLayout) -> AnyElement {
        let TitleBarLayout {
            title_bar_background,
            compact_mode,
            is_macos_fullscreen,
            show_title_bar_menus,
            sidebar_toggle,
            application_menu,
            profile_menu,
            show_title_bar_buttons,
            show_title_bar_control_labels,
            no_mux,
            auto_background_tab,
            auto_background_protected,
            reconnect_control,
            show_broadcast_control,
            broadcast_input,
            show_silent_control,
            silent_mode,
            system_silent,
            focus_status_access,
            ..
        } = layout;
        let reserve_macos_title_bar_space =
            macos_title_bar_reservations_enabled(is_macos_fullscreen);
        let reserve_compact_leading_controls =
            compact_leading_controls_reservation_enabled(compact_mode, is_macos_fullscreen);
        let profile_menu = if compact_mode {
            render_compact_tab_neighbor_control(profile_menu, title_bar_background)
        } else {
            profile_menu
        };
        let broadcast_control = show_broadcast_control.then(|| {
            broadcast_input_control(
                broadcast_input,
                show_title_bar_control_labels,
                compact_mode,
                title_bar_background,
            )
        });
        let silent_control = show_silent_control.then(|| {
            silent_mode_control(SilentControl {
                silent_mode,
                system_silent,
                focus_status_access,
                show_title_bar_control_labels,
                compact_mode,
                title_bar_background,
            })
        });
        h_flex()
            .id("title-bar-controls")
            // Keep application controls out of the draggable native title-bar region.
            .occlude()
            .min_w_0()
            // Drop trailing controls before they can overlap the client corner.
            .flex_shrink_1()
            .overflow_hidden()
            .gap_1()
            // When the reserve below makes this wider than its content, fill it from
            // the tab-bar side inward, so any blank space lands next to the traffic
            // lights instead of between the controls and the tabs.
            .justify_end()
            // The traffic lights are native controls even with a client title bar.
            .when(reserve_macos_title_bar_space, |controls| {
                controls.ml(px(72.))
            })
            // Reserve a minimal, constant gap next to the traffic lights sized for
            // two Large buttons (e.g. the application and profile menu triggers) —
            // enough that tabs don't crowd the traffic lights when nothing else is
            // there, but a `min_w` rather than extra margin so that any menus or the
            // broadcast button which do render here expand into that reserve instead
            // of pushing further out.
            .when(reserve_compact_leading_controls, |controls| {
                controls.min_w(COMPACT_LEADING_CONTROLS_RESERVE)
            })
            .children(sidebar_toggle)
            .when(show_title_bar_menus, |controls| {
                controls.child(application_menu).child(profile_menu)
            })
            .when(
                title_bar_keep_running_visible(no_mux, show_title_bar_buttons),
                |controls| {
                    controls.child(
                        Button::new(
                            "auto-background-tab",
                            if show_title_bar_control_labels {
                                "Keep running"
                            } else {
                                ""
                            },
                        )
                        .start_icon(Icon::new(IconName::Pin).size(IconSize::Small))
                        .style(ButtonStyle::Subtle)
                        .size(ButtonSize::Large)
                        .toggle_state(auto_background_tab)
                        .aria_label("Keep this tab running after close")
                        .tooltip(Tooltip::for_action_title(
                            if auto_background_tab {
                                if auto_background_protected {
                                    "Keep running after close is on · authentication required"
                                } else {
                                    "Keep running after close is on · no authentication"
                                }
                            } else {
                                "Keep this tab running after the tab or window is closed"
                            },
                            &ToggleAutoBackgroundTab,
                        ))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ToggleAutoBackgroundTab), cx);
                        }),
                    )
                },
            )
            .when(show_title_bar_buttons, |controls| {
                controls.child(
                    Button::new(
                        "detach-tab",
                        if show_title_bar_control_labels {
                            "Detach"
                        } else {
                            ""
                        },
                    )
                    .start_icon(Icon::new(IconName::Archive).size(IconSize::Small))
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Large)
                    .aria_label("Detach tab")
                    .tooltip(Tooltip::for_action_title(
                        "Detach tab to background",
                        &DetachTab,
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(DetachTab), cx);
                    }),
                )
            })
            .when_some(reconnect_control, |controls, reconnect_control| {
                controls.child(reconnect_control)
            })
            .when_some(broadcast_control, |controls, broadcast_control| {
                controls.child(broadcast_control)
            })
            .when_some(silent_control, |controls, silent_control| {
                controls.child(silent_control)
            })
            .into_any_element()
    }
}
