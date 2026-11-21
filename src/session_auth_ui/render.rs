//! Drawing the session prompt. One builder per section, each reading the
//! shared `SessionPromptView` bundle.

use super::*;

/// What every part of the session prompt reads: the prompt itself, the theme and
/// handle its controls are built from, and the mode predicates the builders
/// below share.
#[derive(Clone, Copy)]
pub(super) struct SessionPromptView<'a> {
    pub(super) prompt: &'a SessionAuthenticationPrompt,
    pub(super) colors: &'a ThemeColors,
    pub(super) handle: &'a WeakEntity<Zetta>,
    pub(super) no_mux: bool,
    /// Taken from the theme's status colours rather than `colors`, which is why
    /// it travels separately.
    pub(super) error_color: gpui::Hsla,
    pub(super) action: Option<ProtectedSessionAction>,
    pub(super) remote_create: bool,
    pub(super) unlocking_sealed_session: bool,
    pub(super) disk_identity_required: bool,
    pub(super) disk_protected: bool,
    /// A disk resume can need both a session secret and an identity passphrase.
    pub(super) disk_has_secondary_field: bool,
}

impl SessionPromptView<'_> {
    /// One masked field.
    pub(super) fn field(
        &self,
        id: &'static str,
        value: &TextField,
        selected: SessionAuthenticationField,
    ) -> gpui::AnyElement {
        let prompt = self.prompt;
        let colors = self.colors;
        let handle = self.handle;
        let focused = prompt.field == selected;
        let click_handle = handle.clone();
        boxed_text_field(id, value, focused, None, FieldMask::Secret, colors)
            .w_full()
            .cursor_text()
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
    }
}

/// The prompt's title, which names what the secret is for.
pub(super) fn session_prompt_heading(view: SessionPromptView<'_>) -> impl IntoElement {
    let SessionPromptView {
        prompt,
        colors,
        no_mux,
        action,
        remote_create,
        unlocking_sealed_session,
        ..
    } = view;
    dialog_title(
        match action {
            Some(action) => action.title(no_mux),
            None if unlocking_sealed_session => "Unlock your age identity",
            None if matches!(
                prompt.mode,
                SessionAuthenticationPromptMode::ResumeDisk { .. }
            ) =>
            {
                "Restore encrypted disk session"
            }
            None if remote_create => "Create remote session",
            None => "Authenticate protected session",
        },
        colors,
    )
}

/// The line under the title explaining what submitting will do.
pub(super) fn session_prompt_description(view: SessionPromptView<'_>) -> impl IntoElement {
    let SessionPromptView {
        prompt,
        colors,
        no_mux,
        action,
        remote_create,
        unlocking_sealed_session,
        disk_identity_required,
        disk_has_secondary_field,
        ..
    } = view;
    div()
        .text_sm()
        .text_color(colors.text_muted)
        .child(match action {
            Some(action) => action.description(no_mux),
            None if unlocking_sealed_session => {
                "This session is protected by your age key. Enter the \
                 passphrase for your identity file to open it; the \
                 session's own key was generated and is never typed."
            }
            None if matches!(
                prompt.mode,
                SessionAuthenticationPromptMode::ResumeDisk { .. }
            ) =>
            {
                if disk_has_secondary_field {
                    "Enter the session secret and the identity passphrase."
                } else if disk_identity_required {
                    "Enter the passphrase for the encrypted identity file."
                } else {
                    "Enter the session secret after decrypting the disk record."
                }
            }
            None if matches!(
                prompt.mode,
                SessionAuthenticationPromptMode::RemoteAttach { .. }
            ) =>
            {
                "Enter the secret chosen when this remote session was shared."
            }
            None if remote_create => {
                "Choose an optional secret for this remote session. Leave both fields empty to create it without protection."
            }
            None => "Enter the secret chosen when this session was detached.",
        })
}

/// The secret field and its label.
pub(super) fn session_prompt_secret_field(view: SessionPromptView<'_>) -> impl IntoElement {
    let SessionPromptView {
        prompt,
        colors,
        unlocking_sealed_session,
        disk_identity_required,
        disk_protected,
        ..
    } = view;
    let field = |id, value, selected| view.field(id, value, selected);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            Label::new(
                if unlocking_sealed_session || (disk_identity_required && !disk_protected) {
                    "Identity passphrase"
                } else {
                    "Session secret"
                },
            )
            .size(LabelSize::Small)
            .color(Color::Custom(colors.text)),
        )
        .child(field(
            "session-authentication-secret",
            &prompt.secret,
            SessionAuthenticationField::Secret,
        ))
}

/// The identity passphrase or confirmation field, where the mode needs a second
/// one, and the error the last attempt reported.
pub(super) fn session_prompt_secondary_fields(view: SessionPromptView<'_>) -> impl IntoElement {
    let SessionPromptView {
        prompt,
        colors,
        error_color,
        action,
        remote_create,
        disk_has_secondary_field,
        ..
    } = view;
    let field = |id, value, selected| view.field(id, value, selected);
    div()
        .when(
            action.is_some() || remote_create || disk_has_secondary_field,
            |panel| {
                panel.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            Label::new(if disk_has_secondary_field {
                                "Identity passphrase"
                            } else {
                                "Confirm secret"
                            })
                            .size(LabelSize::Small)
                            .color(Color::Custom(colors.text)),
                        )
                        .child(field(
                            "session-authentication-confirmation",
                            &prompt.confirmation,
                            SessionAuthenticationField::Confirmation,
                        )),
                )
            },
        )
        .when_some(prompt.error.as_ref(), |panel, error| {
            panel.child(crate::ui_messages::error_message(
                error.clone(),
                error_color,
            ))
        })
}

/// Cancel, Submit, and — for a `Protect` prompt — the option to keep the
/// session unprotected.
pub(super) fn session_prompt_buttons(view: SessionPromptView<'_>) -> impl IntoElement {
    let SessionPromptView {
        prompt,
        colors,
        handle,
        no_mux,
        action,
        remote_create,
        unlocking_sealed_session,
        ..
    } = view;
    let submit_handle = handle.clone();
    let without_authentication_handle = handle.clone();
    let cancel_handle = handle.clone();
    dialog_buttons()
        .child(
            DialogButton::new(
                "cancel-session-authentication",
                "Cancel",
                ButtonRole::Secondary,
            )
            .key_tooltip("Cancel", SurfaceKey::Escape)
            .render(colors, move |_, window, cx| {
                cancel_handle
                    .update(cx, |this, cx| {
                        this.dismiss_session_authentication(window, cx);
                    })
                    .ok();
            }),
        )
        .when(action.is_some() || remote_create, |buttons| {
            buttons.child(
                DialogButton::new(
                    "continue-without-session-authentication",
                    "No authentication",
                    ButtonRole::Secondary,
                )
                .enabled(!prompt.working)
                .render(colors, move |_, window, cx| {
                    without_authentication_handle
                        .update(cx, |this, cx| {
                            this.continue_without_session_authentication(window, cx);
                        })
                        .ok();
                }),
            )
        })
        .child({
            let submit_label = match action {
                Some(action) => action.submit_label(no_mux),
                None if remote_create => "Create",
                None if unlocking_sealed_session => "Unlock",
                None => "Authenticate",
            };
            DialogButton::new(
                "submit-session-authentication",
                submit_label,
                ButtonRole::Primary,
            )
            .key_tooltip(submit_label, SurfaceKey::Enter)
            .loading(prompt.working)
            .render(colors, move |_, window, cx| {
                submit_handle
                    .update(cx, |this, cx| {
                        this.submit_session_authentication(window, cx);
                    })
                    .ok();
            })
        })
}

impl Zetta {
    /// The session passphrase prompt.
    ///
    /// Split into the builders below — heading, description, fields, buttons —
    /// because what each shows depends on the prompt's mode, and the mode
    /// predicates they share travel in [`SessionPromptView`].
    pub(crate) fn render_session_authentication_overlay(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let prompt = self.session_authentication.as_ref()?;
        let colors = self.window_theme(cx).colors().clone();
        let handle = cx.entity().downgrade();
        let action = match prompt.mode {
            SessionAuthenticationPromptMode::Protect { action, .. } => Some(action),
            SessionAuthenticationPromptMode::Reconnect { .. }
            | SessionAuthenticationPromptMode::RemoteAttach { .. }
            | SessionAuthenticationPromptMode::ResumeDisk { .. } => None,
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => None,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => None,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => None,
        };
        #[cfg(feature = "session-persistence")]
        let unlocking_sealed_session = matches!(
            prompt.mode,
            SessionAuthenticationPromptMode::UnlockSealedSession { .. }
                | SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. }
        );
        #[cfg(not(feature = "session-persistence"))]
        let unlocking_sealed_session = false;
        #[cfg(feature = "session-persistence")]
        let disk_identity_required = matches!(
            prompt.mode,
            SessionAuthenticationPromptMode::ResumeDisk { .. }
        ) && prompt.disk_identity_index.is_some();
        #[cfg(not(feature = "session-persistence"))]
        let disk_identity_required = false;
        #[cfg(feature = "session-persistence")]
        let disk_protected = prompt.disk_protected;
        #[cfg(not(feature = "session-persistence"))]
        let disk_protected = false;
        let disk_has_secondary_field = disk_identity_required && disk_protected;
        #[cfg(feature = "zmux")]
        let remote_create = matches!(prompt.mode, SessionAuthenticationPromptMode::RemoteCreate);
        #[cfg(not(feature = "zmux"))]
        let remote_create = false;
        let view = SessionPromptView {
            prompt,
            colors: &colors,
            handle: &handle,
            no_mux: self.no_mux,
            error_color: self.window_theme(cx).status().error,
            action,
            remote_create,
            unlocking_sealed_session,
            disk_identity_required,
            disk_protected,
            disk_has_secondary_field,
        };
        let panel = dialog_panel("session-authentication", DIALOG_WIDTH_SMALL, &colors)
            .child(session_prompt_heading(view))
            .child(session_prompt_description(view))
            .child(session_prompt_secret_field(view))
            .child(session_prompt_secondary_fields(view))
            .child(session_prompt_buttons(view));
        // A typed secret must survive a stray click outside the prompt.
        Some(modal(
            modal_backdrop(
                "session-authentication-overlay",
                Placement::Centered,
                BackdropClick::Swallow,
            )
            .track_focus(&self.session_authentication_focus),
            panel,
        ))
    }
}
