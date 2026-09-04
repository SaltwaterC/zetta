//! Detaching a tab, protecting it, sharing it, and storing it as a background
//! session.
//!
//! This is the outbound half: everything that takes a tab this window owns and
//! hands it to a holder — the multiplexer (`multiplexer.rs`) or, under
//! `--no-mux`, this process's own background sessions. Coming back is
//! `reconnect.rs`.

use super::*;

impl Zetta {
    pub(crate) fn detach_tab(
        &mut self,
        _: &DetachTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_tab >= self.tabs.len() {
            return;
        }
        let tab_id = self.tabs[self.active_tab].id;
        self.protect_and_then(tab_id, ProtectedSessionAction::Detach, window, cx);
    }

    /// Runs `action`, asking for a secret first if nothing protects the session.
    ///
    /// The one path all three actions take to settle protection before acting.
    /// In daemon mode, the resulting session can be attached by another window
    /// or process; `--no-mux` keeps **Keep running** process-local.
    pub(crate) fn protect_and_then(
        &mut self,
        tab_id: u64,
        action: ProtectedSessionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mux_panes.is_remote_tab(tab_id) {
            self.show_notice(
                "Remote sessions are live-only and cannot be stored as local background sessions.",
                cx,
            );
            self.focus_active(window, cx);
            return;
        }
        match self.existing_protection(tab_id) {
            Some(authentication) => {
                self.apply_protected_session_action(tab_id, action, authentication, window, cx);
            }
            #[cfg(feature = "session-persistence")]
            None if self.auto_protect.is_some() => {
                self.protect_automatically_and_then(tab_id, action, window, cx);
            }
            None => self.prompt_for_session_secret(tab_id, action, window, cx),
        }
    }

    /// Protects a session with the user's age key and then acts, without asking
    /// for anything.
    ///
    /// The dialog exists to settle a secret; here the secret is generated, so
    /// there is nothing to settle and nothing to show. Argon2id and the age seal
    /// run on a background thread — the same reason the dialog hashes off the UI
    /// thread — which is why this is spawned rather than done inline.
    ///
    /// A failure falls back to the dialog rather than proceeding: the user asked
    /// for the session to be protected, and quietly leaving it open to any
    /// process that can reach the multiplexer is the one outcome they did not
    /// ask for.
    #[cfg(feature = "session-persistence")]
    fn protect_automatically_and_then(
        &mut self,
        tab_id: u64,
        action: ProtectedSessionAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(auto_protect) = self.auto_protect.clone() else {
            self.prompt_for_session_secret(tab_id, action, window, cx);
            return;
        };
        cx.spawn_in(window, async move |this, cx| {
            let sealed = cx
                .background_spawn(async move { auto_protect.seal() })
                .await;
            this.update_in(cx, |this, window, cx| match sealed {
                Ok(sealed) => this.apply_protected_session_action(
                    tab_id,
                    action,
                    Some(sealed.authentication),
                    window,
                    cx,
                ),
                Err(error) => {
                    this.show_notice(
                        format!(
                            "Could not protect this session with your age key: {error:#}. \
                             Choose a secret instead."
                        ),
                        cx,
                    );
                    this.prompt_for_session_secret(tab_id, action, window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// What already protects this tab's session, if anything.
    ///
    /// `Some(Some(_))` is a secret this window chose and can send again;
    /// `Some(None)` is one the multiplexer holds and this window does not know,
    /// which is what a tab restored from a kept session has. `None` means
    /// nothing protects it yet, so the user has to choose a secret — asked here
    /// rather than always, because a second secret would replace the one
    /// whoever knows it is expecting.
    fn existing_protection(&self, tab_id: u64) -> Option<Option<SessionAuthentication>> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if let Some(authentication) = tab.close_policy.background_authentication().flatten() {
            return Some(Some(authentication.clone()));
        }
        if self.no_mux {
            return None;
        }
        // Read from the published catalog: the multiplexer owns the verifier,
        // and this is the same source the reconnect picker lists.
        let session_id = self.mux_panes.session_id(tab_id)?;
        let protected = crate::background_sessions::read_session_catalogs(
            &crate::background_sessions::session_catalog_dir(),
        )
        .is_ok_and(|catalogs| {
            crate::background_sessions::multiplexer_held_catalog_sessions(
                &catalogs,
                crate::background_sessions::process_is_zetta,
                std::process::id(),
            )
            .any(|(_, session)| session.id == session_id && session.authentication_required)
        });
        protected.then_some(None)
    }

    /// The sealed session key a multiplexer-held session publishes, if it has
    /// one.
    ///
    /// Read from the same catalog `existing_protection` reads. The envelope is
    /// public ciphertext, so finding it here reveals nothing; opening it is what
    /// needs the private key.
    #[cfg(feature = "session-persistence")]
    fn multiplexer_session_key_envelope(&self, session_id: u64) -> Option<String> {
        crate::background_sessions::read_session_catalogs(
            &crate::background_sessions::session_catalog_dir(),
        )
        .ok()
        .and_then(|catalogs| {
            crate::background_sessions::multiplexer_held_catalog_sessions(
                &catalogs,
                crate::background_sessions::process_is_zetta,
                std::process::id(),
            )
            .find(|(_, session)| session.id == session_id)
            .and_then(|(_, session)| session.key_envelope.clone())
        })
    }

    /// The secret for a multiplexer-held session this window is about to attach,
    /// recovered from its sealed key.
    ///
    /// `Ok(None)` covers everything that is not automatically protected: an
    /// unprotected session, a session protected by a typed secret, and — when
    /// automatic protection is not configured here — one this window simply
    /// cannot open, which is left to the dialog to report as a failed attempt.
    /// An `Err` is an effective identity that could not open an envelope, which
    /// is worth saying plainly rather than turning into an unanswerable prompt.
    #[cfg(feature = "session-persistence")]
    pub(super) fn recovered_session_secret(
        &self,
        session_id: u64,
        passphrase: Option<SessionSecret>,
    ) -> anyhow::Result<SealedKeyRecovery> {
        // Checked before the catalog is read. Finding the envelope means reading
        // and parsing every catalog file, and a window with no automatic
        // protection configured could do nothing with what it found — so every
        // reconnect would have paid for it and thrown the answer away.
        let Some(auto_protect) = self.auto_protect.as_ref() else {
            return Ok(SealedKeyRecovery::NotSealed);
        };
        let Some(envelope) = self.multiplexer_session_key_envelope(session_id) else {
            return Ok(SealedKeyRecovery::NotSealed);
        };
        if passphrase.is_none() && auto_protect.identity_passphrase_required()? {
            return Ok(SealedKeyRecovery::NeedsIdentityPassphrase);
        }
        Ok(SealedKeyRecovery::Recovered(
            auto_protect.open(&envelope, passphrase)?,
        ))
    }

    #[cfg(not(feature = "session-persistence"))]
    pub(super) fn recovered_session_secret(
        &self,
        _: u64,
        _: Option<SessionSecret>,
    ) -> anyhow::Result<SealedKeyRecovery> {
        Ok(SealedKeyRecovery::NotSealed)
    }

    /// Proof of authentication for a session another Zetta process — or this one
    /// — is holding, obtained by opening its sealed key rather than by asking.
    ///
    /// The verifier is checked as always, so this is the same proof a typed
    /// secret produces; only where the secret came from differs. Argon2id runs
    /// inline here, unlike the dialog's background check, because this whole
    /// reconnect path already reads catalogs and decrypts, and it answers its
    /// caller synchronously. That is one hash per reconnect, not per frame.
    ///
    /// `None` for anything not protected this way, and for a failure to open —
    /// the caller falls back to the dialog, which is the right answer for a
    /// session whose secret really was typed by someone.
    #[cfg(feature = "session-persistence")]
    pub(super) fn authorization_from_sealed_key(
        &mut self,
        authentication: &SessionAuthentication,
        passphrase: Option<SessionSecret>,
        cx: &mut Context<Self>,
    ) -> SealedKeyAuthorization {
        let Some(auto_protect) = self.auto_protect.as_ref() else {
            return SealedKeyAuthorization::NotSealed;
        };
        let Some(envelope) = authentication.key_envelope() else {
            return SealedKeyAuthorization::NotSealed;
        };
        if passphrase.is_none() {
            match auto_protect.identity_passphrase_required() {
                Ok(true) => return SealedKeyAuthorization::NeedsIdentityPassphrase,
                Ok(false) => {}
                Err(error) => {
                    self.show_notice(
                        format!("Could not inspect your age identity: {error:#}"),
                        cx,
                    );
                    return SealedKeyAuthorization::NotSealed;
                }
            }
        }
        match auto_protect.open(envelope, passphrase) {
            Ok(secret) => match authentication.verify(secret.expose()) {
                Some(authorization) => SealedKeyAuthorization::Authorized(authorization),
                None => SealedKeyAuthorization::NotSealed,
            },
            Err(error) => {
                self.show_notice(
                    format!("Could not open that session with your age identity: {error:#}"),
                    cx,
                );
                SealedKeyAuthorization::NotSealed
            }
        }
    }

    #[cfg(not(feature = "session-persistence"))]
    pub(super) fn authorization_from_sealed_key(
        &mut self,
        _: &SessionAuthentication,
        _: Option<SessionSecret>,
        _: &mut Context<Self>,
    ) -> SealedKeyAuthorization {
        SealedKeyAuthorization::NotSealed
    }

    /// Carries out an action once the session's secret is settled.
    ///
    /// Reached both ways: directly when a secret already exists, and from the
    /// prompt once the user has chosen one.
    pub(crate) fn apply_protected_session_action(
        &mut self,
        tab_id: u64,
        action: ProtectedSessionAction,
        authentication: Option<SessionAuthentication>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            ProtectedSessionAction::Detach => {
                self.detach_tab_by_id(tab_id, authentication, window, cx);
            }
            ProtectedSessionAction::KeepRunning => {
                let sharing_authentication = authentication.clone();
                if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                    tab.close_policy = TabClosePolicy::Background { authentication };
                }
                if !self.no_mux {
                    // Keep-running sessions used to be handed to the multiplexer
                    // as process-scoped sessions, which made the setting
                    // ineffective once the owning Zetta process went away. Offer
                    // it now as well so the handoff on window close preserves a
                    // cross-process reconnect path. The sharing toggle remains
                    // independent: an explicit unshare before close restores
                    // private ownership.
                    self.set_tab_sharing(tab_id, true, sharing_authentication, cx);
                }
            }
            ProtectedSessionAction::Share => self.set_tab_sharing(tab_id, true, authentication, cx),
        }
        // Whichever action ran, the pane gets the keyboard back. Reaching here
        // from the prompt means focus is on an overlay that has just been taken
        // down, and leaving it there leaves the window with nothing focused: no
        // pane takes typing, and every keybinding bound to `Zetta > Terminal`
        // stops dispatching — which is why a lifecycle action could turn sharing
        // on and then never turn it off, while the same action from the tab's
        // context menu, dispatched directly, worked both ways.
        self.focus_active(window, cx);
    }

    /// Offers the active tab to other Zetta windows, or stops offering it.
    ///
    /// Deliberately not routed through detaching. Sharing a tab leaves it exactly
    /// where it is — same window, same panes, still being read by this process —
    /// and only makes the session visible to other windows, so the first of them
    /// to attach triggers the revoke handover that puts both into shared mode.
    /// Detaching remains a way to get there, because a detached session is
    /// attachable too, but it is no longer the only one.
    pub(crate) fn toggle_tab_sharing(
        &mut self,
        _: &ToggleTabSharing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        if self.mux_panes.is_remote_tab(tab_id) {
            self.show_notice(
                "Remote sessions are already shared by the remote multiplexer and cannot be re-scoped from Zetta.",
                cx,
            );
            self.focus_active(window, cx);
            return;
        }
        if self.no_mux {
            self.show_notice(
                "Sharing requires the session multiplexer; restart without --no-mux to enable it.",
                cx,
            );
            self.focus_active(window, cx);
            return;
        }
        if tab.shared {
            // Scoping a session back to this window takes nothing away from
            // anybody but the windows that could join it, so it needs no secret.
            self.set_tab_sharing(tab_id, false, None, cx);
            return;
        }
        self.protect_and_then(tab_id, ProtectedSessionAction::Share, window, cx);
    }

    pub(crate) fn set_tab_sharing(
        &mut self,
        tab_id: u64,
        offered: bool,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        match self.publish_session_offer(index, offered, authentication, cx) {
            Ok(true) => {
                self.tabs[index].shared = offered;
                self.finish_background_session_change(cx);
                // Nothing about the tab changes when it is shared, so without
                // saying so the toggle has no visible effect at all beyond a
                // checkmark in a menu that has already closed.
                self.show_notice(
                    if offered {
                        "This tab can now be joined from another Zetta window."
                    } else {
                        "This tab is no longer shared, and belongs to this window again."
                    },
                    cx,
                );
            }
            Ok(false) => {
                self.show_notice(
                    "Sharing requires the session multiplexer; this tab is running with --no-mux.",
                    cx,
                );
            }
            // A refused *unshare* is guidance, not a failure: the multiplexer only
            // scopes a session back to one window while one window has it, and it
            // says which. The tab stays shared, which is what the menu then shows.
            Err(error) if !offered => {
                self.show_notice(format!("{error:#}"), cx);
            }
            Err(error) => {
                self.pane_output_error = Some(format!("Could not share this tab: {error:#}"));
            }
        }
        cx.notify();
    }

    /// Tells the multiplexer whether this tab's session is on offer.
    ///
    /// Returns `false` when the tab has no multiplexer session at all — a pane
    /// that fell back to a local process — because there is then nothing another
    /// window could attach to.
    fn publish_session_offer(
        &self,
        index: usize,
        offered: bool,
        authentication: Option<SessionAuthentication>,
        cx: &App,
    ) -> anyhow::Result<bool> {
        let tab = &self.tabs[index];
        let (Some(runtime), Some(session_id)) = (
            self.mux_panes
                .runtime_for_tab(tab.id)
                .or_else(|| self.mux.clone()),
            self.mux_panes.session_id(tab.id),
        ) else {
            return Ok(false);
        };
        if runtime.is_remote() {
            return Ok(false);
        }
        let protected = authentication.is_some()
            || tab
                .close_policy
                .background_authentication()
                .flatten()
                .is_some();
        let (summary, state) = self.session_publication(tab, session_id, protected, cx)?;
        // The verifier is what makes sharing safe, and the multiplexer refuses to
        // offer a session that has none: a window joining one is handed whatever
        // its terminals can already do. Scoping a session back needs none, and
        // leaves the secret it has in place. The sealed key, when there is one,
        // travels inside the authentication so it cannot be separated from the
        // verifier it belongs to.
        if offered && runtime.retention().keeps_snapshot() {
            // An exclusively attached pane is read by this window, so the
            // daemon deliberately has no retained screen for it. Checkpoint
            // each live pane before publishing the offer; otherwise a daemon
            // restart between sharing and backgrounding would restore an empty
            // pane. Panes already relayed through a shared connection are
            // already retained by the daemon and cannot accept an exclusive
            // checkpoint from this window.
            let snapshots = tab
                .panes
                .iter()
                .filter(|pane| !self.shared_panes.contains_key(&pane.id))
                .filter_map(|pane| {
                    let mux_pane_id = self.mux_panes.mux_pane_id(pane.id)?;
                    let terminal = pane.terminal.as_ref()?.read(cx);
                    let bounds = terminal.last_content().terminal_bounds;
                    Some((
                        mux_pane_id,
                        terminal.ansi_snapshot(SNAPSHOT_LINES),
                        bounds.num_columns() as u16,
                        bounds.num_lines() as u16,
                    ))
                })
                .collect::<Vec<_>>();
            for (mux_pane_id, snapshot, columns, lines) in snapshots {
                runtime
                    .client()
                    .send_snapshot(session_id, mux_pane_id, snapshot, columns, lines)
                    .with_context(|| {
                        format!(
                            "checkpointing pane {mux_pane_id} before sharing session {session_id}"
                        )
                    })?;
            }
        }
        runtime
            .client()
            .share(session_id, summary, state, authentication.as_ref(), offered)?;
        Ok(true)
    }

    /// A fresh publication for a tab that is being shared, or `None` when it is
    /// not being shared or has no multiplexer session.
    ///
    /// A session shared while it is on screen keeps changing after it was
    /// offered — panes are split and closed, the tab is renamed — so what the
    /// multiplexer holds has to be refreshed at the one moment a joining client
    /// is about to read it.
    pub(super) fn shared_session_refresh(
        &self,
        tab_id: u64,
        cx: &App,
    ) -> Option<(u64, BackgroundSessionSummary, serde_json::Value)> {
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        if !tab.shared {
            return None;
        }
        let session_id = self.mux_panes.session_id(tab_id)?;
        let protected = tab
            .close_policy
            .background_authentication()
            .flatten()
            .is_some();
        let (summary, state) = self
            .session_publication(tab, session_id, protected, cx)
            .ok()?;
        Some((session_id, summary, state))
    }

    pub(crate) fn toggle_auto_background_tab(
        &mut self,
        _: &ToggleAutoBackgroundTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get(self.active_tab) else {
            return;
        };
        let tab_id = tab.id;
        if self.mux_panes.is_remote_tab(tab_id) {
            self.show_notice(
                "Remote sessions are live-only and cannot be kept as local background sessions.",
                cx,
            );
            return;
        }
        if matches!(tab.close_policy, TabClosePolicy::Background { .. }) {
            self.tabs[self.active_tab].close_policy = TabClosePolicy::Close;
            cx.notify();
        } else {
            self.protect_and_then(tab_id, ProtectedSessionAction::KeepRunning, window, cx);
        }
    }

    pub(crate) fn detach_tab_by_id(
        &mut self,
        tab_id: u64,
        authentication: Option<SessionAuthentication>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        self.active_tab = index;
        if self.tab_search.is_some() {
            self.dismiss_tab_search(window, cx);
        }
        self.move_tab_to_background(self.active_tab, authentication, cx);

        if self.tabs.is_empty() {
            self.active_tab = 0;
            self.open_tab(window, cx);
        } else {
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn move_tab_to_background(
        &mut self,
        index: usize,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) {
        // The panes leave this window: their shared connections close with
        // their terminals, so the multiplexer's shared set stops counting on
        // them. The pane itself stays attached to its session on the daemon.
        let pane_ids = self.tabs[index]
            .panes
            .iter()
            .map(|pane| pane.id)
            .collect::<Vec<_>>();
        for pane_id in pane_ids {
            self.drop_shared_pane(pane_id);
        }
        let tab = self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        } else if self.active_tab >= self.tabs.len() && !self.tabs.is_empty() {
            self.active_tab = self.tabs.len() - 1;
        }
        self.disable_tab_move_mode_if_unavailable(cx);
        if let Some(tab) = self.store_background_tab(tab, authentication, cx) {
            // A normal launch must not silently turn a failed daemon handoff
            // into an in-process background session. Put the tab back exactly
            // where it was so the user can retry after fixing the daemon.
            let insertion_index = index.min(self.tabs.len());
            self.tabs.insert(insertion_index, tab);
            self.active_tab = insertion_index;
        }
        self.finish_background_session_change(cx);
    }

    pub(crate) fn store_background_tab(
        &mut self,
        mut tab: Tab,
        authentication: Option<SessionAuthentication>,
        cx: &mut Context<Self>,
    ) -> Option<Tab> {
        let terminals = tab
            .panes
            .iter()
            .flat_map(|pane| {
                pane.terminal
                    .iter()
                    .map(move |terminal| (pane.id, None, terminal.clone()))
                    .chain(pane.stack.entries.iter().filter_map(move |entry| {
                        Some((pane.id, Some(entry.id), entry.terminal.clone()?))
                    }))
            })
            .collect::<Vec<_>>();
        let tab_id = tab.id;

        // Hand the session to the multiplexer, which already owns the
        // processes. Dropping the tab then drops the PTY descriptors this
        // process was holding, and the multiplexer resumes reading them.
        match self.hand_session_to_multiplexer(&mut tab, authentication.as_ref(), cx) {
            Ok(true) => {
                let run_registry = crate::run_command::process_run_registry();
                for pane in &tab.panes {
                    run_registry.pane_closed(crate::run_command::RunPaneIdentity::new(
                        tab.attention_id,
                        pane.routing_id,
                    ));
                    for entry in &pane.stack.entries {
                        run_registry.pane_closed(crate::run_command::RunPaneIdentity::new(
                            tab.attention_id,
                            entry.routing_id,
                        ));
                    }
                }
                self.mux_panes.forget_tab(tab_id);
                for pane in &tab.panes {
                    self.mux_panes.forget_pane(pane.id);
                }
                return None;
            }
            Ok(false) => {}
            Err(error) => {
                if !self.no_mux {
                    self.pane_output_error = Some(format!(
                        "Could not hand the session to the multiplexer; it remains in this window: {error:#}"
                    ));
                    cx.notify();
                    return Some(tab);
                }
                self.pane_output_error = Some(format!(
                    "Could not hand the session to the multiplexer, so it is being kept in this \
                     window instead: {error:#}"
                ));
                cx.notify();
            }
        }

        tab.rename_buffer = None;
        tab.renaming_pane = None;
        for pane in &mut tab.panes {
            pane.view = None;
            for entry in &mut pane.stack.entries {
                entry.view = None;
            }
        }
        self.background_sessions.detach(tab, authentication);
        for (pane_id, stack_id, terminal) in terminals {
            if let Some(stack_id) = stack_id {
                self.observe_background_stacked_terminal(pane_id, stack_id, terminal.clone(), cx);
            } else {
                self.observe_background_terminal(tab_id, pane_id, terminal.clone(), cx);
                self.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
            }
            terminal.update(cx, |terminal, cx| {
                terminal.set_ui_visible(false, cx);
                terminal.refresh_foreground_process(cx);
            });
        }
        None
    }

    pub(crate) fn finish_background_session_change(&mut self, cx: &mut Context<Self>) {
        self.schedule_background_process_refresh(cx);
        self.publish_background_session_catalog(cx);
    }

    pub(crate) fn reconnect_session(
        &mut self,
        _: &ReconnectSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = self.process_background_session_picker_entries(cx);
        match reconnect_request(entries.len()) {
            ReconnectRequest::None => {}
            ReconnectRequest::Immediate(index) => {
                let (runner_id, session_id, _, _) = &entries[index];
                self.reconnect_process_background_session(
                    *runner_id,
                    *session_id,
                    None,
                    window,
                    cx,
                );
            }
            ReconnectRequest::Choose => self.reconnect_menu_handle.show(window, cx),
        }
    }
}
