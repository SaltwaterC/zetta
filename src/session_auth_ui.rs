//! The session-secret prompt: asking for the secret that protects, reconnects
//! to, attaches or resumes a session, and what each answer does.

use super::*;
#[cfg(feature = "session-persistence")]
use crate::background_session_ui::DiskResumeIdentities;
use crate::background_session_ui::ProtectedSessionAction;
#[cfg(feature = "zmux")]
use crate::background_session_ui::{AttachOutcomeSummary, RemoteAttachOutcome, load_remote_attach};
use zeroize::{Zeroize as _, Zeroizing};

mod render;
mod sealed;
mod submit;

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
    RemoteAttach {
        session_id: u64,
    },
    /// Choosing an optional secret for a headless remote session before the
    /// daemon creates it.
    #[cfg(feature = "zmux")]
    RemoteCreate,
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
    /// Asking for the passphrase of the local age identity that opens a
    /// remotely published automatic-session key envelope.
    #[cfg(feature = "session-persistence")]
    UnlockRemoteSealedSession {
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
    #[cfg(feature = "zmux")]
    CreatedRemote {
        authentication: SessionAuthentication,
        secret: SessionSecret,
    },
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

    #[cfg(feature = "zmux")]
    pub(crate) fn prompt_to_attach_remote_session(
        &mut self,
        target: zmux::remote::RemoteTarget,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remote_session_target = Some(target);
        self.open_session_authentication_prompt(
            SessionAuthenticationPromptMode::RemoteAttach { session_id },
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
                self.show_error_notice(
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
        self.resume_disk_session(session_id, None, None, window, cx)
    }

    pub(crate) fn open_session_authentication_prompt(
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

    /// Keeps an operation failure in the authentication prompt when one is
    /// active; an attach or resume attempted from elsewhere is a floating error
    /// notice. This prevents an internal prompt error from briefly appearing in
    /// the window's persistent feedback column before the prompt consumes it.
    pub(crate) fn show_session_operation_error(
        &mut self,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let message = message.into();
        if let Some(prompt) = self.session_authentication.as_mut() {
            prompt.error = Some(message);
            cx.notify();
        } else {
            self.show_error_notice(message, cx);
        }
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
                self.apply_protected_session_action(tab_id, action, None, window, cx);
            }
            SessionAuthenticationPromptMode::Reconnect { .. }
            | SessionAuthenticationPromptMode::RemoteAttach { .. }
            | SessionAuthenticationPromptMode::ResumeDisk { .. } => {}
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => {
                self.start_remote_session_create(None, None, window, cx);
            }
            // There is no "without": the identity file is encrypted and its
            // passphrase is the only way to read it.
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {}
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => {}
        }
    }

    fn dismiss_session_authentication(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let remote_attach = matches!(
            self.session_authentication
                .as_ref()
                .map(|prompt| prompt.mode),
            Some(SessionAuthenticationPromptMode::RemoteAttach { .. }),
        );
        #[cfg(feature = "zmux")]
        let remote_attach = remote_attach
            || matches!(
                self.session_authentication
                    .as_ref()
                    .map(|prompt| prompt.mode),
                Some(SessionAuthenticationPromptMode::RemoteCreate),
            );
        #[cfg(feature = "session-persistence")]
        let remote_attach = remote_attach
            || matches!(
                self.session_authentication
                    .as_ref()
                    .map(|prompt| prompt.mode),
                Some(SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. }),
            );
        if remote_attach {
            #[cfg(feature = "zmux")]
            {
                self.next_remote_session_operation_generation();
                self.remote_session_create = None;
            }
            self.remote_session_target = None;
            #[cfg(feature = "session-persistence")]
            {
                self.remote_session_key_envelope = None;
            }
        }
        self.session_authentication = None;
        self.session_authentication_generation =
            self.session_authentication_generation.wrapping_add(1);
        self.focus_active(window, cx);
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
            SessionAuthenticationPromptMode::RemoteAttach { .. } => false,
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => true,
            // One field: the identity's passphrase, and nothing else to type.
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => false,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => false,
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
                match key {
                    // Paste only, and handled here rather than through the shared
                    // clipboard shortcuts: a masked secret must not be copyable
                    // or cuttable out of the field it was typed into.
                    _ if crate::text_edit::is_paste_chord(&event.keystroke) => {
                        if let Some(text) = crate::overlay_clipboard::resolved_text(cx) {
                            field.insert(&text);
                        }
                    }
                    _ => {
                        apply_text_field_key(field, &event.keystroke);
                    }
                }
                prompt.error = None;
                cx.notify();
            }
        }
        true
    }
}

#[cfg(test)]
#[path = "tests/session_auth_ui.rs"]
mod tests;
