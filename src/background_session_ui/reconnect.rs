//! Taking a stored session back into a window, and the authentication a
//! protected one has to pass first.
//!
//! Every entry point ends in one of `restore.rs`'s attach functions once the
//! session is in hand; what differs between them is where the session comes
//! from (this process, another process over the control socket, the
//! multiplexer, or a disk session) and what it has to prove to be released.

use super::*;

impl Zetta {
    pub(crate) fn reconnect_background_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reconnect_process_background_session(runner_id, session_id, None, window, cx);
    }

    #[cfg(feature = "session-persistence")]
    pub(crate) fn disk_resume_identity_paths(&self) -> Vec<PathBuf> {
        self.launch_config
            .sessions
            .persistence
            .resolved_identity()
            .into_iter()
            .collect()
    }

    pub(crate) fn reconnect_process_background_session(
        &mut self,
        runner_id: u64,
        session_id: u64,
        identity_passphrase: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        // A session the multiplexer is holding takes precedence: it is the one
        // that survived this process, so it is what the user is asking for.
        if self.multiplexer_holds_session(session_id) {
            // Except when this window is the one showing it. A shared session is
            // listed while it is still on screen, and the multiplexer hands a
            // pane straight back to the process that already holds it — so
            // attaching here would open a second tab reading the same pty, and
            // the two would split its output between them. Joining a shared
            // session is something *another* Zetta process does.
            if self.mux_panes.holds_session(session_id) {
                // Guidance, not a failure to act on: said once and taken away
                // again, rather than left on screen as an error the user cannot
                // clear.
                self.show_notice(
                    "This window is already showing that session. Attach it from another Zetta \
                     window to share it.",
                    cx,
                );
                return ReconnectSessionResult::Rejected;
            }
            // A session protected with the user's age key publishes the sealed
            // key beside it, so this window opens that instead of asking for a
            // secret nobody chose. Recovered before the attach rather than after
            // an `AuthenticationRequired`, which would cost a second round trip
            // to learn what the catalog already said.
            let secret = match self.recovered_session_secret(session_id, identity_passphrase) {
                Ok(SealedKeyRecovery::Recovered(secret)) => Some(secret),
                Ok(SealedKeyRecovery::NotSealed) => None,
                Ok(SealedKeyRecovery::NeedsIdentityPassphrase) => {
                    // Answerable, so it is asked rather than reported: the
                    // identity file is encrypted and only its passphrase is
                    // missing. The session's own key is still never typed.
                    self.prompt_to_unlock_sealed_session(runner_id, session_id, window, cx);
                    return ReconnectSessionResult::AuthenticationFailed;
                }
                Err(error) => {
                    self.pane_output_error = Some(format!(
                        "Could not open that session with your age identity: {error:#}"
                    ));
                    cx.notify();
                    return ReconnectSessionResult::Rejected;
                }
            };
            return match self.attach_multiplexer_session(session_id, secret, window, cx) {
                Ok(AttachOutcomeSummary::Attached) => ReconnectSessionResult::Reconnected,
                Ok(AttachOutcomeSummary::AuthenticationRequired)
                | Ok(AttachOutcomeSummary::AuthenticationFailed) => {
                    self.prompt_to_reconnect_session(runner_id, session_id, window, cx);
                    ReconnectSessionResult::AuthenticationFailed
                }
                Err(error) => {
                    self.pane_output_error =
                        Some(format!("Could not attach that session: {error:#}"));
                    cx.notify();
                    ReconnectSessionResult::Rejected
                }
            };
        }
        #[cfg(feature = "session-persistence")]
        if runner_id == crate::background_sessions::RESTORABLE_RUNNER_ID {
            let record = zmux::persistence::read_opaque_records(
                &crate::background_sessions::session_catalog_dir(),
            )
            .ok()
            .and_then(|records| records.into_iter().find(|record| record.id == session_id));
            // An automatically protected record needs no secret asked for: its
            // key is sealed inside the very ciphertext the resume is about to
            // open, and the client recovers it there with the identity that
            // decrypted the record. The flag is public because this decision has
            // to be made before anything is decrypted.
            let protected = record.is_some_and(|record| record.protected && !record.auto_protected);
            return self.prompt_to_resume_disk_session(session_id, protected, window, cx);
        }
        if runner_id != self.background_sessions.runner_id() {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return ReconnectSessionResult::SessionNotFound;
            };
            if !source
                .read(cx)
                .background_session_is_transferable(session_id)
            {
                self.pane_output_error = Some(
                    "That background session is still starting. Try attaching it again shortly."
                        .to_owned(),
                );
                cx.notify();
                return ReconnectSessionResult::StillStarting;
            }
            let verifier = source
                .read(cx)
                .background_session_authentication(session_id);
            if let Some(verifier) = verifier.as_ref() {
                // Opened here when the holder protected it with the user's age
                // key, which this window has too; otherwise it is a secret only a
                // person knows, so the dialog is the only way through.
                match self.authorization_from_sealed_key(verifier, identity_passphrase, cx) {
                    SealedKeyAuthorization::Authorized(authorization) => {
                        return self.complete_authenticated_reconnect(
                            runner_id,
                            session_id,
                            &authorization,
                            window,
                            cx,
                        );
                    }
                    SealedKeyAuthorization::NeedsIdentityPassphrase => {
                        self.prompt_to_unlock_sealed_session(runner_id, session_id, window, cx);
                        return ReconnectSessionResult::AuthenticationFailed;
                    }
                    SealedKeyAuthorization::NotSealed => {}
                }
                self.prompt_to_reconnect_session(runner_id, session_id, window, cx);
                return ReconnectSessionResult::AuthenticationFailed;
            }
            let tab = source.update(cx, |source, cx| {
                source.take_background_session_by_id(session_id, None, cx)
            });
            if let Some(tab) = tab {
                prune_empty_dormant_runners(cx);
                self.attach_reconnected_tab(tab, true, window, cx);
                return ReconnectSessionResult::Reconnected;
            }
            return ReconnectSessionResult::SessionNotFound;
        }
        let Some(index) = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)
        else {
            return ReconnectSessionResult::SessionNotFound;
        };
        let Some(tab) = self.background_sessions.iter().nth(index) else {
            return ReconnectSessionResult::SessionNotFound;
        };
        if let Some(verifier) = self.background_sessions.authentication_at(index).cloned() {
            let tab_id = tab.id;
            match self.authorization_from_sealed_key(&verifier, identity_passphrase, cx) {
                SealedKeyAuthorization::Authorized(authorization) => {
                    return self.complete_authenticated_reconnect(
                        runner_id,
                        tab_id,
                        &authorization,
                        window,
                        cx,
                    );
                }
                SealedKeyAuthorization::NeedsIdentityPassphrase => {
                    self.prompt_to_unlock_sealed_session(runner_id, tab_id, window, cx);
                    return ReconnectSessionResult::AuthenticationFailed;
                }
                SealedKeyAuthorization::NotSealed => {}
            }
            self.prompt_to_reconnect_session(runner_id, tab_id, window, cx);
            return ReconnectSessionResult::AuthenticationFailed;
        }
        let session_id = tab.id;
        if let Some(tab) = self.take_background_session_by_id(session_id, None, cx) {
            self.attach_reconnected_tab(tab, false, window, cx);
            return ReconnectSessionResult::Reconnected;
        }
        ReconnectSessionResult::SessionNotFound
    }

    #[cfg(feature = "session-persistence")]
    pub(crate) fn resume_disk_session(
        &mut self,
        session_id: u64,
        secret: Option<SessionSecret>,
        identities: Option<DiskResumeIdentities>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        // The daemon can keep a recovery-only view of old disk records while
        // its active retention is temporarily memory-backed. Reuse that
        // connection instead of rejecting the restore before the request ever
        // reaches the daemon.
        let runtime = if let Some(runtime) = self.mux.clone() {
            runtime
        } else {
            let runtime = match MuxRuntime::connect_for_disk_resume() {
                Ok(runtime) => runtime,
                Err(error) => {
                    self.pane_output_error = Some(format!(
                        "Could not reach the session multiplexer: {error:#}"
                    ));
                    cx.notify();
                    return ReconnectSessionResult::Rejected;
                }
            };
            self.mux = Some(runtime.clone());
            runtime
        };
        let (identity_paths, identity_passphrases) = identities.map_or_else(
            || (self.disk_resume_identity_paths(), Vec::new()),
            |identities| (identities.paths, identities.passphrases),
        );
        let persisted = match runtime
            .client()
            .resume_with_secret_and_identity_passphrases(
                session_id,
                &identity_paths,
                &identity_passphrases,
                secret.as_ref(),
            ) {
            Ok(persisted) => persisted,
            Err(error) => {
                self.pane_output_error = Some(format!(
                    "Could not resume encrypted session {session_id}: {error:#}"
                ));
                cx.notify();
                return ReconnectSessionResult::Rejected;
            }
        };
        // Refresh the process-wide picker even if rebuilding the tab below
        // fails. The daemon now holds an authenticated restore lease; the
        // consumed disk record is represented by that lease until handoff.
        cx.defer(refresh_process_background_sessions);
        self.rebuild_resumed_disk_tab(session_id, persisted, window, cx)
    }

    /// Rebuilds the tab a resumed disk session becomes, and starts a terminal in
    /// every pane it had.
    ///
    /// Split from [`Self::resume_disk_session`] so the authentication and lease
    /// handling above is not interleaved with reconstructing the tab; by the
    /// time this runs the daemon already holds the restore lease, so a failure
    /// here reports rather than retries.
    #[cfg(feature = "session-persistence")]
    fn rebuild_resumed_disk_tab(
        &mut self,
        session_id: u64,
        persisted: zmux::persistence::PersistedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        let summary_title = persisted.summary.title.clone();
        let persisted_summary = persisted.summary;
        let snapshots = persisted.snapshots;
        let mut state: crate::session_state::TabState =
            match serde_json::from_value(persisted.state).context("reading restored tab state") {
                Ok(state) => state,
                Err(error) => {
                    self.pane_output_error = Some(format!(
                        "Could not restore disk session {session_id}: {error:#}"
                    ));
                    cx.notify();
                    return ReconnectSessionResult::Rejected;
                }
            };
        let restored_panes = restored_pane_metadata(&state, &persisted_summary);
        let restored_metadata = self.prepare_restored_panes(restored_panes.clone());
        let restored_profiles = self.restored_profiles(&restored_panes, &restored_metadata);
        let snapshot_routing_ids = state
            .panes
            .iter()
            .filter_map(|pane| Some((pane.mux_pane_id?, pane.id)))
            .collect::<HashMap<_, _>>();
        let mut replay_by_routing_id = snapshots
            .into_iter()
            .filter_map(|snapshot| {
                snapshot_routing_ids
                    .get(&snapshot.pane_id)
                    .copied()
                    .or_else(|| {
                        state
                            .panes
                            .iter()
                            .find(|pane| pane.id == snapshot.pane_id)
                            .map(|pane| pane.id)
                    })
                    .map(|routing_id| (routing_id, snapshot.bytes))
            })
            .collect::<HashMap<_, _>>();

        // A disk record may have been written by a daemon that was alive when
        // the tab detached. Its pane IDs and exit details describe that old
        // process tree, not a process this restore is allowed to revive.
        // Every base pane below gets a new PTY-backed shell. Its saved screen is
        // replayed into that shell after layout; no old process is reattached.
        state.keep_running = false;
        state.shared = false;
        for pane in &mut state.panes {
            pane.mux_pane_id = None;
            pane.exit = None;
            pane.base_exited = false;
            // Keep the task history, but always show the newly-created base
            // shell rather than a stacked entry that belonged to the old
            // process tree.
            pane.selected_stacked = None;
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = match state.into_tab_by_pane(tab_id, |routing_id, name| {
            restored_profiles
                .get(&routing_id)
                .cloned()
                .unwrap_or_else(|| Profile {
                    name: name.to_owned(),
                    command: task::Shell::System,
                    theme: None,
                    dark_theme: None,
                    icon: ProfileIcon::default(),
                })
        }) {
            Ok(tab) => tab,
            Err(error) => {
                self.pane_output_error = Some(format!(
                    "Could not restore disk session {session_id}: {error:#}"
                ));
                cx.notify();
                return ReconnectSessionResult::Rejected;
            }
        };
        tab.close_policy = TabClosePolicy::Close;
        tab.shared = false;
        // The catalog title is what the user selected. Preserve it until the
        // fresh shell emits a replacement title, just as the live multiplexer
        // attach path does.
        if tab.custom_title.is_none() && !summary_title.is_empty() {
            tab.process_title = Some(summary_title);
        }

        self.next_attention_id = self
            .next_attention_id
            .max(tab.attention_id.saturating_add(1));
        if cx.has_global::<ZettaProcessState>() {
            let process = cx.global_mut::<ZettaProcessState>();
            process.next_attention_id = process
                .next_attention_id
                .max(tab.attention_id.saturating_add(1));
        }

        // The saved ids remain as routing ids for snapshot and command-state
        // mapping, while pane ids are remapped into this window's namespace.
        tab.reassign_ids(tab_id, &mut self.next_pane_id);
        self.pane_controls_hidden_for
            .extend(default_hidden_pane_controls(
                self.launch_config.pane_controls_hidden_by_default,
                tab.panes.iter().map(|pane| pane.id),
            ));
        self.bind_restored_projects(&tab, &restored_metadata);
        let restore_spawns = tab
            .panes
            .iter_mut()
            .map(|pane| {
                let routing_id = pane.routing_id;
                let saved_directory = restored_metadata.working_directory(routing_id);
                let (working_directory, wsl_directory) = if is_wsl_shell(&pane.profile.command) {
                    let wsl_directory = saved_directory
                        .as_deref()
                        .and_then(|directory| directory.to_str())
                        .filter(|directory| directory.starts_with('/'))
                        .map(str::to_owned)
                        .or_else(|| Some("~".to_owned()));
                    (None, wsl_directory)
                } else {
                    (saved_directory, None)
                };
                let prefill = crate::session_state::restore_prefill_from_commands(
                    pane.pending_command.as_deref(),
                    pane.active_command.as_deref(),
                );
                pane.pending_command = None;
                pane.active_command = None;
                (
                    pane.id,
                    pane.profile.clone(),
                    working_directory,
                    wsl_directory,
                    wsl_cwd_tracking_file(&pane.profile, pane.id),
                    pane.environment_overrides.clone(),
                    replay_by_routing_id.remove(&routing_id),
                    prefill,
                )
            })
            .collect::<Vec<_>>();

        for terminal in std::mem::take(&mut self.visible_terminals) {
            terminal.update(cx, |terminal, cx| terminal.set_ui_visible(false, cx));
        }
        self.active_tab = insert_tab_in_pin_order(&mut self.tabs, tab);
        self.mux_panes.adopt_session(tab_id, session_id);
        cx.notify();

        for (
            pane_id,
            profile,
            working_directory,
            wsl_directory,
            wsl_cwd_file,
            environment_overrides,
            replay,
            prefill,
        ) in restore_spawns
        {
            self.spawn_terminal_for_pane(
                TerminalSpawnRequest {
                    working_directory,
                    wsl_directory,
                    wsl_cwd_file,
                    environment: environment_overrides,
                    ..TerminalSpawnRequest::new(tab_id, pane_id, profile)
                }
                .restored(replay, prefill),
                window,
                cx,
            );
        }
        self.focus_active(window, cx);
        ReconnectSessionResult::Reconnected
    }

    #[cfg(not(feature = "session-persistence"))]
    pub(crate) fn resume_disk_session(
        &mut self,
        session_id: u64,
        secret: Option<SessionSecret>,
        identities: Option<DiskResumeIdentities>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        let identities = identities.map(|identities| (identities.paths, identities.passphrases));
        let _ = (session_id, secret, identities, window, cx);
        ReconnectSessionResult::Rejected
    }

    pub(crate) fn resume_disk_session_from_cli(
        &mut self,
        session_id: u64,
        secret: Option<SessionSecret>,
        identities: DiskResumeIdentities,
        completion: std::sync::mpsc::Sender<ReconnectSessionResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.resume_disk_session(session_id, secret, Some(identities), window, cx);
        if result == ReconnectSessionResult::Rejected
            && let Some(error) = self.pane_output_error.take()
        {
            self.show_notice(error, cx);
        }
        let _ = completion.send(result);
    }

    pub(crate) fn reconnect_session_from_cli(
        &mut self,
        runner_id: u64,
        session_id: u64,
        secret: Option<SessionSecret>,
        completion: std::sync::mpsc::Sender<ReconnectSessionResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A session the multiplexer holds is not this process's to look up:
        // its verifier lives in the multiplexer, which checks the secret
        // itself as part of the attach.
        if self.multiplexer_holds_session(session_id) {
            let result = match self.attach_multiplexer_session(session_id, secret, window, cx) {
                Ok(AttachOutcomeSummary::Attached) => ReconnectSessionResult::Reconnected,
                Ok(AttachOutcomeSummary::AuthenticationRequired)
                | Ok(AttachOutcomeSummary::AuthenticationFailed) => {
                    ReconnectSessionResult::AuthenticationFailed
                }
                Err(error) => {
                    self.pane_output_error =
                        Some(format!("Could not attach that session: {error:#}"));
                    cx.notify();
                    ReconnectSessionResult::Rejected
                }
            };
            let _ = completion.send(result);
            return;
        }
        let verifier = self.process_background_session_authentication(runner_id, session_id, cx);
        if verifier.is_none() {
            let result = if secret.is_none() {
                self.reconnect_process_background_session(runner_id, session_id, None, window, cx)
            } else {
                ReconnectSessionResult::AuthenticationFailed
            };
            let _ = completion.send(result);
            return;
        }
        let Some(secret) = secret else {
            let _ = completion.send(ReconnectSessionResult::AuthenticationFailed);
            return;
        };
        // Refused attempts report the same status as wrong ones, so the backoff
        // window cannot be probed to learn whether a guess was even evaluated.
        if self.process_authentication_is_refused(runner_id, session_id, cx) {
            let _ = completion.send(ReconnectSessionResult::AuthenticationFailed);
            return;
        }
        let generation = self.session_authentication_generation;
        cx.spawn_in(window, async move |this, cx| {
            let authorization = cx
                .background_spawn(async move {
                    let verifier =
                        verifier.context("the protected session is no longer available")?;
                    Ok::<_, anyhow::Error>(verifier.verify(secret.expose()))
                })
                .await
                .ok()
                .flatten();
            let result = this
                .update_in(cx, |this, window, cx| {
                    if this.session_authentication_generation != generation {
                        return ReconnectSessionResult::Rejected;
                    }
                    let Some(authorization) = authorization else {
                        this.process_record_failed_authentication(runner_id, session_id, cx);
                        return ReconnectSessionResult::AuthenticationFailed;
                    };
                    this.process_clear_failed_authentications(runner_id, session_id, cx);
                    this.complete_authenticated_reconnect(
                        runner_id,
                        session_id,
                        &authorization,
                        window,
                        cx,
                    )
                })
                .unwrap_or(ReconnectSessionResult::Rejected);
            let _ = completion.send(result);
        })
        .detach();
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the CLI request's five fields plus the GPUI window and context"
    )]
    pub(crate) fn open_remote_session_from_cli(
        &mut self,
        target: String,
        port: Option<u16>,
        session_id: u64,
        secret: Option<SessionSecret>,
        completion: std::sync::mpsc::Sender<ReconnectSessionResult>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = {
            let target = zmux::remote::RemoteTarget::new(target).with_port(port);
            match self.attach_remote_multiplexer_session(target, session_id, secret, window, cx) {
                Ok(AttachOutcomeSummary::Attached) => ReconnectSessionResult::Reconnected,
                Ok(AttachOutcomeSummary::AuthenticationRequired)
                | Ok(AttachOutcomeSummary::AuthenticationFailed) => {
                    ReconnectSessionResult::AuthenticationFailed
                }
                Err(error) => {
                    self.pane_output_error = Some(format!(
                        "Could not attach the remote session {session_id}: {error:#}"
                    ));
                    cx.notify();
                    ReconnectSessionResult::Rejected
                }
            }
        };
        if result == ReconnectSessionResult::Rejected
            && let Some(error) = self.pane_output_error.take()
        {
            self.show_notice(error, cx);
        }
        let _ = completion.send(result);
    }

    pub(crate) fn background_session_authentication(
        &self,
        session_id: u64,
    ) -> Option<SessionAuthentication> {
        let index = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)?;
        self.background_sessions.authentication_at(index).cloned()
    }

    fn background_session_is_transferable(&self, session_id: u64) -> bool {
        self.background_sessions
            .iter()
            .find(|tab| tab.id == session_id)
            .is_some_and(|tab| {
                tab.panes.iter().all(|pane| {
                    (pane.terminal.is_some() || pane.error.is_some())
                        && pane
                            .stack
                            .entries
                            .iter()
                            .all(|entry| entry.terminal.is_some() || entry.error.is_some())
                })
            })
    }

    pub(crate) fn process_background_session_authentication(
        &self,
        runner_id: u64,
        session_id: u64,
        cx: &App,
    ) -> Option<SessionAuthentication> {
        if runner_id == self.background_sessions.runner_id() {
            return self.background_session_authentication(session_id);
        }
        zetta_for_runner(runner_id, cx)?
            .read(cx)
            .background_session_authentication(session_id)
    }

    fn background_session_index(&self, session_id: u64) -> Option<usize> {
        self.background_sessions
            .iter()
            .position(|tab| tab.id == session_id)
    }

    fn authentication_is_refused(&self, session_id: u64) -> bool {
        self.background_session_index(session_id)
            .is_some_and(|index| self.background_sessions.authentication_is_refused_at(index))
    }

    fn record_failed_authentication(&mut self, session_id: u64) {
        if let Some(index) = self.background_session_index(session_id) {
            self.background_sessions
                .record_failed_authentication_at(index);
        }
    }

    fn clear_failed_authentications(&mut self, session_id: u64) {
        if let Some(index) = self.background_session_index(session_id) {
            self.background_sessions
                .clear_failed_authentications_at(index);
        }
    }

    /// Whether the owning runner is currently refusing attempts for this
    /// session. Checked before the secret is evaluated, so a refused attempt
    /// costs an attacker a full Argon2 verification's worth of nothing.
    pub(crate) fn process_authentication_is_refused(
        &self,
        runner_id: u64,
        session_id: u64,
        cx: &App,
    ) -> bool {
        if runner_id == self.background_sessions.runner_id() {
            return self.authentication_is_refused(session_id);
        }
        zetta_for_runner(runner_id, cx)
            .is_some_and(|source| source.read(cx).authentication_is_refused(session_id))
    }

    pub(crate) fn process_record_failed_authentication(
        &mut self,
        runner_id: u64,
        session_id: u64,
        cx: &mut Context<Self>,
    ) {
        if runner_id == self.background_sessions.runner_id() {
            self.record_failed_authentication(session_id);
            return;
        }
        if let Some(source) = zetta_for_runner(runner_id, cx) {
            source.update(cx, |source, _| {
                source.record_failed_authentication(session_id);
            });
        }
    }

    pub(crate) fn process_clear_failed_authentications(
        &mut self,
        runner_id: u64,
        session_id: u64,
        cx: &mut Context<Self>,
    ) {
        if runner_id == self.background_sessions.runner_id() {
            self.clear_failed_authentications(session_id);
            return;
        }
        if let Some(source) = zetta_for_runner(runner_id, cx) {
            source.update(cx, |source, _| {
                source.clear_failed_authentications(session_id);
            });
        }
    }

    pub(crate) fn complete_authenticated_reconnect(
        &mut self,
        runner_id: u64,
        session_id: u64,
        authorization: &VerifiedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ReconnectSessionResult {
        let tab = if runner_id == self.background_sessions.runner_id() {
            self.take_background_session_by_id(session_id, Some(authorization), cx)
        } else {
            let Some(source) = zetta_for_runner(runner_id, cx) else {
                return ReconnectSessionResult::SessionNotFound;
            };
            if !source
                .read(cx)
                .background_session_is_transferable(session_id)
            {
                self.pane_output_error = Some(
                    "That background session is still starting. Try attaching it again shortly."
                        .to_owned(),
                );
                cx.notify();
                return ReconnectSessionResult::StillStarting;
            }
            let tab = source.update(cx, |source, cx| {
                source.take_background_session_by_id(session_id, Some(authorization), cx)
            });
            prune_empty_dormant_runners(cx);
            tab
        };
        if let Some(tab) = tab {
            let transferred = runner_id != self.background_sessions.runner_id();
            self.attach_reconnected_tab(tab, transferred, window, cx);
            return ReconnectSessionResult::Reconnected;
        }
        ReconnectSessionResult::SessionNotFound
    }
}
