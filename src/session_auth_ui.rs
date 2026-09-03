use super::*;
#[cfg(feature = "session-persistence")]
use crate::background_session_ui::DiskResumeIdentities;
use crate::background_session_ui::{AttachOutcomeSummary, ProtectedSessionAction};
use zeroize::{Zeroize as _, Zeroizing};

#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAuthenticationPromptMode {
    /// Choosing the secret for an action that widens a session's reach.
    ///
    /// One mode for detaching, keeping and sharing, because the prompt is the
    /// same in all three: a required, confirmed secret, and no way past it. What
    /// the action is decides only the wording and what happens afterwards, both
    /// of which [`ProtectedSessionAction`] answers.
    Protect {
        tab_id: u64,
        action: ProtectedSessionAction,
    },
    Reconnect {
        runner_id: u64,
        session_id: u64,
    },
    ResumeDisk {
        session_id: u64,
    },
    /// Asking for the passphrase that unlocks an encrypted *identity* file, so
    /// an automatically protected session's sealed key can be opened.
    ///
    /// Not a session secret: nobody chose the key this recovers, and there is
    /// nothing about the session to type. It exists because `age` otherwise
    /// falls back to reading the passphrase from a controlling terminal, which a
    /// window does not have — leaving the identity undecrypted and the failure
    /// reported as "No matching keys found".
    #[cfg(feature = "session-persistence")]
    UnlockSealedSession {
        runner_id: u64,
        session_id: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SessionAuthenticationField {
    Secret,
    Confirmation,
}

/// What the background verification task produced. Creating a verifier and
/// checking one against a typed secret yield different types now, so they
/// cannot be confused for one another when the result is matched below.
enum Outcome {
    Created(SessionAuthentication),
    Verified(Option<VerifiedSession>),
}

/// What a typed pair asks for.
///
/// One rule for every action that widens a session's reach: an empty pair leaves
/// the session unprotected, a confirmed pair protects it, and anything else is
/// half-typed. Sharing follows it too — a dialog that refused the empty pair
/// would be the odd one out, and the choice is the user's to make in all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionAuthenticationChoice {
    Unprotected,
    Protected,
    Incomplete,
}

fn session_authentication_choice(secret: &str, confirmation: &str) -> SessionAuthenticationChoice {
    match (
        secret.is_empty(),
        confirmation.is_empty(),
        secret == confirmation,
    ) {
        (true, true, _) => SessionAuthenticationChoice::Unprotected,
        (false, false, true) => SessionAuthenticationChoice::Protected,
        _ => SessionAuthenticationChoice::Incomplete,
    }
}

pub(crate) struct SessionAuthenticationPrompt {
    pub(crate) mode: SessionAuthenticationPromptMode,
    pub(crate) secret: TextField,
    pub(crate) confirmation: TextField,
    pub(crate) field: SessionAuthenticationField,
    pub(crate) error: Option<String>,
    pub(crate) working: bool,
    #[cfg(feature = "session-persistence")]
    pub(crate) disk_identities: Option<DiskResumeIdentities>,
    pub(crate) disk_identity_index: Option<usize>,
    pub(crate) disk_protected: bool,
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
            #[cfg(feature = "session-persistence")]
            disk_identities: None,
            disk_identity_index: None,
            disk_protected: false,
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
    /// Asks for the secret an action that widens a session's reach requires.
    pub(crate) fn prompt_for_session_secret(
        &mut self,
        tab_id: u64,
        action: ProtectedSessionAction,
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
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::Protect { tab_id, action },
            window,
            cx,
        );
    }

    pub(crate) fn prompt_to_reconnect_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::Reconnect {
                runner_id,
                session_id,
            },
            window,
            cx,
        );
    }

    #[cfg(feature = "session-persistence")]
    pub(crate) fn prompt_to_resume_disk_session(
        &mut self,
        session_id: u64,
        protected: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        let paths = self.disk_resume_identity_paths();
        let required_identity = paths.iter().enumerate().find_map(|(index, path)| {
            match zmux::persistence::identity_path_requires_passphrase(path) {
                Ok(true) => Some(Ok(index)),
                Ok(false) => None,
                Err(error) => Some(Err((path.clone(), error))),
            }
        });
        let identity_index = match required_identity {
            Some(Ok(index)) => Some(index),
            Some(Err((path, error))) => {
                self.show_notice(
                    format!(
                        "Could not inspect encrypted identity {}: {error:#}",
                        path.display()
                    ),
                    cx,
                );
                return ReconnectSessionResult::Rejected;
            }
            None => None,
        };
        if protected || identity_index.is_some() {
            let identities = identity_index.map(|_| DiskResumeIdentities {
                paths: paths.clone(),
                passphrases: vec![None; paths.len()],
            });
            self.open_session_authentication_prompt(
                SessionAuthenticationPromptMode::ResumeDisk { session_id },
                window,
                cx,
            );
            if let Some(prompt) = self.session_authentication.as_mut() {
                prompt.disk_identities = identities;
                prompt.disk_identity_index = identity_index;
                prompt.disk_protected = protected;
            }
            cx.notify();
            return ReconnectSessionResult::AuthenticationFailed;
        }
        let result = self.resume_disk_session(session_id, None, None, window, cx);
        if result == ReconnectSessionResult::Rejected
            && let Some(error) = self.pane_output_error.take()
        {
            self.show_notice(error, cx);
        }
        result
    }

    /// Asks for the passphrase of the encrypted identity file, so an
    /// automatically protected session's sealed key can be opened.
    ///
    /// The counterpart of the identity-passphrase half of
    /// [`Self::prompt_to_resume_disk_session`], which is where this behaviour
    /// already worked; a live session had no equivalent, so an encrypted identity
    /// failed with `age`'s own "No matching keys found".
    #[cfg(feature = "session-persistence")]
    pub(crate) fn prompt_to_unlock_sealed_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::UnlockSealedSession {
                runner_id,
                session_id,
            },
            window,
            cx,
        );
    }

    /// Without age there are no sealed sessions to unlock, so nothing reaches
    /// this; it degrades to the ordinary secret prompt rather than being absent
    /// and forcing every call site to spell the feature out.
    #[cfg(not(feature = "session-persistence"))]
    pub(crate) fn prompt_to_unlock_sealed_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_to_reconnect_session(runner_id, session_id, window, cx);
    }

    /// Retries the reconnect with the typed identity passphrase.
    ///
    /// Deliberately the same entry point the reconnect took the first time, so
    /// the multiplexer-held, other-process and own-process cases stay in one
    /// place rather than being re-implemented behind the prompt.
    #[cfg(feature = "session-persistence")]
    fn submit_sealed_session_passphrase(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self.session_authentication.as_mut() else {
            return;
        };
        let passphrase = Zeroizing::new(prompt.secret.text.clone());
        if passphrase.is_empty() {
            prompt.error = Some("Enter the identity passphrase.".into());
            cx.notify();
            return;
        }
        prompt.secret.text.zeroize();
        prompt.error = None;
        let passphrase = SessionSecret::from_zeroizing(passphrase);
        let result = self.reconnect_process_background_session(
            runner_id,
            session_id,
            Some(passphrase),
            window,
            cx,
        );
        if result == ReconnectSessionResult::Reconnected {
            self.session_authentication = None;
        } else if let Some(prompt) = self.session_authentication.as_mut() {
            prompt.working = false;
            prompt.secret = TextField::default();
            prompt.error = Some(
                self.pane_output_error
                    .take()
                    .unwrap_or_else(|| "Could not open the identity file.".to_owned()),
            );
        }
        cx.notify();
    }

    fn open_session_authentication_prompt(
        &mut self,
        mode: SessionAuthenticationPromptMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.session_authentication = Some(SessionAuthenticationPrompt::new(mode));
        self.session_authentication_focus.focus(window, cx);
        cx.notify();
    }

    /// Carries out the action with the session left unprotected, which is what
    /// an empty dialog asks for.
    pub(crate) fn continue_without_session_authentication(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(prompt) = self
            .session_authentication
            .as_ref()
            .filter(|prompt| !prompt.working)
        else {
            return;
        };
        let mode = prompt.mode;
        self.session_authentication = None;
        match mode {
            SessionAuthenticationPromptMode::Protect { tab_id, action } => {
                self.apply_protected_session_action(tab_id, action, None, window, cx)
            }
            SessionAuthenticationPromptMode::Reconnect { .. }
            | SessionAuthenticationPromptMode::ResumeDisk { .. } => {}
            // There is no "without": the identity file is encrypted and its
            // passphrase is the only way to read it.
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {}
        }
    }

    fn dismiss_session_authentication(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.session_authentication = None;
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn submit_session_authentication(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mode) = self
            .session_authentication
            .as_ref()
            .filter(|prompt| !prompt.working)
            .map(|prompt| prompt.mode)
        else {
            return;
        };
        // Checked before the prompt is borrowed mutably, and before the secret
        // is read, so a refused attempt never reaches the verifier.
        if let SessionAuthenticationPromptMode::Reconnect {
            runner_id,
            session_id,
        } = mode
            && self.process_authentication_is_refused(runner_id, session_id, cx)
        {
            if let Some(prompt) = self.session_authentication.as_mut() {
                prompt.error = Some("Too many failed attempts. Try again shortly.".into());
            }
            cx.notify();
            return;
        }
        #[cfg(feature = "session-persistence")]
        if let SessionAuthenticationPromptMode::UnlockSealedSession {
            runner_id,
            session_id,
        } = mode
        {
            self.submit_sealed_session_passphrase(runner_id, session_id, window, cx);
            return;
        }
        let Some(prompt) = self.session_authentication.as_mut() else {
            return;
        };
        let secret = Zeroizing::new(prompt.secret.text.clone());
        match mode {
            SessionAuthenticationPromptMode::Protect { .. } => {
                match session_authentication_choice(&secret, &prompt.confirmation.text) {
                    SessionAuthenticationChoice::Unprotected => {
                        self.continue_without_session_authentication(window, cx);
                        return;
                    }
                    SessionAuthenticationChoice::Protected => {}
                    SessionAuthenticationChoice::Incomplete => {
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
            SessionAuthenticationPromptMode::ResumeDisk { .. } => {
                if prompt.disk_protected && secret.is_empty() {
                    prompt.error = Some("Enter the session secret.".into());
                    cx.notify();
                    return;
                }
                if prompt.disk_identity_index.is_some()
                    && (if prompt.disk_protected {
                        prompt.confirmation.text.is_empty()
                    } else {
                        secret.is_empty()
                    })
                {
                    prompt.error = Some("Enter the identity passphrase.".into());
                    cx.notify();
                    return;
                }
            }
            SessionAuthenticationPromptMode::Reconnect { .. } => {}
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
                unreachable!("the identity passphrase is handled before the verifier")
            }
        }
        #[cfg(feature = "session-persistence")]
        if let SessionAuthenticationPromptMode::ResumeDisk { session_id } = mode {
            self.submit_disk_resume_authentication(session_id, window, cx);
            return;
        }
        prompt.secret.text.zeroize();
        prompt.confirmation.text.zeroize();
        prompt.working = true;
        prompt.error = None;
        // A session the multiplexer is holding keeps its verifier in the
        // multiplexer, not in any Zetta process, so there is nothing to check
        // here: the secret is handed to the daemon as part of the attach, and
        // the daemon evaluates it. Without this branch the local verifier
        // lookup returns nothing and a correct secret is reported as "no
        // longer available".
        if let SessionAuthenticationPromptMode::Reconnect { session_id, .. } = mode
            && self.multiplexer_holds_session(session_id)
        {
            match self.attach_multiplexer_session(
                session_id,
                Some(SessionSecret::from_zeroizing(secret)),
                window,
                cx,
            ) {
                Ok(AttachOutcomeSummary::Attached) => {
                    self.session_authentication = None;
                    cx.notify();
                }
                Ok(AttachOutcomeSummary::AuthenticationFailed)
                | Ok(AttachOutcomeSummary::AuthenticationRequired) => {
                    if let Some(prompt) = self.session_authentication.as_mut() {
                        prompt.working = false;
                        prompt.secret = TextField::default();
                        prompt.error = Some("Authentication failed.".into());
                    }
                    cx.notify();
                }
                Err(error) => {
                    if let Some(prompt) = self.session_authentication.as_mut() {
                        prompt.working = false;
                        prompt.error = Some(format!("{error:#}"));
                    }
                    cx.notify();
                }
            }
            return;
        }
        let generation = self.session_authentication_generation;
        let verifier = match mode {
            SessionAuthenticationPromptMode::Protect { .. } => None,
            SessionAuthenticationPromptMode::Reconnect {
                runner_id,
                session_id,
            } => self.process_background_session_authentication(runner_id, session_id, cx),
            SessionAuthenticationPromptMode::ResumeDisk { .. } => None,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
                unreachable!("the identity passphrase is handled before the verifier")
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    match mode {
                        SessionAuthenticationPromptMode::Protect { .. } => {
                            SessionAuthentication::create(&secret).map(Outcome::Created)
                        }
                        SessionAuthenticationPromptMode::Reconnect { .. } => verifier
                            .context("the protected session is no longer available")
                            .map(|verifier| Outcome::Verified(verifier.verify(&secret))),
                        SessionAuthenticationPromptMode::ResumeDisk { .. } => {
                            unreachable!("disk resume is handled before the background verifier")
                        }
                        #[cfg(feature = "session-persistence")]
                        SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
                            unreachable!("the identity passphrase is handled before the verifier")
                        }
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.session_authentication_generation != generation {
                    return;
                }
                match (mode, result) {
                    (
                        SessionAuthenticationPromptMode::Protect { tab_id, action },
                        Ok(Outcome::Created(authentication)),
                    ) => {
                        this.session_authentication = None;
                        this.apply_protected_session_action(
                            tab_id,
                            action,
                            Some(authentication),
                            window,
                            cx,
                        );
                    }
                    (
                        SessionAuthenticationPromptMode::Reconnect {
                            runner_id,
                            session_id,
                        },
                        Ok(Outcome::Verified(Some(authorization))),
                    ) => {
                        this.session_authentication = None;
                        this.process_clear_failed_authentications(runner_id, session_id, cx);
                        this.complete_authenticated_reconnect(
                            runner_id,
                            session_id,
                            &authorization,
                            window,
                            cx,
                        );
                    }
                    (
                        SessionAuthenticationPromptMode::Reconnect {
                            runner_id,
                            session_id,
                        },
                        Ok(Outcome::Verified(None)),
                    ) => {
                        this.process_record_failed_authentication(runner_id, session_id, cx);
                        if let Some(prompt) = this.session_authentication.as_mut() {
                            prompt.working = false;
                            prompt.secret = TextField::default();
                            prompt.error = Some("Authentication failed.".into());
                        }
                        cx.notify();
                    }
                    (SessionAuthenticationPromptMode::ResumeDisk { .. }, _) => {}
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

    #[cfg(feature = "session-persistence")]
    fn submit_disk_resume_authentication(
        &mut self,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (protected, identity_index, mut identities, session_secret, identity_passphrase) = {
            let Some(prompt) = self.session_authentication.as_mut() else {
                return;
            };
            let protected = prompt.disk_protected;
            let identity_index = prompt.disk_identity_index;
            let session_secret = protected
                .then(|| SessionSecret::from_zeroizing(Zeroizing::new(prompt.secret.text.clone())));
            let identity_passphrase = identity_index.map(|_| {
                let text = if protected {
                    prompt.confirmation.text.clone()
                } else {
                    prompt.secret.text.clone()
                };
                SessionSecret::from_zeroizing(Zeroizing::new(text))
            });
            let identities = prompt.disk_identities.clone();
            prompt.secret.text.zeroize();
            prompt.confirmation.text.zeroize();
            prompt.working = true;
            prompt.error = None;
            (
                protected,
                identity_index,
                identities,
                session_secret,
                identity_passphrase,
            )
        };

        if let Some(index) = identity_index {
            let Some(identity_passphrase) = identity_passphrase else {
                if let Some(prompt) = self.session_authentication.as_mut() {
                    prompt.working = false;
                    prompt.error = Some("The encrypted identity passphrase is unavailable.".into());
                }
                cx.notify();
                return;
            };
            let Some(identities) = identities.as_mut() else {
                if let Some(prompt) = self.session_authentication.as_mut() {
                    prompt.working = false;
                    prompt.error = Some("The configured identity is unavailable.".into());
                }
                cx.notify();
                return;
            };
            if index >= identities.passphrases.len() {
                identities
                    .passphrases
                    .resize_with(identities.paths.len(), || None);
            }
            if let Some(passphrase) = identities.passphrases.get_mut(index) {
                *passphrase = Some(identity_passphrase);
            }
        }
        let result = self.resume_disk_session(session_id, session_secret, identities, window, cx);
        if result == ReconnectSessionResult::Reconnected {
            self.session_authentication = None;
        } else {
            let error = self.pane_output_error.take().unwrap_or_else(|| {
                if protected {
                    "Authentication failed.".to_owned()
                } else {
                    "Could not decrypt the identity file.".to_owned()
                }
            });
            if let Some(prompt) = self.session_authentication.as_mut() {
                prompt.working = false;
                prompt.secret = TextField::default();
                prompt.confirmation = TextField::default();
                prompt.error = Some(error);
            }
        }
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
        let can_tab_between_fields = match prompt.mode {
            SessionAuthenticationPromptMode::Protect { .. } => true,
            SessionAuthenticationPromptMode::Reconnect { .. } => false,
            // One field: the identity's passphrase, and nothing else to type.
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => false,
            SessionAuthenticationPromptMode::ResumeDisk { .. } => {
                prompt.disk_protected && prompt.disk_identity_index.is_some()
            }
        };
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss_session_authentication(window, cx),
            "enter" => self.submit_session_authentication(window, cx),
            "tab" if can_tab_between_fields => {
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
        let colors = self.window_theme(cx).colors().clone();
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
        let action = match prompt.mode {
            SessionAuthenticationPromptMode::Protect { action, .. } => Some(action),
            SessionAuthenticationPromptMode::Reconnect { .. }
            | SessionAuthenticationPromptMode::ResumeDisk { .. } => None,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => None,
        };
        #[cfg(feature = "session-persistence")]
        let unlocking_sealed_session = matches!(
            prompt.mode,
            SessionAuthenticationPromptMode::UnlockSealedSession { .. }
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
        let submit_handle = handle.clone();
        let without_authentication_handle = handle.clone();
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
                        .text_color(colors.text)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            Label::new(match action {
                                Some(action) => action.title(self.no_mux),
                                None if unlocking_sealed_session => "Unlock your age identity",
                                None if matches!(
                                    prompt.mode,
                                    SessionAuthenticationPromptMode::ResumeDisk { .. }
                                ) =>
                                {
                                    "Restore encrypted disk session"
                                }
                                None => "Authenticate protected session",
                            })
                            .size(LabelSize::Large)
                            .color(Color::Custom(colors.text)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(colors.text_muted)
                                .child(match action {
                                    Some(action) => action.description(self.no_mux),
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
                                    None => {
                                        "Enter the secret chosen when this session was detached."
                                    }
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    Label::new(
                                        if unlocking_sealed_session
                                            || (disk_identity_required && !disk_protected)
                                        {
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
                                )),
                        )
                        .when(action.is_some() || disk_has_secondary_field, |panel| {
                            panel.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(Label::new(if disk_has_secondary_field {
                                        "Identity passphrase"
                                    } else {
                                        "Confirm secret"
                                    })
                                    .size(LabelSize::Small)
                                    .color(Color::Custom(colors.text)))
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
                                    .text_color(self.window_theme(cx).status().error)
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
                                        .color(Color::Custom(colors.text))
                                        .on_click(move |_, window, cx| {
                                            cancel_handle
                                                .update(cx, |this, cx| {
                                                    this.dismiss_session_authentication(window, cx)
                                                })
                                                .ok();
                                        }),
                                )
                                .when(action.is_some(), |buttons| {
                                    buttons.child(
                                        Button::new(
                                            "continue-without-session-authentication",
                                            "No authentication",
                                        )
                                        .style(ButtonStyle::Outlined)
                                        .color(Color::Custom(colors.text))
                                        .disabled(prompt.working)
                                        .on_click(
                                            move |_, window, cx| {
                                                without_authentication_handle
                                                .update(cx, |this, cx| {
                                                    this.continue_without_session_authentication(
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
                                        match action {
                                            Some(action) => action.submit_label(self.no_mux),
                                            None if unlocking_sealed_session => "Unlock",
                                            None => "Authenticate",
                                        },
                                    )
                                    .style(ButtonStyle::Filled)
                                    .color(Color::Custom(colors.text))
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
