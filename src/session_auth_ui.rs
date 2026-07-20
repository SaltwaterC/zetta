use super::*;
use zeroize::{Zeroize as _, Zeroizing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAuthenticationPromptMode {
    Detach { tab_id: u64 },
    Reconnect { runner_id: u64, session_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAuthenticationField {
    Secret,
    Confirmation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetachAuthenticationChoice {
    Unprotected,
    Protected,
    Incomplete,
}

fn detach_authentication_choice(secret: &str, confirmation: &str) -> DetachAuthenticationChoice {
    match (
        secret.is_empty(),
        confirmation.is_empty(),
        secret == confirmation,
    ) {
        (true, true, _) => DetachAuthenticationChoice::Unprotected,
        (false, false, true) => DetachAuthenticationChoice::Protected,
        _ => DetachAuthenticationChoice::Incomplete,
    }
}

pub(crate) struct SessionAuthenticationPrompt {
    pub(crate) mode: SessionAuthenticationPromptMode,
    pub(crate) secret: TextField,
    pub(crate) confirmation: TextField,
    pub(crate) field: SessionAuthenticationField,
    pub(crate) error: Option<String>,
    pub(crate) working: bool,
}

impl SessionAuthenticationPrompt {
    fn new(mode: SessionAuthenticationPromptMode) -> Self {
        Self {
            mode,
            secret: TextField::default(),
            confirmation: TextField::default(),
            field: SessionAuthenticationField::Secret,
            error: None,
            working: false,
        }
    }
}

impl Drop for SessionAuthenticationPrompt {
    fn drop(&mut self) {
        self.secret.text.zeroize();
        self.confirmation.text.zeroize();
    }
}

impl Zetta {
    pub(crate) fn prompt_to_detach_session(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.command_palette = None;
        self.multi_command = None;
        self.tab_search = None;
        self.settings_editor = None;
        self.serial_console = None;
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.session_authentication = Some(SessionAuthenticationPrompt::new(
            SessionAuthenticationPromptMode::Detach { tab_id },
        ));
        self.session_authentication_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn prompt_to_reconnect_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.session_authentication = Some(SessionAuthenticationPrompt::new(
            SessionAuthenticationPromptMode::Reconnect {
                runner_id,
                session_id,
            },
        ));
        self.session_authentication_focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss_session_authentication(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_authentication = None;
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn detach_without_authentication(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(SessionAuthenticationPrompt {
            mode: SessionAuthenticationPromptMode::Detach { tab_id },
            working: false,
            ..
        }) = self.session_authentication.as_ref()
        else {
            return;
        };
        let tab_id = *tab_id;
        self.session_authentication = None;
        self.detach_tab_by_id(tab_id, None, window, cx);
    }

    pub(crate) fn submit_session_authentication(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.session_authentication.as_mut() else {
            return;
        };
        if prompt.working {
            return;
        }
        let mode = prompt.mode;
        let secret = Zeroizing::new(prompt.secret.text.clone());
        match mode {
            SessionAuthenticationPromptMode::Detach { .. } => {
                match detach_authentication_choice(&secret, &prompt.confirmation.text) {
                    DetachAuthenticationChoice::Unprotected => {
                        self.detach_without_authentication(window, cx);
                        return;
                    }
                    DetachAuthenticationChoice::Protected => {}
                    DetachAuthenticationChoice::Incomplete => {
                        prompt.error = Some("Enter the same secret in both fields.".into());
                        cx.notify();
                        return;
                    }
                }
            }
            SessionAuthenticationPromptMode::Reconnect { .. } if secret.is_empty() => {
                prompt.error = Some("Enter the session secret.".into());
                cx.notify();
                return;
            }
            SessionAuthenticationPromptMode::Reconnect { .. } => {}
        }
        prompt.secret.text.zeroize();
        prompt.confirmation.text.zeroize();
        prompt.working = true;
        prompt.error = None;
        let generation = self.session_authentication_generation;
        let verifier = match mode {
            SessionAuthenticationPromptMode::Detach { .. } => None,
            SessionAuthenticationPromptMode::Reconnect {
                runner_id,
                session_id,
            } => self.process_background_session_authentication(runner_id, session_id, cx),
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match mode {
                        SessionAuthenticationPromptMode::Detach { .. } => {
                            SessionAuthentication::create(&secret).map(Some)
                        }
                        SessionAuthenticationPromptMode::Reconnect { .. } => verifier
                            .context("the protected session is no longer available")
                            .map(|verifier| verifier.verify(&secret).then_some(verifier)),
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.session_authentication_generation != generation {
                    return;
                }
                match (mode, result) {
                    (
                        SessionAuthenticationPromptMode::Detach { tab_id },
                        Ok(Some(authentication)),
                    ) => {
                        this.session_authentication = None;
                        this.detach_tab_by_id(tab_id, Some(authentication), window, cx);
                    }
                    (
                        SessionAuthenticationPromptMode::Reconnect {
                            runner_id,
                            session_id,
                        },
                        Ok(Some(authentication)),
                    ) => {
                        this.session_authentication = None;
                        this.complete_authenticated_reconnect(
                            runner_id,
                            session_id,
                            &authentication,
                            window,
                            cx,
                        );
                    }
                    (SessionAuthenticationPromptMode::Reconnect { .. }, Ok(None)) => {
                        if let Some(prompt) = this.session_authentication.as_mut() {
                            prompt.working = false;
                            prompt.secret = TextField::default();
                            prompt.error = Some("Authentication failed.".into());
                        }
                        cx.notify();
                    }
                    (_, Err(error)) => {
                        if let Some(prompt) = this.session_authentication.as_mut() {
                            prompt.working = false;
                            prompt.error = Some(format!("{error:#}"));
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn session_authentication_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = self.session_authentication.as_mut() else {
            return false;
        };
        if prompt.working {
            if event.keystroke.key == "escape" {
                self.dismiss_session_authentication(window, cx);
            }
            return true;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_session_authentication(window, cx),
            "enter" => self.submit_session_authentication(window, cx),
            "tab" if matches!(prompt.mode, SessionAuthenticationPromptMode::Detach { .. }) => {
                prompt.field = match prompt.field {
                    SessionAuthenticationField::Secret => SessionAuthenticationField::Confirmation,
                    SessionAuthenticationField::Confirmation => SessionAuthenticationField::Secret,
                };
                cx.notify();
            }
            key => {
                let field = match prompt.field {
                    SessionAuthenticationField::Secret => &mut prompt.secret,
                    SessionAuthenticationField::Confirmation => &mut prompt.confirmation,
                };
                let command =
                    event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
                match key {
                    "backspace" => field.backspace(),
                    "delete" => field.delete(),
                    "left" => field.move_left(),
                    "right" => field.move_right(),
                    "home" => {
                        field.cursor = 0;
                        field.select_all = false;
                    }
                    "end" => {
                        field.cursor = field.text.len();
                        field.select_all = false;
                    }
                    "a" if command => field.select_all(),
                    "v" if command => {
                        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                            field.insert(&text);
                        }
                    }
                    _ if !command && !event.keystroke.modifiers.alt => {
                        if let Some(text) = event.keystroke.key_char.as_ref() {
                            field.insert(text);
                        }
                    }
                    _ => {}
                }
                prompt.error = None;
                cx.notify();
            }
        }
        true
    }

    pub(crate) fn render_session_authentication_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let prompt = self.session_authentication.as_ref()?;
        let colors = cx.theme().colors().clone();
        let handle = cx.entity().downgrade();
        let field = |id: &'static str,
                     value: &TextField,
                     selected: SessionAuthenticationField|
         -> gpui::AnyElement {
            let focused = prompt.field == selected;
            let cursor = value.cursor.min(value.text.len());
            let (before, after) = value.text.split_at(cursor);
            let click_handle = handle.clone();
            div()
                .id(id)
                .h_9()
                .w_full()
                .px_2()
                .flex()
                .items_center()
                .rounded(px(4.))
                .border_1()
                .border_color(if focused {
                    colors.border_focused
                } else {
                    colors.border
                })
                .bg(colors.editor_background)
                .cursor_text()
                .when(value.select_all && focused, |input| {
                    input.bg(colors.element_selection_background)
                })
                .child(
                    div()
                        .whitespace_nowrap()
                        .child("•".repeat(before.chars().count())),
                )
                .when(focused && !value.select_all, |input| {
                    input.child(
                        div()
                            .flex_none()
                            .w(px(1.))
                            .h(px(16.))
                            .bg(colors.text_accent),
                    )
                })
                .child(
                    div()
                        .whitespace_nowrap()
                        .child("•".repeat(after.chars().count())),
                )
                .on_click(move |_, _, cx| {
                    click_handle
                        .update(cx, |this, cx| {
                            if let Some(prompt) = this.session_authentication.as_mut() {
                                prompt.field = selected;
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .into_any_element()
        };
        let reconnect = matches!(
            prompt.mode,
            SessionAuthenticationPromptMode::Reconnect { .. }
        );
        let submit_handle = handle.clone();
        let detach_handle = handle.clone();
        let cancel_handle = handle.clone();
        Some(
            div()
                .id("session-authentication-overlay")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(transparent_black().opacity(0.24))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .track_focus(&self.session_authentication_focus)
                .child(
                    div()
                        .w(px(480.))
                        .max_w(gpui::relative(0.9))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.elevated_surface_background)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Label::new(if reconnect {
                                "Authenticate protected session"
                            } else {
                                "Detach session"
                            })
                            .size(LabelSize::Large),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child(if reconnect {
                                    "Enter the secret chosen when this session was detached."
                                } else {
                                    "Press Enter with both fields empty for no authentication, or enter and confirm a secret."
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(Label::new("Secret").size(LabelSize::Small))
                                .child(field(
                                    "session-authentication-secret",
                                    &prompt.secret,
                                    SessionAuthenticationField::Secret,
                                )),
                        )
                        .when(!reconnect, |panel| {
                            panel.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(Label::new("Confirm secret").size(LabelSize::Small))
                                    .child(field(
                                        "session-authentication-confirmation",
                                        &prompt.confirmation,
                                        SessionAuthenticationField::Confirmation,
                                    )),
                            )
                        })
                        .when_some(prompt.error.as_ref(), |panel, error| {
                            panel.child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().status().error)
                                    .child(error.clone()),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("cancel-session-authentication", "Cancel")
                                        .style(ButtonStyle::Outlined)
                                        .on_click(move |_, window, cx| {
                                            cancel_handle
                                                .update(cx, |this, cx| {
                                                    this.dismiss_session_authentication(window, cx)
                                                })
                                                .ok();
                                        }),
                                )
                                .when(!reconnect, |buttons| {
                                    buttons.child(
                                        Button::new(
                                            "detach-without-authentication",
                                            "No authentication",
                                        )
                                        .style(ButtonStyle::Outlined)
                                        .disabled(prompt.working)
                                        .on_click(
                                            move |_, window, cx| {
                                                detach_handle
                                                    .update(cx, |this, cx| {
                                                        this.detach_without_authentication(
                                                            window, cx,
                                                        )
                                                    })
                                                    .ok();
                                            },
                                        ),
                                    )
                                })
                                .child(
                                    Button::new(
                                        "submit-session-authentication",
                                        if reconnect {
                                            "Authenticate"
                                        } else {
                                            "Protect and detach"
                                        },
                                    )
                                    .style(ButtonStyle::Filled)
                                    .loading(prompt.working)
                                    .disabled(prompt.working)
                                    .on_click(
                                        move |_, window, cx| {
                                            submit_handle
                                                .update(cx, |this, cx| {
                                                    this.submit_session_authentication(window, cx)
                                                })
                                                .ok();
                                        },
                                    ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
#[path = "tests/session_auth_ui.rs"]
mod tests;
