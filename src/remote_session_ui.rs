//! The remote-session picker.
//!
//! SSH discovery is deliberately kept out of the render path. The picker only
//! reads the local SSH config when it opens, and does the endpoint/list request
//! on GPUI's background executor after the user submits a target. The forwarded
//! mux connection is then handed to the ordinary shared-pane attach path.

use super::*;
use crate::background_session_ui::AttachOutcomeSummary;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteSessionField {
    Target,
    Port,
    List,
}

pub(crate) struct RemoteSessionPicker {
    pub(crate) target: TextField,
    pub(crate) port: TextField,
    pub(crate) field: RemoteSessionField,
    pub(crate) sessions: Vec<zmux::protocol::BackgroundSessionSummary>,
    pub(crate) selected: usize,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    pub(crate) suggestions: Vec<String>,
    pub(crate) scroll: UniformListScrollHandle,
    pub(crate) generation: u64,
    pub(crate) task: Option<Task<()>>,
}

impl Default for RemoteSessionPicker {
    fn default() -> Self {
        Self {
            target: TextField::default(),
            port: TextField::default(),
            field: RemoteSessionField::Target,
            sessions: Vec::new(),
            selected: 0,
            loading: false,
            error: None,
            suggestions: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            generation: 0,
            task: None,
        }
    }
}

#[cfg(test)]
#[path = "tests/remote_session_ui.rs"]
mod tests;

impl RemoteSessionPicker {
    fn invalidate_results(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.task.take();
        self.sessions.clear();
        self.selected = 0;
        self.loading = false;
        self.error = None;
    }
}

impl Zetta {
    pub(crate) fn open_remote_session(
        &mut self,
        _: &OpenRemoteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        #[cfg(feature = "serial-console")]
        {
            self.serial_console = None;
        }
        let picker = RemoteSessionPicker {
            suggestions: crate::multi_command::ssh_config_host_suggestions(),
            ..Default::default()
        };
        self.remote_session_picker = Some(picker);
        #[cfg(feature = "session-persistence")]
        {
            self.remote_session_key_envelope = None;
        }
        self.remote_session_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_remote_session_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remote_session_picker = None;
        self.remote_session_target = None;
        #[cfg(feature = "session-persistence")]
        {
            self.remote_session_key_envelope = None;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    fn remote_target_from_picker(
        picker: &RemoteSessionPicker,
    ) -> anyhow::Result<zmux::remote::RemoteTarget> {
        let destination = picker.target.text.trim();
        anyhow::ensure!(!destination.is_empty(), "enter an SSH target");
        let port =
            if picker.port.text.trim().is_empty() {
                None
            } else {
                let port =
                    picker.port.text.trim().parse::<u16>().map_err(|_| {
                        anyhow::anyhow!("SSH port must be a number from 1 to 65535")
                    })?;
                anyhow::ensure!(port != 0, "SSH port must be between 1 and 65535");
                Some(port)
            };
        let target = zmux::remote::RemoteTarget::new(destination).with_port(port);
        target.validate()?;
        Ok(target)
    }

    pub(crate) fn load_remote_sessions(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return;
        };
        let target = match Self::remote_target_from_picker(picker) {
            Ok(target) => target,
            Err(error) => {
                picker.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        picker.generation = picker.generation.wrapping_add(1);
        picker.task.take();
        picker.sessions.clear();
        picker.selected = 0;
        picker.loading = true;
        picker.error = None;
        let generation = picker.generation;
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let client = zmux::client::Client::connect_remote(target)
                        .context("connecting to the remote multiplexer")?;
                    client.list().context("listing remote sessions")
                })
                .await;
            this.update(cx, |this, cx| {
                let Some(picker) = this.remote_session_picker.as_mut() else {
                    return;
                };
                if picker.generation != generation {
                    return;
                }
                picker.loading = false;
                picker.task = None;
                match result {
                    Ok(sessions) => {
                        picker.sessions = sessions;
                        picker.selected = 0;
                        if picker.sessions.is_empty() {
                            picker.error = Some("The remote host has no shared sessions.".into());
                        }
                    }
                    Err(error) => picker.error = Some(format!("{error:#}")),
                }
                cx.notify();
            })
            .ok();
        });
        picker.task = Some(task);
        cx.notify();
    }

    fn select_remote_session(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picker) = self.remote_session_picker.as_ref() else {
            return;
        };
        let Some(summary) = picker.sessions.get(index).cloned() else {
            return;
        };
        let target = match Self::remote_target_from_picker(picker) {
            Ok(target) => target,
            Err(error) => {
                if let Some(picker) = self.remote_session_picker.as_mut() {
                    picker.error = Some(format!("{error:#}"));
                }
                cx.notify();
                return;
            }
        };
        self.remote_session_picker = None;
        if summary.authentication_required {
            #[cfg(feature = "session-persistence")]
            if let Some(envelope) = summary.key_envelope.clone() {
                self.prompt_to_unlock_remote_session(target, summary.id, envelope, window, cx);
            } else {
                self.prompt_to_attach_remote_session(target, summary.id, window, cx);
            }
            #[cfg(not(feature = "session-persistence"))]
            self.prompt_to_attach_remote_session(target, summary.id, window, cx);
            return;
        }
        match self.attach_remote_multiplexer_session(target, summary.id, None, window, cx) {
            Ok(AttachOutcomeSummary::Attached) => {}
            Ok(AttachOutcomeSummary::AuthenticationRequired)
            | Ok(AttachOutcomeSummary::AuthenticationFailed) => self.show_notice(
                format!("Remote session {} requires authentication.", summary.id),
                cx,
            ),
            Err(error) => self.show_notice(
                format!("Could not attach remote session {}: {error:#}", summary.id),
                cx,
            ),
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn remote_session_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.remote_session_picker.is_none() {
            return false;
        }
        if event.keystroke.key == "escape" {
            self.dismiss_remote_session_picker(window, cx);
            cx.stop_propagation();
            return true;
        }
        if event.keystroke.key == "enter" {
            let action = self.remote_session_picker.as_ref().map(|picker| {
                (
                    picker.field,
                    picker.loading,
                    picker.selected,
                    !picker.sessions.is_empty(),
                )
            });
            if let Some((field, loading, selected, has_sessions)) = action {
                if field == RemoteSessionField::List && has_sessions {
                    self.select_remote_session(selected, window, cx);
                } else if !loading {
                    self.load_remote_sessions(cx);
                }
            }
            cx.stop_propagation();
            return true;
        }
        let Some(picker) = self.remote_session_picker.as_mut() else {
            return false;
        };
        match event.keystroke.key.as_str() {
            "tab" => {
                picker.field = match (picker.field, event.keystroke.modifiers.shift) {
                    (RemoteSessionField::Target, false) => RemoteSessionField::Port,
                    (RemoteSessionField::Port, false) => RemoteSessionField::List,
                    (RemoteSessionField::List, false) => RemoteSessionField::Target,
                    (RemoteSessionField::Target, true) => RemoteSessionField::List,
                    (RemoteSessionField::Port, true) => RemoteSessionField::Target,
                    (RemoteSessionField::List, true) => RemoteSessionField::Port,
                };
                cx.notify();
            }
            "up" | "down"
                if picker.field == RemoteSessionField::List && !picker.sessions.is_empty() =>
            {
                if event.keystroke.key == "up" {
                    picker.selected = picker.selected.saturating_sub(1);
                } else {
                    picker.selected = (picker.selected + 1).min(picker.sessions.len() - 1);
                }
                picker
                    .scroll
                    .scroll_to_item(picker.selected, ScrollStrategy::Nearest);
                cx.notify();
            }
            "down"
                if picker.field == RemoteSessionField::Target && picker.target.text.is_empty() =>
            {
                if let Some(suggestion) = picker.suggestions.first().cloned() {
                    picker.target = TextField::new(suggestion);
                    picker.field = RemoteSessionField::Port;
                    cx.notify();
                }
            }
            _ if matches!(
                picker.field,
                RemoteSessionField::Target | RemoteSessionField::Port
            ) =>
            {
                let field = match picker.field {
                    RemoteSessionField::Target => &mut picker.target,
                    RemoteSessionField::Port => &mut picker.port,
                    RemoteSessionField::List => unreachable!(),
                };
                match apply_clipboard_shortcut(field, &event.keystroke, cx) {
                    ClipboardOutcome::Unchanged => {
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Edited => {
                        picker.invalidate_results();
                        cx.notify();
                        cx.stop_propagation();
                        return true;
                    }
                    ClipboardOutcome::Ignored => {}
                }
                // The port field takes digits only, so a character that is not
                // one is dropped before the field sees it; everything else is
                // the shared editing behaviour.
                let typed_a_rejected_character = picker.field == RemoteSessionField::Port
                    && event
                        .keystroke
                        .key_char
                        .as_ref()
                        .is_some_and(|text| !text.chars().all(|c| c.is_ascii_digit()));
                if !typed_a_rejected_character {
                    apply_text_field_key(field, &event.keystroke);
                }
                picker.invalidate_results();
                cx.notify();
            }
            _ => {}
        }
        cx.stop_propagation();
        true
    }

    pub(crate) fn render_remote_session_overlay(
        &self,
        colors: &ThemeColors,
        error_color: Hsla,
        handle: &WeakEntity<Self>,
    ) -> Option<AnyElement> {
        let picker = self.remote_session_picker.as_ref()?;
        let target = picker.target.clone();
        let port = picker.port.clone();
        let field = picker.field;
        let error = picker.error.clone();
        let loading = picker.loading;
        let session_count = picker.sessions.len();
        let selected = picker.selected.min(session_count.saturating_sub(1));
        let sessions = picker.sessions.clone();
        let suggestions = picker
            .suggestions
            .iter()
            .filter(|suggestion| {
                target.text.is_empty()
                    || suggestion
                        .to_lowercase()
                        .starts_with(&target.text.to_lowercase())
            })
            .take(6)
            .cloned()
            .collect::<Vec<_>>();
        let picker_scroll = picker.scroll.clone();
        let rows = remote_session_rows(handle, colors, &picker_scroll, sessions, selected);

        let target_handle = handle.clone();
        let port_handle = handle.clone();
        let cancel_handle = handle.clone();
        let load_handle = handle.clone();
        let attach_handle = handle.clone();
        let has_suggestions = !suggestions.is_empty();
        let suggestion_rows = remote_session_suggestion_rows(handle, colors, suggestions);

        let session_list = div()
            .id("remote-session-list-panel")
            .w_full()
            .rounded(px(4.))
            .border_1()
            .border_color(if field == RemoteSessionField::List {
                colors.border_focused
            } else {
                transparent_black()
            })
            .when(session_count > 0, |panel| panel.child(rows))
            .when(session_count == 0 && !loading && error.is_none(), |panel| {
                panel.child(
                    div()
                        .h_16()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(colors.text_muted)
                        .child("Enter a target and press Enter to load sessions."),
                )
            })
            .when_some(error, |panel, error| {
                panel.child(div().p_2().text_sm().text_color(error_color).child(error))
            });

        let field_widget = |id: &'static str,
                            value: TextField,
                            selected_field: RemoteSessionField,
                            placeholder: &'static str,
                            click_handle: WeakEntity<Self>|
         -> AnyElement {
            let focused = field == selected_field;
            let (before, after) = value.split_at_cursor();
            field_box(id, focused, colors)
                .flex_1()
                .min_w_0()
                .cursor_text()
                .when(value.select_all && focused, |input| {
                    input.bg(colors.element_selection_background)
                })
                .when(focused && !value.select_all, |input| {
                    input
                        .child(div().whitespace_nowrap().child(before.to_owned()))
                        .child(caret(colors))
                        .child(div().whitespace_nowrap().child(after.to_owned()))
                })
                .when(focused && value.select_all, |input| {
                    input.child(div().whitespace_nowrap().child(value.text.clone()))
                })
                .when(!focused, |input| {
                    input.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if value.text.is_empty() {
                                colors.text_placeholder
                            } else {
                                colors.text
                            })
                            .child(if value.text.is_empty() {
                                placeholder.to_owned()
                            } else {
                                value.text.clone()
                            }),
                    )
                })
                .on_click(move |_, _, cx| {
                    click_handle
                        .update(cx, |this, cx| {
                            if let Some(picker) = this.remote_session_picker.as_mut() {
                                picker.field = selected_field;
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .into_any_element()
        };

        Some(
            div()
                .id("remote-session-backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .id("remote-session-picker")
                        .track_focus(&self.remote_session_focus)
                        .w_full()
                        .max_w(px(680.))
                        .max_h(gpui::relative(0.9))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(Label::new("Open remote session").size(LabelSize::Large))
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child("Connect through your normal OpenSSH configuration. Remote sessions must be shared.")
                        )
                        .child(
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
                                .child(
                                    div()
                                        .flex_none()
                                        .w(px(120.))
                                        .child(field_widget(
                                            "remote-session-port",
                                            port,
                                            RemoteSessionField::Port,
                                            "Port",
                                            port_handle,
                                        )),
                                ),
                        )
                        .when(
                            field == RemoteSessionField::Target && has_suggestions,
                            |panel| {
                                panel.child(
                                    div()
                                        .children(suggestion_rows),
                                )
                            },
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(Label::new("Shared sessions").size(LabelSize::Small))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.text_muted)
                                        .child(if loading {
                                            "Loading…".to_owned()
                                        } else {
                                            format!("{session_count} session{}", if session_count == 1 { "" } else { "s" })
                                        }),
                                ),
                        )
                        .child(session_list)
                        .child(
                            div()
                                .flex()
                                .justify_between()
                                .items_center()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(colors.text_muted)
                                        .child("Tab next · ↑↓ choose · Enter load/attach · Esc cancel"),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("cancel-remote-session", "Cancel")
                                                .style(ButtonStyle::Outlined)
                                                .color(Color::Custom(colors.text))
                                                .on_click(move |_, window, cx| {
                                                    cancel_handle
                                                        .update(cx, |this, cx| {
                                                            this.dismiss_remote_session_picker(window, cx)
                                                        })
                                                        .ok();
                                                }),
                                        )
                                        .child(
                                            Button::new(
                                                "load-remote-sessions",
                                                if loading { "Loading…" } else { "Load" },
                                            )
                                            .style(ButtonStyle::Outlined)
                                            .color(Color::Custom(colors.text))
                                            .disabled(loading)
                                            .on_click(move |_, _, cx| {
                                                load_handle
                                                    .update(cx, |this, cx| {
                                                        this.load_remote_sessions(cx)
                                                    })
                                                    .ok();
                                            }),
                                        )
                                        .child(
                                            Button::new("attach-remote-session", "Attach")
                                                .style(ButtonStyle::Filled)
                                                .color(Color::Custom(colors.text))
                                                .disabled(loading || session_count == 0)
                                                .on_click(move |_, window, cx| {
                                                    attach_handle
                                                        .update(cx, |this, cx| {
                                                            this.select_remote_session(selected, window, cx)
                                                        })
                                                        .ok();
                                                }),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// The session list: one row per session the remote host is holding.
///
/// Virtualized, because a host that has been up for a while can be holding
/// more sessions than the panel shows at once.
fn remote_session_rows(
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
                                    this.select_remote_session(index, window, cx)
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
    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
}

/// The host suggestions under the target field, from the user's SSH
/// configuration.
fn remote_session_suggestion_rows(
    handle: &WeakEntity<Zetta>,
    colors: &ThemeColors,
    suggestions: Vec<String>,
) -> Vec<gpui::Stateful<gpui::Div>> {
    let handle = handle.clone();
    let colors = colors.clone();
    suggestions
        .into_iter()
        .enumerate()
        .map(|(index, suggestion)| {
            let suggestion_handle = handle.clone();
            let suggestion_label = suggestion.clone();
            div()
                .id(("remote-session-suggestion", index))
                .h_7()
                .px_2()
                .flex()
                .items_center()
                .cursor_pointer()
                .text_xs()
                .text_color(colors.text_muted)
                .hover(|style| style.bg(colors.element_hover))
                .on_click(move |_, _, cx| {
                    suggestion_handle
                        .update(cx, |this, cx| {
                            if let Some(picker) = this.remote_session_picker.as_mut() {
                                picker.target = TextField::new(suggestion.clone());
                                picker.field = RemoteSessionField::Port;
                                picker.invalidate_results();
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .child(suggestion_label)
        })
        .collect()
}
