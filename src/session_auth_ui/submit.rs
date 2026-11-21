//! Submitting the session prompt: validating what was typed, verifying it
//! off the render path, and applying the outcome for a multiplexed, remote or
//! on-disk session.

use super::*;

/// Creates or checks the session secret. Runs on a background thread, because
/// the key derivation behind it is deliberately slow.
///
/// Only `Protect` and `Reconnect` reach here: every other mode is settled before
/// the local verifier is consulted, which is what the arms say.
pub(super) fn verify_session_secret(
    mode: SessionAuthenticationPromptMode,
    secret: &Zeroizing<String>,
    verifier: Option<SessionAuthentication>,
) -> anyhow::Result<Outcome> {
    match mode {
        SessionAuthenticationPromptMode::Protect { .. } => {
            SessionAuthentication::create(secret).map(Outcome::Created)
        }
        #[cfg(feature = "zmux")]
        SessionAuthenticationPromptMode::RemoteCreate => {
            SessionAuthentication::create(secret).map(|authentication| Outcome::CreatedRemote {
                authentication,
                secret: SessionSecret::new(secret.to_string()),
            })
        }
        SessionAuthenticationPromptMode::Reconnect { .. } => verifier
            .context("the protected session is no longer available")
            .map(|verifier| Outcome::Verified(verifier.verify(secret))),
        SessionAuthenticationPromptMode::RemoteAttach { .. } => {
            unreachable!("remote attach is handled before the local verifier")
        }
        SessionAuthenticationPromptMode::ResumeDisk { .. } => {
            unreachable!("disk resume is handled before the background verifier")
        }
        #[cfg(feature = "session-persistence")]
        SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
            unreachable!("the identity passphrase is handled before the verifier")
        }
        #[cfg(feature = "session-persistence")]
        SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => {
            unreachable!("the remote identity passphrase is handled before the verifier")
        }
    }
}

impl Zetta {
    /// Submits whatever the prompt is asking for.
    ///
    /// The order is load-bearing and each step says why: the modes that never
    /// reach the local verifier are settled first, then the fields are
    /// validated, and only then is the secret taken out of the prompt and
    /// zeroized. A refused attempt therefore never reads the secret at all.
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
        #[cfg(feature = "session-persistence")]
        if let SessionAuthenticationPromptMode::UnlockRemoteSealedSession { session_id } = mode {
            self.submit_remote_sealed_session_passphrase(session_id, window, cx);
            return;
        }
        if !self.validate_session_authentication(mode, window, cx) {
            return;
        }
        #[cfg(feature = "session-persistence")]
        if let SessionAuthenticationPromptMode::ResumeDisk { session_id } = mode {
            self.submit_disk_resume_authentication(session_id, window, cx);
            return;
        }
        let Some(prompt) = self.session_authentication.as_mut() else {
            return;
        };
        let secret = Zeroizing::new(prompt.secret.text.clone());
        prompt.secret.text.zeroize();
        prompt.confirmation.text.zeroize();
        prompt.working = true;
        prompt.error = None;
        if self.submit_multiplexed_authentication(mode, &secret, window, cx) {
            return;
        }
        let generation = self.session_authentication_generation;
        let verifier = match mode {
            SessionAuthenticationPromptMode::Protect { .. } => None,
            SessionAuthenticationPromptMode::Reconnect {
                runner_id,
                session_id,
            } => self.process_background_session_authentication(runner_id, session_id, cx),
            SessionAuthenticationPromptMode::RemoteAttach { .. } => {
                unreachable!("remote attach is handled before the local verifier")
            }
            SessionAuthenticationPromptMode::ResumeDisk { .. } => None,
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
                unreachable!("the identity passphrase is handled before the verifier")
            }
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => {
                unreachable!("the remote identity passphrase is handled before the verifier")
            }
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => None,
        };
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { verify_session_secret(mode, &secret, verifier) })
                .await;
            this.update_in(cx, |this, window, cx| {
                // A newer prompt has replaced this one, so its answer is stale.
                if this.session_authentication_generation != generation {
                    return;
                }
                this.apply_session_authentication_outcome(mode, result, window, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Checks the prompt's fields against what `mode` needs, reporting the first
    /// problem on the prompt itself.
    ///
    /// Returns `false` once it has reported one, or once a `Protect` prompt was
    /// left blank — which means "do not protect this session" rather than an
    /// error, and is acted on here.
    pub(super) fn validate_session_authentication(
        &mut self,
        mode: SessionAuthenticationPromptMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(prompt) = self.session_authentication.as_mut() else {
            return false;
        };
        let secret = Zeroizing::new(prompt.secret.text.clone());
        let optional_secret = match mode {
            SessionAuthenticationPromptMode::Protect { .. } => true,
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => true,
            _ => false,
        };
        if optional_secret {
            match session_authentication_choice(&secret, &prompt.confirmation.text) {
                SessionAuthenticationChoice::Unprotected => {
                    self.continue_without_session_authentication(window, cx);
                    return false;
                }
                SessionAuthenticationChoice::Protected => {}
                SessionAuthenticationChoice::Incomplete => {
                    prompt.error = Some("Enter the same secret in both fields.".into());
                    cx.notify();
                    return false;
                }
            }
        }
        match mode {
            SessionAuthenticationPromptMode::Protect { .. } => {}
            SessionAuthenticationPromptMode::Reconnect { .. }
            | SessionAuthenticationPromptMode::RemoteAttach { .. }
                if secret.is_empty() =>
            {
                prompt.error = Some("Enter the session secret.".into());
                cx.notify();
                return false;
            }
            SessionAuthenticationPromptMode::ResumeDisk { .. } => {
                if prompt.disk_protected && secret.is_empty() {
                    prompt.error = Some("Enter the session secret.".into());
                    cx.notify();
                    return false;
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
                    return false;
                }
            }
            SessionAuthenticationPromptMode::Reconnect { .. } => {}
            SessionAuthenticationPromptMode::RemoteAttach { .. } => {
                unreachable!("remote attach is handled before the local verifier")
            }
            #[cfg(feature = "zmux")]
            SessionAuthenticationPromptMode::RemoteCreate => {}
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockSealedSession { .. } => {
                unreachable!("the identity passphrase is handled before the verifier")
            }
            #[cfg(feature = "session-persistence")]
            SessionAuthenticationPromptMode::UnlockRemoteSealedSession { .. } => {
                unreachable!("the remote identity passphrase is handled before the verifier")
            }
        }
        true
    }

    /// The two modes whose secret the multiplexer evaluates rather than a
    /// verifier in this process.
    ///
    /// A session the multiplexer is holding keeps its verifier in the
    /// multiplexer, not in any Zetta process, so there is nothing to check
    /// locally: the secret is handed to the daemon as part of the attach and the
    /// daemon evaluates it. Without this the local verifier lookup returns
    /// nothing and a correct secret is reported as "no longer available".
    ///
    /// Returns `true` once it has handled the submission.
    pub(super) fn submit_multiplexed_authentication(
        &mut self,
        mode: SessionAuthenticationPromptMode,
        secret: &Zeroizing<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        #[cfg(not(feature = "zmux"))]
        {
            let _ = (mode, secret, window, cx);
            false
        }
        #[cfg(feature = "zmux")]
        {
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
                    Some(SessionSecret::from_zeroizing(secret.clone())),
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
                return true;
            }
            if let SessionAuthenticationPromptMode::RemoteAttach { session_id } = mode {
                let Some(target) = self.remote_session_target.clone() else {
                    if let Some(prompt) = self.session_authentication.as_mut() {
                        prompt.working = false;
                        prompt.error = Some("The remote SSH target is no longer available.".into());
                    }
                    cx.notify();
                    return true;
                };
                let generation = self.session_authentication_generation;
                let transport = self.remote_session_transport;
                let operation_generation = self.next_remote_session_operation_generation();
                let secret = SessionSecret::from_zeroizing(secret.clone());
                cx.spawn_in(window, async move |this, cx| {
                    let result = cx
                        .background_spawn(async move {
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
                                        this.focus_active(window, cx);
                                    }
                                    Ok(AttachOutcomeSummary::AuthenticationRequired)
                                    | Ok(AttachOutcomeSummary::AuthenticationFailed) => {
                                        this.remote_attach_authentication_failed(
                                            "Authentication failed.",
                                            cx,
                                        );
                                    }
                                    Err(error) => this.remote_attach_authentication_failed(
                                        &format!("{error:#}"),
                                        cx,
                                    ),
                                }
                            }
                            Ok(RemoteAttachOutcome::AuthenticationRequired)
                            | Ok(RemoteAttachOutcome::AuthenticationFailed) => {
                                this.remote_attach_authentication_failed(
                                    "Authentication failed.",
                                    cx,
                                );
                            }
                            Err(error) => {
                                this.remote_attach_authentication_failed(&format!("{error:#}"), cx);
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
                cx.notify();
                return true;
            }
            false
        }
    }

    #[cfg(feature = "zmux")]
    pub(super) fn remote_attach_authentication_failed(
        &mut self,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(prompt) = self.session_authentication.as_mut() {
            prompt.working = false;
            prompt.secret = TextField::default();
            prompt.error = Some(message.to_owned());
        }
        cx.notify();
    }

    /// What the verified secret does to the prompt and the session behind it.
    pub(super) fn apply_session_authentication_outcome(
        &mut self,
        mode: SessionAuthenticationPromptMode,
        result: anyhow::Result<Outcome>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let this = self;
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
            #[cfg(feature = "zmux")]
            (
                SessionAuthenticationPromptMode::RemoteCreate,
                Ok(Outcome::CreatedRemote {
                    authentication,
                    secret,
                }),
            ) => {
                this.session_authentication = None;
                this.start_remote_session_create(Some(authentication), Some(secret), window, cx);
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
    }

    #[cfg(feature = "session-persistence")]
    pub(super) fn submit_disk_resume_authentication(
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
            if let Some(prompt) = self.session_authentication.as_mut() {
                prompt.working = false;
                prompt.secret = TextField::default();
                prompt.confirmation = TextField::default();
                if prompt.error.is_none() {
                    prompt.error = Some(if protected {
                        "Authentication failed.".to_owned()
                    } else {
                        "Could not decrypt the identity file.".to_owned()
                    });
                }
            }
        }
        cx.notify();
    }
}
