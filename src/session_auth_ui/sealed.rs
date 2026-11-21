//! Unlocking a sealed session: asking for the identity passphrase that opens
//! its key envelope, locally or on a remote host, and the automatic unlock
//! that is tried before the prompt is shown.

use super::*;

impl Zetta {
    /// Opens an automatically protected remote session with the same age
    /// identity used for local background sessions. The remote daemon only
    /// ever receives the recovered session secret; the identity and its
    /// private key stay in this process.
    #[cfg(feature = "session-persistence")]
    pub(crate) fn prompt_to_unlock_remote_session(
        &mut self,
        target: zmux::remote::RemoteTarget,
        session_id: u64,
        envelope: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(auto_protect) = self.auto_protect.clone() else {
            self.show_notice(
                "This remote session is protected by an age identity. Configure the matching identity before attaching.",
                cx,
            );
            self.focus_active(window, cx);
            return;
        };
        match auto_protect.identity_passphrase_required() {
            Ok(true) => {
                self.remote_session_target = Some(target);
                self.remote_session_key_envelope = Some(envelope);
                self.open_session_authentication_prompt(
                    SessionAuthenticationPromptMode::UnlockRemoteSealedSession { session_id },
                    window,
                    cx,
                );
            }
            Ok(false) => {
                self.unlock_remote_sealed_session(
                    target,
                    session_id,
                    envelope,
                    auto_protect,
                    None,
                    window,
                    cx,
                );
            }
            Err(error) => {
                self.show_error_notice(
                    format!("Could not inspect your age identity: {error:#}"),
                    cx,
                );
                self.focus_active(window, cx);
            }
        }
    }

    #[cfg(feature = "session-persistence")]
    #[expect(
        clippy::too_many_arguments,
        reason = "five owned values the unlock consumes, plus the GPUI window \
                  and context"
    )]
    pub(super) fn unlock_remote_sealed_session(
        &mut self,
        target: zmux::remote::RemoteTarget,
        session_id: u64,
        envelope: String,
        auto_protect: std::sync::Arc<crate::session_auto_protect::SessionAutoProtect>,
        passphrase: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = self.session_authentication_generation;
        let transport = self.remote_session_transport;
        let operation_generation = self.next_remote_session_operation_generation();
        if let Some(prompt) = self.session_authentication.as_mut() {
            prompt.working = true;
            prompt.error = None;
            prompt.secret.text.zeroize();
            prompt.secret = TextField::default();
        }
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let secret = auto_protect.open_for_remote(&envelope, passphrase, &target)?;
                    load_remote_attach(target, session_id, Some(secret), transport)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.session_authentication_generation != generation
                    || !this.remote_session_operation_is_current(operation_generation)
                {
                    return;
                }
                match result {
                    Ok(RemoteAttachOutcome::Attached(data)) => {
                        match this.finish_remote_multiplexer_session(*data, window, cx) {
                            Ok(AttachOutcomeSummary::Attached) => {
                                this.session_authentication = None;
                                this.remote_session_target = None;
                                this.remote_session_key_envelope = None;
                                this.focus_active(window, cx);
                            }
                            Ok(AttachOutcomeSummary::AuthenticationRequired)
                            | Ok(AttachOutcomeSummary::AuthenticationFailed) => {
                                this.remote_auto_unlock_failed(
                                    "The remote session rejected its automatically recovered secret.",
                                    cx,
                                );
                            }
                            Err(error) => this.remote_auto_unlock_failed(
                                &format!("Could not attach the remote session: {error:#}"),
                                cx,
                            ),
                        }
                    }
                    Ok(RemoteAttachOutcome::AuthenticationRequired)
                    | Ok(RemoteAttachOutcome::AuthenticationFailed) => {
                        this.remote_auto_unlock_failed(
                            "The remote session rejected its automatically recovered secret.",
                            cx,
                        );
                    }
                    Err(error) => this.remote_auto_unlock_failed(
                        &format!(
                            "Could not open the remote session with your age identity: {error:#}"
                        ),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    #[cfg(feature = "session-persistence")]
    pub(super) fn remote_auto_unlock_failed(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(prompt) = self.session_authentication.as_mut() {
            prompt.working = false;
            prompt.secret = TextField::default();
            prompt.error = Some(message.to_owned());
        } else {
            self.remote_session_target = None;
            self.remote_session_key_envelope = None;
            self.show_error_notice(message, cx);
        }
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
    pub(super) fn submit_sealed_session_passphrase(
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
            if prompt.error.is_none() {
                prompt.error = Some("Could not open the identity file.".to_owned());
            }
        }
        cx.notify();
    }

    #[cfg(feature = "session-persistence")]
    pub(super) fn submit_remote_sealed_session_passphrase(
        &mut self,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (target, envelope, auto_protect, passphrase) = {
            let target = self.remote_session_target.clone();
            let envelope = self.remote_session_key_envelope.clone();
            let auto_protect = self.auto_protect.clone();
            let Some(prompt) = self.session_authentication.as_mut() else {
                return;
            };
            let passphrase = Zeroizing::new(prompt.secret.text.clone());
            if passphrase.is_empty() {
                prompt.error = Some("Enter the identity passphrase.".into());
                cx.notify();
                return;
            }
            let Some(target) = target else {
                prompt.error = Some("The remote SSH target is no longer available.".into());
                cx.notify();
                return;
            };
            let Some(envelope) = envelope else {
                prompt.error = Some("The remote session key is no longer available.".into());
                cx.notify();
                return;
            };
            let Some(auto_protect) = auto_protect else {
                prompt.error = Some("Automatic session protection is not configured.".into());
                cx.notify();
                return;
            };
            prompt.secret.text.zeroize();
            prompt.error = None;
            (
                target,
                envelope,
                auto_protect,
                SessionSecret::from_zeroizing(passphrase),
            )
        };
        self.unlock_remote_sealed_session(
            target,
            session_id,
            envelope,
            auto_protect,
            Some(passphrase),
            window,
            cx,
        );
    }
}
