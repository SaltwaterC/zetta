use super::*;
use crate::mux::SharedPaneEntry;
use crate::rename::resolve_tab_title;
use crate::worktree_detection::terminal_event_requires_worktree_detection;

const BACKGROUND_PROCESS_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub(crate) struct DiskResumeIdentities {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) passphrases: Vec<Option<SessionSecret>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReconnectRequest {
    None,
    Immediate(usize),
    Choose,
}

fn reconnect_request(session_count: usize) -> ReconnectRequest {
    match session_count {
        0 => ReconnectRequest::None,
        1 => ReconnectRequest::Immediate(0),
        _ => ReconnectRequest::Choose,
    }
}

/// Whether a reconnect entry names a multiplexer session a tab in this window
/// is already showing.
///
/// Sharing a tab publishes its session while the tab stays on screen, so the
/// window that shared it would otherwise be offered its own session back. Taking
/// that offer is not a join: the multiplexer recognises the process that already
/// holds the pane and hands the terminal straight back, giving this window a
/// second tab reading the same pty as the first, with the two splitting its
/// output between them.
///
/// The runner check is what keeps this from hiding an unrelated session. A
/// session kept inside this process because the multiplexer was unreachable is
/// numbered from a different counter than the multiplexer's, so the two id spaces
/// can collide; an entry under this window's own runner is therefore never
/// hidden, and the reconnect path refuses the duplicate anyway.
fn session_is_already_shown_here(
    panes: &crate::mux::MuxPanes,
    (runner_id, session_id, _, _): &ProcessBackgroundSessionEntry,
    own_runner: u64,
) -> bool {
    *runner_id != own_runner && panes.holds_session(*session_id)
}

/// Copies one pane's project root onto every pane, and every stacked command, of
/// a tab that is arriving in this window.
///
/// Stacked entries are included because they are panes as far as the project
/// registry is concerned: each has its own id, its own view and its own theme.
fn inherit_project_for_panes(
    projects: &mut crate::project_context::ProjectState,
    source_pane_id: u64,
    tab: &Tab,
) {
    for pane in &tab.panes {
        projects.inherit_pane_root(source_pane_id, pane.id);
        for entry in &pane.stack.entries {
            projects.inherit_pane_root(source_pane_id, entry.id);
        }
    }
}

/// What to do with the size the multiplexer arbitrated for a shared pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SharedSizeAction {
    /// The viewer's own size is not known yet. Keep the arbitrated size pending
    /// rather than resizing against a guess.
    WaitForLayout,
    /// The viewer is already showing the pane at that size.
    AlreadyMatches,
    Resize,
}

/// Whether these bounds describe a pane that has been laid out, rather than the
/// placeholder a `TerminalContent` starts with.
///
/// The placeholder is 100 columns by 6 lines, and mistaking it for a real layout
/// has now caused two separate faults — a window resized to fit a size it already
/// had, and a joining window telling the multiplexer its pane was six rows tall,
/// which arbitrated *every* viewer down to six. Both sides of the size exchange ask
/// this, so they cannot disagree about what counts as known.
fn bounds_are_laid_out(bounds: terminal::TerminalBounds) -> bool {
    bounds != terminal::TerminalBounds::default()
}

/// The size to tell the multiplexer this viewer is showing a pane at, if it is
/// known yet.
///
/// `None` before the pane's first layout. Reporting the placeholder instead made a
/// window that had only just joined claim six rows, and since the pane must fit
/// inside every viewer, the window that had been showing it perfectly well was
/// resized down to match.
fn shared_size_to_report(bounds: terminal::TerminalBounds) -> Option<(u16, u16)> {
    bounds_are_laid_out(bounds).then(|| (bounds.num_columns() as u16, bounds.num_lines() as u16))
}

/// Decides whether an arbitrated size has to be imposed on this viewer.
///
/// The arbitrated size only needs *applying* to a viewer showing the pane at
/// some other size — the pty runs at the smallest of the viewers, so a larger one
/// has to shrink its grid or the shell's wrapping stops lining up with the cells
/// drawn. A viewer that already matches must not be touched: two windows tiled to
/// the same size by a compositor are the common case, and resizing one of them to
/// the size it already had moves the user's window for no reason.
///
/// The layout check is the other half of the same bug. A terminal reports the
/// placeholder bounds a `TerminalContent` starts with until its pane has been laid
/// out and synced once, and those are 100x6 — so a pane that was *already* the
/// arbitrated 98x51 looked like a two-column, forty-five-row difference, and the
/// window was resized to fit a size it already had. This ran before the first
/// paint and then reported success, so nothing ever corrected it.
fn shared_size_action(
    bounds: Option<terminal::TerminalBounds>,
    columns: u16,
    lines: u16,
) -> SharedSizeAction {
    let Some(bounds) = bounds else {
        return SharedSizeAction::WaitForLayout;
    };
    if !bounds_are_laid_out(bounds) {
        return SharedSizeAction::WaitForLayout;
    }
    if (bounds.num_columns(), bounds.num_lines()) == (columns as usize, lines as usize) {
        return SharedSizeAction::AlreadyMatches;
    }
    SharedSizeAction::Resize
}

fn remove_exited_background_pane(
    sessions: &mut BackgroundSessionRunner<Tab>,
    pane_id: u64,
) -> Option<Vec<u64>> {
    let session_index = sessions
        .iter()
        .position(|tab| tab.pane(pane_id).is_some())?;
    let pane_count = sessions.iter().nth(session_index)?.panes.len();
    if pane_count == 1 {
        let tab = sessions.reconnect_at(session_index)?;
        return Some(tab.panes.into_iter().map(|pane| pane.id).collect());
    }

    let tab = sessions.iter_mut().nth(session_index)?;
    let layout = tab.layout.clone().without(pane_id)?;
    tab.remove_pane(pane_id);
    tab.layout = layout;
    tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
    Some(vec![pane_id])
}

/// An action that normally makes a tab's session reachable beyond this window.
///
/// In daemon mode each ends with the session attachable by something other than
/// the window driving it now — another window joining it, or a reconnect after
/// this one is gone — so each offers the secret that will gate that. Offered,
/// not required: an empty dialog leaves the session unprotected, which is what
/// detaching has always meant and therefore what all three mean. They share a
/// path — settle the secret, then act — and differ only in wording and in what
/// "act" means. `KeepRunning` remains process-local when `--no-mux` is active;
/// there is no multiplexer to make it reachable from another process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProtectedSessionAction {
    Detach,
    KeepRunning,
    Share,
}

impl ProtectedSessionAction {
    pub(crate) fn title(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => "Detach session",
            Self::KeepRunning if no_mux => "Keep tab running after close",
            Self::KeepRunning => "Keep and share tab after close",
            Self::Share => "Share tab",
        }
    }

    pub(crate) fn description(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => {
                "Leave both fields blank and press Enter to detach without authentication. \
                 Otherwise, enter and confirm a secret."
            }
            Self::KeepRunning if no_mux => {
                "Choose the authentication required when this tab is reattached. In --no-mux \
                 mode the session stays inside this Zetta process after the window closes and \
                 cannot be shared with another process. Press Enter with both fields empty for \
                 no authentication."
            }
            Self::KeepRunning => {
                "Choose the authentication required when this tab is reattached. This also makes \
                 the session available to another Zetta process after this window closes. Press \
                 Enter with both fields empty for no authentication."
            }
            Self::Share => {
                "Choose the authentication a window joining this tab has to present; it can \
                 then do everything this tab's terminals can already do. Press Enter with both \
                 fields empty for no authentication."
            }
        }
    }

    pub(crate) fn submit_label(self, no_mux: bool) -> &'static str {
        match self {
            Self::Detach => "Protect and detach",
            Self::KeepRunning if no_mux => "Protect and keep running",
            Self::KeepRunning => "Protect, keep, and share",
            Self::Share => "Protect and share",
        }
    }
}

/// What opening a multiplexer-held session's sealed key produced.
///
/// The passphrase case is a state, not an error, because it is answerable: the
/// window asks for the identity's passphrase and comes back. Collapsing it into
/// a failure is what made an encrypted SSH identity look like the wrong key.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
pub(crate) enum SealedKeyRecovery {
    /// Not protected this way, or not something this window can open — the
    /// caller attaches without a secret and lets the daemon decide.
    NotSealed,
    /// The identity file is itself encrypted and no passphrase was supplied yet.
    NeedsIdentityPassphrase,
    Recovered(SessionSecret),
}

/// As [`SealedKeyRecovery`], for a session another Zetta process is holding,
/// where the proof rather than the secret is what the caller needs.
#[cfg_attr(not(feature = "session-persistence"), allow(dead_code))]
pub(crate) enum SealedKeyAuthorization {
    NotSealed,
    NeedsIdentityPassphrase,
    Authorized(VerifiedSession),
}

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
        match self.existing_protection(tab_id) {
            Some(authentication) => {
                self.apply_protected_session_action(tab_id, action, authentication, window, cx)
            }
            #[cfg(feature = "session-persistence")]
            None if self.auto_protect.is_some() => {
                self.protect_automatically_and_then(tab_id, action, window, cx)
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
    /// An `Err` is a configured identity that could not open an envelope, which
    /// is worth saying plainly rather than turning into an unanswerable prompt.
    #[cfg(feature = "session-persistence")]
    fn recovered_session_secret(
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
    fn recovered_session_secret(
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
    fn authorization_from_sealed_key(
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
    fn authorization_from_sealed_key(
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
                self.detach_tab_by_id(tab_id, authentication, window, cx)
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
        // stops dispatching — which is why `Ctrl-Shift-K` could turn sharing on
        // and then never turn it off, while the same action from the tab's
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
        let (Some(runtime), Some(session_id)) =
            (self.mux.clone(), self.mux_panes.session_id(tab.id))
        else {
            return Ok(false);
        };
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
    fn shared_session_refresh(
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
        let runtime = if self
            .mux
            .as_ref()
            .is_some_and(|runtime| runtime.retention() == zmux::retention::Retention::Disk)
        {
            self.mux.clone().expect("disk runtime was present")
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
        let (identity_paths, identity_passphrases) = identities
            .map(|identities| (identities.paths, identities.passphrases))
            .unwrap_or_else(|| (self.disk_resume_identity_paths(), Vec::new()));
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
        // The daemon consumes the encrypted record before returning. Refresh
        // the process-wide picker even if rebuilding the tab below fails, so
        // the UI cannot keep offering a record that no longer exists.
        cx.defer(refresh_process_background_sessions);
        let summary_title = persisted.summary.title.clone();
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
        // A disk record may have been written by a daemon that was alive when
        // the tab detached. Its pane IDs and exit details describe that old
        // process tree, not a process this restore is allowed to revive.
        // The process cannot be revived from a disk snapshot, but the saved
        // screen is kept below as a read-only terminal so resume does not lose
        // the useful state the record actually contains.
        state.keep_running = false;
        state.shared = false;
        for pane in &mut state.panes {
            pane.mux_pane_id = None;
            pane.exit = None;
            pane.base_exited = false;
            pane.pending_command = None;
            pane.stack.clear();
            pane.selected_stacked = None;
        }
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let profiles = self.profiles.clone();
        let mut tab = match state.into_tab(tab_id, |name| {
            profiles
                .iter()
                .find(|profile| profile.name == name)
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
        // The catalog title is what the user selected. A display-only terminal
        // has no process to emit a fresh title, so preserve it on the restored
        // tab just as the live multiplexer attach path does.
        if tab.custom_title.is_none() && !summary_title.is_empty() {
            tab.process_title = Some(summary_title);
        }
        let settings = TerminalSpawnSettings::current(cx);
        for snapshot in snapshots {
            if snapshot.bytes.is_empty() {
                continue;
            }
            let builder = TerminalBuilder::new_display_only(
                settings.cursor_shape,
                settings.alternate_scroll,
                settings.max_scroll_history_lines,
                cx.entity_id().as_u64(),
                cx.background_executor(),
                PathStyle::local(),
            )
            .with_replay(snapshot.bytes);
            let terminal = cx.new(|cx| builder.subscribe(cx));
            if let Some(pane) = tab.pane_mut(snapshot.pane_id) {
                pane.terminal = Some(terminal);
                pane.error = None;
                pane.base_exited = true;
            }
        }
        for pane in &mut tab.panes {
            pane.pending_command = None;
            if pane.terminal.is_none() {
                pane.error = Some(format!("Run: profile {}", pane.profile.name));
            }
        }
        self.attach_reconnected_tab(tab, true, window, cx);
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
                source.record_failed_authentication(session_id)
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
                source.clear_failed_authentications(session_id)
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

    pub(crate) fn take_background_session_by_id(
        &mut self,
        session_id: u64,
        authorization: Option<&VerifiedSession>,
        cx: &mut Context<Self>,
    ) -> Option<Tab> {
        let index = self
            .background_sessions
            .iter()
            .position(|tab| tab.id == session_id)?;
        match (
            self.background_sessions.authentication_at(index),
            authorization,
        ) {
            (None, None) => {}
            (Some(expected), Some(supplied)) if expected.authorizes(supplied) => {}
            _ => return None,
        }
        let tab = self.background_sessions.reconnect_at(index)?;
        self.publish_background_session_catalog(cx);
        Some(tab)
    }

    /// Gives a tab arriving from elsewhere the project of the pane this window is
    /// on, the way every other route to a new pane does.
    ///
    /// A pane's theme is resolved once, when its view is built, and it resolves
    /// through the pane's project. A tab attached from the multiplexer or handed
    /// over by another window had no project recorded for its panes at all, so
    /// the theme fell back to the default and only corrected itself when
    /// detection eventually reported a root — which is the next time the pane's
    /// title changes, so a joined session showing a full-screen program kept the
    /// wrong theme until that program quit.
    ///
    /// Inheriting is the same answer `split_pane` and the command panes use: the
    /// window's own context is the best guess, and detection still corrects it
    /// if the session's shell turns out to be somewhere else.
    fn inherit_project_for_incoming_panes(&mut self, tab: &Tab) {
        let Some(source) = self
            .tabs
            .get(self.active_tab)
            .map(|active| active.active_pane)
        else {
            return;
        };
        inherit_project_for_panes(&mut self.projects, source, tab);
    }

    pub(crate) fn attach_reconnected_tab(
        &mut self,
        mut tab: Tab,
        transferred: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if transferred {
            let tab_id = self.next_tab_id;
            self.next_tab_id += 1;
            tab.reassign_ids(tab_id, &mut self.next_pane_id);
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
        let tab_id = tab.id;
        self.inherit_project_for_incoming_panes(&tab);
        let panes = tab
            .panes
            .iter()
            .flat_map(|pane| {
                let project = self.projects.config_for_pane(pane.id).cloned();
                let base = pane
                    .terminal
                    .clone()
                    .filter(|_| pane.exit.is_none())
                    .map(|terminal| {
                        (
                            pane.id,
                            None,
                            terminal,
                            resolve_project_profile_theme(&pane.profile, project.as_deref(), cx),
                        )
                    });
                let stacked = pane
                    .stack
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            pane.id,
                            Some(entry.id),
                            entry.terminal.clone()?,
                            resolve_project_profile_theme(&entry.profile, project.as_deref(), cx),
                        ))
                    })
                    .collect::<Vec<_>>();
                base.into_iter().chain(stacked)
            })
            .collect::<Vec<_>>();
        self.active_tab = insert_tab_in_pin_order(&mut self.tabs, tab);

        for (pane_id, stack_id, terminal, terminal_theme) in panes {
            match terminal_theme {
                Ok(theme) => {
                    let display_only = !terminal.read(cx).is_pty();
                    let view = cx.new(|cx| {
                        TerminalView::new_with_theme(terminal.clone(), theme, window, cx)
                    });
                    if display_only {
                        view.update(cx, |view, cx| view.set_input_enabled(false, cx));
                    }
                    if let Some(entry_id) = stack_id {
                        self.connect_stacked_terminal_view(
                            tab_id, pane_id, entry_id, view, window, cx,
                        );
                    } else {
                        self.connect_terminal_view(tab_id, pane_id, view, window, cx);
                    }
                }
                Err(error) => {
                    if let Some(entry_id) = stack_id {
                        if let Some(entry) =
                            self.tabs[self.active_tab]
                                .pane_mut(pane_id)
                                .and_then(|pane| {
                                    pane.stack
                                        .entries
                                        .iter_mut()
                                        .find(|entry| entry.id == entry_id)
                                })
                        {
                            entry.error =
                                Some(format!("Could not reattach terminal view: {error:#}"));
                            entry.state = StackedPaneState::Failed;
                        }
                    } else if let Some(pane) = self.tabs[self.active_tab].pane_mut(pane_id) {
                        pane.error = Some(format!("Could not reattach terminal view: {error:#}"));
                    }
                }
            }
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    pub(crate) fn process_background_session_picker_entries(
        &self,
        cx: &App,
    ) -> Arc<[ProcessBackgroundSessionEntry]> {
        let own_runner = self.background_sessions.runner_id();
        if cx.has_global::<ZettaProcessState>() {
            let entries = cx
                .global::<ZettaProcessState>()
                .background_session_entries
                .clone();
            let shown_here = |entry: &ProcessBackgroundSessionEntry| {
                session_is_already_shown_here(&self.mux_panes, entry, own_runner)
            };
            if !entries.iter().any(shown_here) {
                return entries;
            }
            return entries
                .iter()
                .filter(|entry| !shown_here(entry))
                .cloned()
                .collect::<Vec<_>>()
                .into();
        }
        self.background_session_picker_entries
            .iter()
            .map(|(session_id, title, details)| {
                (own_runner, *session_id, title.clone(), details.clone())
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn picker_entries_from_summaries(
        sessions: &[BackgroundSessionSummary],
    ) -> Vec<(u64, String, String)> {
        sessions
            .iter()
            .rev()
            .map(|session| {
                if session.authentication_required {
                    return (
                        session.id,
                        "Protected session".to_owned(),
                        format!("Session {} · protected", session.id),
                    );
                }
                let mut applications = Vec::new();
                for pane in &session.panes {
                    if !applications.contains(&pane.application) {
                        applications.push(pane.application.clone());
                    }
                }
                let pane_count = session.panes.len();
                let mut details = format!(
                    "Session {} · {pane_count} pane{}",
                    session.id,
                    if pane_count == 1 { "" } else { "s" }
                );
                if !applications.is_empty() {
                    details.push_str(" · ");
                    details.push_str(&applications.join(", "));
                }
                // A session another window is showing does not reconnect, it is
                // *joined*: the multiplexer asks that window to hand its
                // terminals over and both then see the same panes. Listing it
                // identically to a detached session made that look like an
                // ordinary reconnect right up until the other window changed.
                if session.held {
                    details.push_str(" · in use elsewhere");
                }
                if let Some(exit) = session.panes.iter().find_map(|pane| pane.exit.as_ref()) {
                    details.push_str(" · failed: ");
                    details.push_str(&exit.reason_text());
                }
                (session.id, session.title.clone(), details)
            })
            .collect()
    }

    pub(crate) fn observe_background_terminal(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if !self.background_observed_panes.insert(pane_id) {
            return;
        }
        cx.subscribe(
            &terminal,
            move |this, _, event: &TerminalEvent, cx| match event {
                TerminalEvent::TerminalExited(exit)
                    if exit.is_unexpected()
                        && this.retain_unexpected_terminal_exit(tab_id, pane_id, exit, cx) =>
                {
                    this.publish_background_session_catalog(cx);
                }
                event if terminal_event_requires_worktree_detection(event) => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.publish_background_session_catalog(cx);
                }
                TerminalEvent::CloseTerminal
                    if !this.retain_background_stacked_entries_after_base_exit(pane_id, cx) =>
                {
                    this.reap_background_pane(pane_id, cx);
                }
                _ => {}
            },
        )
        .detach();
    }

    fn retain_background_stacked_entries_after_base_exit(
        &mut self,
        pane_id: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(pane) = self
            .background_sessions
            .iter_mut()
            .find_map(|tab| tab.pane_mut(pane_id))
        else {
            return false;
        };
        if pane.stack.is_empty() {
            return false;
        }

        pane.terminal = None;
        pane.base_exited = true;
        pane.stack.select_after_base_exit();
        self.background_observed_panes.remove(&pane_id);
        self.publish_background_session_catalog(cx);
        true
    }

    pub(crate) fn observe_background_stacked_terminal(
        &mut self,
        pane_id: u64,
        entry_id: u64,
        terminal: Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        if !self.background_observed_panes.insert(entry_id) {
            return;
        }
        cx.subscribe(
            &terminal,
            move |this, _, event: &TerminalEvent, cx| match event {
                TerminalEvent::TaskFinished { exit_code } => {
                    let Some(tab_id) = this
                        .background_sessions
                        .iter()
                        .find(|tab| {
                            tab.pane(pane_id).is_some_and(|pane| {
                                pane.stack.entries.iter().any(|entry| entry.id == entry_id)
                            })
                        })
                        .map(|tab| tab.id)
                    else {
                        return;
                    };
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
                TerminalEvent::CloseTerminal => {
                    let Some(tab_id) = this
                        .background_sessions
                        .iter()
                        .find(|tab| {
                            tab.pane(pane_id).is_some_and(|pane| {
                                pane.stack.entries.iter().any(|entry| entry.id == entry_id)
                            })
                        })
                        .map(|tab| tab.id)
                    else {
                        return;
                    };
                    let removed = this
                        .background_sessions
                        .iter_mut()
                        .find(|tab| tab.id == tab_id)
                        .and_then(|tab| tab.pane_mut(pane_id))
                        .and_then(|pane| pane.stack.remove(entry_id));
                    if removed.is_some() {
                        this.background_observed_panes.remove(&entry_id);
                        this.publish_background_session_catalog(cx);
                    }
                }
                _ => {}
            },
        )
        .detach();
    }

    fn reap_background_pane(&mut self, pane_id: u64, cx: &mut Context<Self>) {
        let Some(removed_pane_ids) =
            remove_exited_background_pane(&mut self.background_sessions, pane_id)
        else {
            return;
        };
        for pane_id in removed_pane_ids {
            self.transient_pane_themes
                .retain(|(id, _), _| *id != pane_id);
            self.background_observed_panes.remove(&pane_id);
        }
        self.publish_background_session_catalog(cx);
        if self.background_sessions.is_empty() {
            cx.defer(prune_empty_dormant_runners);
        }
    }

    fn schedule_background_process_refresh(&mut self, cx: &mut Context<Self>) {
        if self.background_process_refresh_running || self.background_sessions.is_empty() {
            return;
        }
        self.background_process_refresh_running = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(BACKGROUND_PROCESS_REFRESH_INTERVAL).await;
                let keep_refreshing = this
                    .update(cx, |this, cx| {
                        if this.background_sessions.is_empty() {
                            this.background_process_refresh_running = false;
                            return false;
                        }
                        for terminal in this
                            .background_sessions
                            .iter()
                            .flat_map(|tab| tab.panes.iter())
                            .flat_map(|pane| {
                                pane.terminal.iter().cloned().chain(
                                    pane.stack
                                        .entries
                                        .iter()
                                        .filter_map(|entry| entry.terminal.clone()),
                                )
                            })
                        {
                            terminal.update(cx, Terminal::refresh_foreground_process);
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_refreshing {
                    break;
                }
            }
        })
        .detach();
    }

    pub(crate) fn publish_background_session_catalog(&mut self, cx: &mut Context<Self>) {
        let sessions = self
            .background_sessions
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                self.background_session_summary(
                    tab,
                    self.background_sessions.authentication_at(index).is_some(),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        self.background_session_picker_entries = Self::picker_entries_from_summaries(&sessions);
        if let Err(error) = self.background_sessions.publish(sessions) {
            eprintln!("Could not publish background session catalog: {error:#}");
        }
        cx.defer(refresh_process_background_sessions);
    }

    fn background_session_summary(
        &self,
        tab: &Tab,
        authentication_required: bool,
        cx: &App,
    ) -> BackgroundSessionSummary {
        let title = self.background_session_title(tab, cx);
        let panes = tab
            .panes
            .iter()
            .map(|pane| {
                let (terminal_title, foreground_command) = pane
                    .terminal
                    .as_ref()
                    .map(|terminal| {
                        let terminal = terminal.read(cx);
                        (
                            Some(terminal.title(false)),
                            terminal.foreground_process_command_line(),
                        )
                    })
                    .unwrap_or_default();
                let working_directory = pane.working_directory(cx);
                let state = if pane.error.is_some() || pane.exit.is_some() {
                    BackgroundPaneState::Failed
                } else if pane.terminal.is_some() {
                    BackgroundPaneState::Running
                } else {
                    BackgroundPaneState::Starting
                };
                let (program, arguments) = pane.profile.command.program_and_args();
                let configured_command = std::iter::once(program)
                    .chain(arguments.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" ");
                let application = application_from_command_line(foreground_command.as_deref())
                    .unwrap_or_else(|| {
                        pane.generated_label
                            .as_deref()
                            .and_then(|label| {
                                if label.starts_with("HTTP: ") {
                                    Some("Zetta HTTP server")
                                } else if label.starts_with("TFTP: ") {
                                    Some("Zetta TFTP server")
                                } else if label.starts_with("Serial: ") {
                                    Some("Serial console")
                                } else {
                                    None
                                }
                            })
                            .map(str::to_owned)
                            .unwrap_or_else(|| pane.profile.command.program_and_args().0)
                    });
                BackgroundPaneSummary {
                    id: pane.id,
                    label: pane.label(),
                    profile: pane.profile.name.clone(),
                    configured_command,
                    application,
                    foreground_command,
                    terminal_title,
                    working_directory,
                    state,
                    exit: pane.exit.clone(),
                }
            })
            .collect();
        BackgroundSessionSummary {
            id: tab.id,
            title,
            authentication_required,
            active_pane: tab.active_pane,
            layout: background_pane_layout(&tab.layout),
            panes,
            held: false,
            // The multiplexer decides whose a session is; a client describing
            // one only says what it contains.
            scoped_to: None,
            // As with the scope: the multiplexer publishes the sealed key from
            // the protection it was given, so a summary never carries one.
            key_envelope: None,
        }
    }

    fn background_session_title(&self, tab: &Tab, cx: &App) -> String {
        resolve_tab_title(tab, || {
            tab.active_terminal()
                .map(|terminal| terminal.read(cx).title(false).into())
                .unwrap_or_else(|| format!("Tab {}", tab.id).into())
        })
        .to_string()
    }

    fn connect_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let visible = self.tabs.get(self.active_tab).is_some_and(|tab| {
            tab.id == tab_id
                && tab.pane_is_visible(pane_id)
                && tab
                    .pane(pane_id)
                    .is_some_and(|pane| pane.stack.selected_is_base())
        });
        let terminal = view.read(cx).terminal().clone();
        let display_only = !terminal.read(cx).is_pty();
        terminal.update(cx, |terminal, cx| terminal.set_ui_visible(visible, cx));
        if self.shared_panes.contains_key(&pane_id) {
            self.subscribe_shared_pane_size(pane_id, &terminal, window, cx);
        }

        let pane_label = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.generated_label.as_deref());
        let is_http_server = cfg!(feature = "http-server")
            && pane_label.is_some_and(|label| label.starts_with("HTTP: "));
        let is_tftp_server = cfg!(feature = "tftp-server")
            && pane_label.is_some_and(|label| label.starts_with("TFTP: "));
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => this.terminal_closed(tab_id, pane_id, window, cx),
                TerminalViewEvent::TitleChanged => {
                    this.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
                    this.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
                    cx.notify();
                }
                TerminalViewEvent::Input(input)
                    if server_input_stops_server(input, is_http_server, is_tftp_server) =>
                {
                    this.terminal_closed(tab_id, pane_id, window, cx);
                }
                TerminalViewEvent::Input(input) => {
                    this.broadcast_input(tab_id, pane_id, input, cx);
                }
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
        let focus_handle = view.focus_handle(cx);
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this
                .tabs
                .iter_mut()
                .find(|tab| tab.id == tab_id)
                .filter(|tab| tab.pane(pane_id).is_some_and(|pane| !pane.base_exited))
            {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Base);
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        let emit_input_events = is_http_server
            || is_tftp_server
            || self
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .is_some_and(|tab| tab.broadcast_input);
        view.update(cx, |view, _| view.set_emit_input_events(emit_input_events));
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            pane.view = Some(view);
            pane.error = None;
            pane.exit = None;
            if !display_only {
                pane.base_exited = false;
            }
        }
        self.schedule_worktree_detection_for_pane(tab_id, pane_id, cx);
        self.schedule_project_detection_for_pane(tab_id, pane_id, window, cx);
    }

    fn connect_stacked_terminal_view(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        entry_id: u64,
        view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.configure_terminal_view_silent_mode(tab_id, &view, cx);
        let visible = self.tabs.get(self.active_tab).is_some_and(|tab| {
            tab.id == tab_id
                && tab.pane_is_visible(pane_id)
                && tab.pane(pane_id).is_some_and(|pane| {
                    pane.stack.selected == PaneStackSelection::Stacked(entry_id)
                })
        });
        let terminal = view.read(cx).terminal().clone();
        terminal.update(cx, |terminal, cx| terminal.set_ui_visible(visible, cx));
        cx.subscribe_in(
            &terminal,
            window,
            move |this, terminal, event: &TerminalEvent, _window, cx| match event {
                TerminalEvent::TaskFinished { exit_code } => {
                    this.stacked_task_finished(tab_id, pane_id, entry_id, *exit_code, cx);
                }
                TerminalEvent::ResizeRequested { .. } => {
                    terminal.update(cx, |terminal, _| terminal.truncate_on_next_resize());
                }
                _ => {}
            },
        )
        .detach();
        cx.subscribe_in(
            &view,
            window,
            move |this, _, event, window, cx| match event {
                TerminalViewEvent::Close => {
                    this.stacked_terminal_closed(tab_id, pane_id, entry_id, window, cx);
                }
                TerminalViewEvent::TitleChanged => cx.notify(),
                TerminalViewEvent::Input(_) => {}
                TerminalViewEvent::OpenEditor(request) => {
                    this.open_editor_in_new_pane(tab_id, pane_id, request.clone(), window, cx);
                }
            },
        )
        .detach();
        let input_enabled = self.terminal_input_enabled();
        view.update(cx, |view, cx| {
            view.set_emit_input_events(false);
            view.set_input_enabled(input_enabled, cx);
        });
        let focus_handle = view.focus_handle(cx);
        cx.on_focus_in(&focus_handle, window, move |this, window, cx| {
            if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.activate_stack_entry(pane_id, PaneStackSelection::Stacked(entry_id));
                cx.notify();
            }
            this.activate_current_project(window, cx);
            this.clear_active_tab_attention_if_focused(window, cx);
        })
        .detach();
        if let Some(entry) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane_mut(pane_id))
            .and_then(|pane| {
                pane.stack
                    .entries
                    .iter_mut()
                    .find(|entry| entry.id == entry_id)
            })
        {
            entry.view = Some(view);
        }
    }
}

#[inline]
fn server_input_stops_server(
    input: &TerminalInput,
    is_http_server: bool,
    is_tftp_server: bool,
) -> bool {
    (is_http_server || is_tftp_server) && byte_stream_pane::ctrl_c_interrupts_byte_stream(input)
}

#[cfg(test)]
#[path = "tests/background_session_ui.rs"]
mod tests;

impl Zetta {
    /// Gives a detached tab to the multiplexer to hold.
    ///
    /// Returns `false` when explicit `--no-mux` mode selected the legacy
    /// in-process owner. Normal launches return an error when a pane cannot be
    /// handed to the daemon, so backgrounding never silently changes its
    /// lifetime guarantees.
    fn hand_session_to_multiplexer(
        &mut self,
        tab: &mut Tab,
        authentication: Option<&SessionAuthentication>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<bool> {
        if self.no_mux {
            return Ok(false);
        }
        let (Some(runtime), Some(session_id)) =
            (self.mux.clone(), self.mux_panes.session_id(tab.id))
        else {
            anyhow::bail!(
                "the tab has no daemon-owned session; start Zetta with --no-mux to use local session ownership"
            );
        };

        // Stacked terminals are task terminals, not interactive terminals, and
        // cannot be reattached yet. Stop their readers before releasing their
        // daemon panes, then leave their durable entries for restore_stack to
        // mark as failed instead of publishing dangling pane ids.
        let stacked_mux_panes = tab
            .panes
            .iter()
            .flat_map(|pane| pane.stack.entries.iter().map(|entry| entry.id))
            .filter_map(|entry_id| {
                self.mux_panes
                    .mux_pane_id(entry_id)
                    .map(|id| (entry_id, id))
            })
            .collect::<Vec<_>>();
        for (entry_id, mux_pane_id) in &stacked_mux_panes {
            if let Some(terminal) = tab.panes.iter().find_map(|pane| {
                pane.stack
                    .entries
                    .iter()
                    .find(|entry| entry.id == *entry_id)
                    .and_then(|entry| entry.terminal.clone())
            }) {
                terminal
                    .update(cx, |terminal, _| terminal.stop_pty_loop())
                    .context("stopping a stacked terminal before detach")?;
            }
            runtime
                .client()
                .close_pane(session_id, *mux_pane_id)
                .with_context(|| format!("closing stacked daemon pane {mux_pane_id}"))?;
            self.mux_panes.forget_pane(*entry_id);
        }

        // The screen as the user last saw it. The multiplexer keeps a grid of its
        // own, but it has only been reading this pane while nobody was showing
        // it — everything on screen now was drawn here — so the handover starts
        // by giving it that screen to carry on from.
        for pane in &tab.panes {
            if let Some(terminal) = &pane.terminal {
                terminal
                    .update(cx, |terminal, _| terminal.stop_pty_loop())
                    .context("stopping a terminal before detach")?;
            }
        }
        let snapshots = if runtime.retention().keeps_snapshot() {
            tab.panes
                .iter()
                .filter_map(|pane| {
                    let mux_pane_id = self.mux_panes.mux_pane_id(pane.id)?;
                    let terminal = pane.terminal.as_ref()?;
                    Some((mux_pane_id, terminal.read(cx).ansi_snapshot(SNAPSHOT_LINES)))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let (summary, state) =
            self.session_publication(tab, session_id, authentication.is_some(), cx)?;
        runtime
            .client()
            .detach(session_id, summary, state, authentication, snapshots)?;
        Ok(true)
    }

    /// What the multiplexer publishes for a tab's session: the summary the
    /// catalog lists, and the state another window rebuilds the tab from when it
    /// attaches or joins.
    ///
    /// Shared by detaching and sharing, because both are the same publication —
    /// they differ in what happens to the terminals, not in how the session is
    /// described.
    fn session_publication(
        &self,
        tab: &Tab,
        session_id: u64,
        authentication_required: bool,
        cx: &App,
    ) -> anyhow::Result<(BackgroundSessionSummary, serde_json::Value)> {
        let mut summary = self.background_session_summary(tab, authentication_required, cx);
        // The catalog and the daemon address a session by the id the
        // multiplexer assigned it, not by this window's tab id. Publishing the
        // tab id instead made the catalog list a session the daemon would then
        // claim did not exist, because it looks sessions up under the mux id.
        summary.id = session_id;
        let state = crate::session_state::TabState::from_tab(tab, self.mux_panes.ids());
        let state = serde_json::to_value(state).context("serializing the session's tab state")?;
        Ok((summary, state))
    }
}

/// How much scrollback is handed over with a detached pane. Enough to restore a
/// screen and the context above it, without making detaching a tab an expensive
/// operation on a session that has been running for hours.
const SNAPSHOT_LINES: usize = 2_000;

impl Zetta {
    /// Takes a session the multiplexer is holding and shows it in this window.
    ///
    /// Each pane's terminal comes back as the same PTY it was detached with, so
    /// the processes never restarted and the scrollback the multiplexer
    /// retained is replayed into the grid before the pane is shown.
    pub(crate) fn attach_multiplexer_session(
        &mut self,
        session_id: u64,
        secret: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<AttachOutcomeSummary> {
        if self.mux.is_none() {
            // Attaching may be the first thing this window does, before any
            // pane has been spawned through the multiplexer.
            let retention = self
                .launch_config
                .sessions
                .to_zmux_retention()
                .context("configuring session retention")?;
            #[cfg(feature = "session-persistence")]
            let runtime = MuxRuntime::connect_with_retention_and_persistence(
                retention,
                self.launch_config.sessions.to_zmux_persistence(),
            )?;
            #[cfg(not(feature = "session-persistence"))]
            let runtime = MuxRuntime::connect_with_retention(retention)?;
            self.mux = Some(runtime);
        }
        let Some(runtime) = self.mux.clone() else {
            anyhow::bail!("no multiplexer is running");
        };

        // Starts from the session: which panes it has is part of what a
        // protected session's secret protects, so the multiplexer resolves the
        // first pane itself once the secret has been checked.
        let first = runtime.client().attach(
            session_id,
            None,
            secret.as_ref().map(|secret| secret.expose().to_owned()),
        )?;
        let (pane, state, summary) = match first {
            zmux::client::AttachOutcome::Attached {
                pane,
                state,
                summary,
            } => (AttachedPaneKind::Exclusive(pane), state, summary),
            zmux::client::AttachOutcome::SharedAttached {
                pane,
                state,
                summary,
            } => (AttachedPaneKind::Shared(pane), state, summary),
            zmux::client::AttachOutcome::AuthenticationRequired => {
                return Ok(AttachOutcomeSummary::AuthenticationRequired);
            }
            zmux::client::AttachOutcome::AuthenticationFailed => {
                return Ok(AttachOutcomeSummary::AuthenticationFailed);
            }
        };

        // A session the multiplexer holds but that has never been detached or
        // shared has published no layout, so there is nothing to rebuild a tab
        // from. Reachable by asking for a session by id, and worth saying
        // plainly: the raw serde error for this is "invalid type: null,
        // expected struct TabState", which describes the symptom and not the
        // cause.
        anyhow::ensure!(
            !state.is_null(),
            "session {session_id} has not published a layout, so it cannot be attached; share or \
             detach it from the window showing it first"
        );
        let state: crate::session_state::TabState =
            serde_json::from_value(state).context("reading the session's tab state")?;
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let profiles = self.profiles.clone();
        let mut tab = state.clone().into_tab(tab_id, |name| {
            profiles
                .iter()
                .find(|profile| profile.name == name)
                .cloned()
                .unwrap_or_else(|| Profile {
                    // A profile the configuration no longer describes. Keeping
                    // the name means the pane still reports what it was
                    // started with rather than silently claiming another
                    // profile's command.
                    name: name.to_owned(),
                    command: task::Shell::System,
                    theme: None,
                    dark_theme: None,
                    icon: ProfileIcon::default(),
                })
        })?;
        tab.id = tab_id;
        // The catalog's title is what the user picked this session out of the
        // list by. A restored tab whose terminal has not yet reprinted its
        // title would otherwise fall back to a generic name, so the tab they
        // chose as "htop" comes back called "Terminal".
        if tab.custom_title.is_none() && !summary.title.is_empty() {
            tab.process_title = Some(summary.title.clone());
        }

        self.mux_panes.adopt_session(tab_id, session_id);
        // Pair the pane the multiplexer chose with the tab pane that named it,
        // rather than assuming it was the first one listed.
        let first_pane = state
            .panes
            .iter()
            .find(|candidate| candidate.mux_pane_id == Some(pane.pane_id()))
            .map(|candidate| candidate.id)
            .unwrap_or(state.panes[0].id);
        let mut attached = vec![(first_pane, pane)];
        for pane_state in state.panes.iter().filter(|pane| pane.id != first_pane) {
            let Some(mux_pane_id) = pane_state.mux_pane_id else {
                continue;
            };
            match runtime.client().attach(
                session_id,
                Some(mux_pane_id),
                secret.as_ref().map(|secret| secret.expose().to_owned()),
            )? {
                zmux::client::AttachOutcome::Attached { pane, .. } => {
                    attached.push((pane_state.id, AttachedPaneKind::Exclusive(pane)))
                }
                zmux::client::AttachOutcome::SharedAttached { pane, .. } => {
                    attached.push((pane_state.id, AttachedPaneKind::Shared(pane)))
                }
                // The session authenticated a moment ago, so this can only mean
                // it was taken in between. Show what was attached rather than
                // dropping the whole tab.
                _ => break,
            }
        }

        // The tab arrives carrying the pane ids of the window that published it,
        // and every window-scoped registry here is keyed by pane id alone:
        // `mux_panes`, `shared_panes`, pane controls, the project registry. A tab
        // this window already has will very often own the same ids — both windows
        // number from one — so the two tabs end up sharing those entries, and
        // closing *either* takes the other's bookkeeping with it. That is how
        // closing an unrelated tab left a joined session's pane with no exit
        // reporter and no shared entry: nothing could ever tell it its shell had
        // ended, so `exit` and Ctrl-D hung it.
        //
        // Renumbering into this window's own counter is what the in-process
        // transfer path already does, for exactly this reason.
        let pane_ids = tab.reassign_ids(tab_id, &mut self.next_pane_id);
        let attached = attached
            .into_iter()
            .filter_map(|(pane_id, kind)| Some((pane_ids.get(&pane_id).copied()?, kind)))
            .collect::<Vec<_>>();
        self.inherit_project_for_incoming_panes(&tab);

        self.build_attached_panes(&mut tab, session_id, attached, &runtime, window, cx);
        self.active_tab = insert_tab_in_pin_order(&mut self.tabs, tab);
        // The pane views were built before the tab was inserted. Wire them up
        // now that it is: without the view subscription, the terminal's
        // `CloseTerminal` (a shell that ended in a restored tab) would never
        // reach `terminal_closed` and the tab would stay open.
        let tab_id = self.tabs[self.active_tab].id;
        let views = self.tabs[self.active_tab]
            .panes
            .iter()
            .filter_map(|pane| pane.view.clone().map(|view| (pane.id, view)))
            .collect::<Vec<_>>();
        for (pane_id, view) in views {
            self.connect_terminal_view(tab_id, pane_id, view, window, cx);
        }
        self.focus_active(window, cx);
        cx.notify();
        Ok(AttachOutcomeSummary::Attached)
    }
}

pub(crate) enum AttachOutcomeSummary {
    Attached,
    AuthenticationRequired,
    AuthenticationFailed,
}

/// What the multiplexer handed over for one pane.
///
/// An exclusive pane's terminal reads the pty descriptor; a shared pane's
/// terminal reads the multiplexer's relay over the connection that stayed
/// open. Both are ordinary terminals from here on — the difference is only
/// in what feeds them.
pub(crate) enum AttachedPaneKind {
    Exclusive(zmux::client::AttachedPane),
    Shared(zmux::client::SharedPane),
}

impl AttachedPaneKind {
    fn pane_id(&self) -> u64 {
        match self {
            AttachedPaneKind::Exclusive(pane) => pane.pane_id,
            AttachedPaneKind::Shared(pane) => pane.pane_id(),
        }
    }
}

impl Zetta {
    /// Turns the descriptors the multiplexer handed over into live terminals.
    ///
    /// Each becomes an ordinary terminal view: from here on the pane behaves
    /// exactly like one this process spawned, because it is reading the same
    /// kind of descriptor through the same event loop.
    fn build_attached_panes(
        &mut self,
        tab: &mut Tab,
        session_id: u64,
        attached: Vec<(u64, AttachedPaneKind)>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = TerminalSpawnSettings::current(cx);
        for (pane_id, attached) in attached {
            let Some(pane) = tab.pane(pane_id) else {
                continue;
            };
            let profile = pane.profile.clone();
            let project = self.projects.config_for_pane(pane_id).cloned();
            let theme = match resolve_project_profile_theme(&profile, project.as_deref(), cx) {
                Ok(theme) => theme,
                Err(error) => {
                    if let Some(pane) = tab.pane_mut(pane_id) {
                        pane.error = Some(format!("Could not apply the pane's theme: {error:#}"));
                    }
                    continue;
                }
            };

            let options = terminal::AttachedOptions {
                shell: profile.command.clone(),
                env: HashMap::default(),
                cursor_shape: settings.cursor_shape,
                alternate_scroll: settings.alternate_scroll,
                max_scroll_history_lines: settings.max_scroll_history_lines,
                path_hyperlink_regexes: Vec::new(),
                path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
                window_id: cx.entity_id().as_u64(),
            };
            let (mux_pane_id, built, child_events, shared) = match attached {
                AttachedPaneKind::Exclusive(attached) => {
                    let mux_pane_id = attached.pane_id;
                    match TerminalBuilder::new_attached(
                        crate::mux::attached_pane_handover(attached, runtime.client().clone()),
                        options,
                        cx.background_executor(),
                        PathStyle::local(),
                    ) {
                        Ok(built) => (
                            mux_pane_id,
                            Some(built.builder),
                            Some(built.child_events),
                            None::<Arc<zmux::client::SharedPane>>,
                        ),
                        Err(error) => {
                            if let Some(pane) = tab.pane_mut(pane_id) {
                                pane.error =
                                    Some(format!("Could not reattach the terminal: {error:#}"));
                            }
                            (
                                mux_pane_id,
                                None,
                                None,
                                None::<Arc<zmux::client::SharedPane>>,
                            )
                        }
                    }
                }
                AttachedPaneKind::Shared(pane) => {
                    let pane = Arc::new(pane);
                    let mux_pane_id = pane.pane_id();
                    // The replay goes to `with_replay` below and *only* there.
                    // Prefixing the reader with it as well wrote the restored
                    // screen twice: once here, into a grid still at its
                    // placeholder size, where all but the last few lines were lost
                    // and the survivors landed at the top — and once properly after
                    // the pane was laid out. A full-screen program redraws only
                    // what it thinks has changed, so the stray first rows stayed
                    // there: htop's footer, painted across the top of the window.
                    let reader: Box<dyn std::io::Read + Send> = Box::new(pane.reader());
                    let writer: Box<dyn std::io::Write + Send> =
                        Box::new(SharedPaneWriter { pane: pane.clone() });
                    let built = TerminalBuilder::new_byte_stream(
                        reader,
                        writer,
                        tab.process_title.clone().unwrap_or_default(),
                        settings.cursor_shape,
                        settings.alternate_scroll,
                        settings.max_scroll_history_lines,
                        cx.entity_id().as_u64(),
                        cx.background_executor(),
                        PathStyle::local(),
                    )
                    .with_replay(pane.replay.clone())
                    .with_pty_control(crate::mux::mux_pty_control(
                        runtime.client().clone(),
                        session_id,
                        mux_pane_id,
                    ));
                    (mux_pane_id, Some(built), None, Some(pane))
                }
            };
            let Some(built) = built else {
                continue;
            };
            if let Some(child_events) = child_events {
                // Only the multiplexer is the process's parent, so this is the one
                // route by which the terminal can learn that it ended.
                runtime.reporters().register(mux_pane_id, child_events);
            }
            self.mux_panes.record(pane_id, mux_pane_id);
            if let Some(shared) = &shared {
                self.register_shared_pane(
                    tab.id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    shared,
                    runtime,
                    window,
                    cx,
                );
            } else {
                // This window now holds the descriptor, so it is the one the
                // multiplexer will ask to hand the pane over when a third
                // window attaches. Without this the request went nowhere and
                // that attach waited out the whole handover timeout.
                self.watch_for_revoke(
                    tab.id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    runtime,
                    window,
                    cx,
                );
            }

            let terminal = cx.new(|cx| built.subscribe(cx));
            let view =
                cx.new(|cx| TerminalView::new_with_theme(terminal.clone(), theme, window, cx));
            if let Some(pane) = tab.pane_mut(pane_id) {
                pane.terminal = Some(terminal);
                pane.view = Some(view);
                pane.error = None;
                pane.exit = None;
            }
        }
    }
}

/// How often a shared pane's task checks for arbitrated sizes. The
/// multiplexer's size events are rare and arrive with output, so this is a
/// wake-up check, not a poll that races anything.
const SHARED_SIZE_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// A reader that yields a replay prefix before the live stream, for a
/// terminal built around a shared pane whose retained output the multiplexer
/// sent with the attachment.
struct ReplayReader {
    pending: Vec<u8>,
    inner: zmux::client::SharedReader,
}

impl std::io::Read for ReplayReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if !self.pending.is_empty() {
            let count = self.pending.len().min(buffer.len());
            buffer[..count].copy_from_slice(&self.pending[..count]);
            self.pending.drain(..count);
            return Ok(count);
        }
        self.inner.read(buffer)
    }
}

/// A writer that sends a shared pane's input to the multiplexer, so every
/// shared client's input is attributed to the client that typed it.
struct SharedPaneWriter {
    pane: Arc<zmux::client::SharedPane>,
}

impl std::io::Write for SharedPaneWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.pane
            .send_input(buffer)
            .map(|()| buffer.len())
            .map_err(std::io::Error::other)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Builds the terminal's exit status from the multiplexer's report, the way
/// each platform spells one: Unix carries a wait status and Windows an exit
/// code, and the same number does not mean the same thing to both.
fn exit_status_from_raw(raw_status: i32) -> std::process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(raw_status)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        std::process::ExitStatus::from_raw(raw_status as u32)
    }
}

impl Zetta {
    /// Records a shared pane and starts the task that keeps it in step with
    /// the multiplexer: applying arbitrated sizes and routing the pane's
    /// exit report to its terminal.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn register_shared_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        pane: &Arc<zmux::client::SharedPane>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (exit_tx, exit_rx) = async_channel::unbounded();
        runtime.reporters().register_shared(mux_pane_id, exit_tx);
        self.shared_panes.insert(
            pane_id,
            SharedPaneEntry {
                pane: pane.clone(),
                mux_pane_id,
            },
        );
        // Every route into shared mode has to come through here, so every shared
        // pane can be offered back when it turns out to be the last viewer — the
        // mirror of `watch_for_revoke`'s rule for every route into holding one.
        self.watch_for_grant(
            tab_id,
            pane_id,
            session_id,
            mux_pane_id,
            runtime,
            window,
            cx,
        );
        self.spawn_shared_pane_task(tab_id, pane_id, pane.clone(), exit_rx, window, cx);
    }

    fn spawn_shared_pane_task(
        &self,
        tab_id: u64,
        pane_id: u64,
        pane: Arc<zmux::client::SharedPane>,
        exit_rx: async_channel::Receiver<zmux::client::PaneExitReport>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            // The size the pane attached at has to be applied once the tab
            // exists; without this the grid could stay at the window's size
            // while the multiplexer's pty runs at the arbitrated one.
            let mut pending_size: Option<(u16, u16)> = None;
            loop {
                match exit_rx.try_recv() {
                    Ok(report) => {
                        this.update_in(cx, |this, _window, cx| {
                            this.route_shared_pane_exit(tab_id, pane_id, report, cx);
                        })
                        .ok();
                        return;
                    }
                    Err(async_channel::TryRecvError::Empty) => {}
                    Err(async_channel::TryRecvError::Closed) => return,
                }
                if let Some(size) = pane.take_sizes().last().copied() {
                    pending_size = Some(size);
                }
                let Some(size) = pending_size else {
                    executor.timer(SHARED_SIZE_POLL_INTERVAL).await;
                    continue;
                };
                let applied = this
                    .update_in(cx, |this, window, cx| {
                        this.apply_shared_pane_size(tab_id, pane_id, size, window, cx)
                    })
                    .unwrap_or(true);
                if applied {
                    pending_size = None;
                }
                executor.timer(SHARED_SIZE_POLL_INTERVAL).await;
            }
        })
        .detach();
    }

    /// Applies the size the multiplexer arbitrated for a shared pane, by
    /// resizing the layout to it: the grid is laid out from the pane's
    /// bounds, so the shell's wraps can only line up with the cells drawn
    /// when the two agree.
    fn apply_shared_pane_size(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        (columns, lines): (u16, u16),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let bounds = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(TerminalPane::selected_terminal)
            .map(|terminal| terminal.read(cx).last_content().terminal_bounds);
        match shared_size_action(bounds, columns, lines) {
            SharedSizeAction::WaitForLayout => false,
            SharedSizeAction::AlreadyMatches => true,
            SharedSizeAction::Resize => {
                self.resize_pane_to(
                    tab_id,
                    pane_id,
                    Some(columns as usize),
                    Some(lines as usize),
                    window,
                    cx,
                );
                true
            }
        }
    }

    /// Routes a shared pane's exit report to its terminal.
    ///
    /// The terminal no longer reads the pty, so it has no event loop to learn
    /// the exit from; the multiplexer is the child's parent and its
    /// attribution of input replaces the terminal's own.
    fn route_shared_pane_exit(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        report: zmux::client::PaneExitReport,
        cx: &mut Context<Self>,
    ) {
        let mux_pane_id = self
            .shared_panes
            .get(&pane_id)
            .map(|entry| entry.mux_pane_id);
        self.shared_panes.remove(&pane_id);
        if let Some(runtime) = &self.mux
            && let Some(mux_pane_id) = mux_pane_id
        {
            runtime.reporters().forget_shared(mux_pane_id);
        }
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        if report.disconnected {
            // The byte stream already printed the connection error into the
            // grid; the pane stays open on it until the user closes it.
            return;
        }
        terminal.update(cx, |terminal, cx| match report.raw_status {
            Some(raw_status) => {
                terminal.report_child_exit(exit_status_from_raw(raw_status), report.input_sent, cx)
            }
            // The multiplexer saw the process end but could not say how — or the
            // pane was already gone when this client asked what it had missed.
            // Reporting it anyway is what lets the pane be closed: silently
            // dropping the report left a shared terminal waiting for something
            // that had already happened, with nothing on screen to say so.
            None => terminal.report_child_exit_status_unavailable(report.input_sent, cx),
        });
    }

    /// Reports this window's size for a shared pane, so the multiplexer can
    /// arbitrate shared clients down to the smallest of them.
    fn report_shared_pane_size(
        &mut self,
        pane_id: u64,
        terminal: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.shared_panes.get(&pane_id) else {
            return;
        };
        let bounds = terminal.read(cx).last_content().terminal_bounds;
        let Some((columns, lines)) = shared_size_to_report(bounds) else {
            // Not laid out yet, so this viewer has no size to arbitrate against.
            // The first layout emits `GridSizeChanged`, which reports it then.
            return;
        };
        if let Err(error) = entry.pane.send_resize(columns, lines) {
            log::debug!("could not report the shared pane's size: {error:#}");
        }
    }

    /// Drops a shared pane: its terminal is going away, so the shared
    /// connection and the exit routing that keep it in step go with it.
    ///
    /// The pane itself stays alive on the multiplexer — dropping the shared
    /// connection only removes this client from the shared set, which is
    /// exactly what closing the window's view of it means.
    pub(crate) fn drop_shared_pane(&mut self, pane_id: u64) {
        let Some(entry) = self.shared_panes.remove(&pane_id) else {
            return;
        };
        if let Some(runtime) = &self.mux {
            runtime.reporters().forget_shared(entry.mux_pane_id);
            runtime.revoke_reporters().forget(entry.mux_pane_id);
            // The grant watcher too: an offer to hand back a pane this window no
            // longer shows has nothing to hand back to, and the registration
            // would outlive every other trace of the pane.
            runtime.grant_reporters().forget(entry.mux_pane_id);
        }
    }

    /// The multiplexer offered this pane's terminal back: this window is the only
    /// viewer left, so there is nothing left to relay for.
    ///
    /// The reverse of [`Zetta::handle_pane_revoke`]. The pane keeps its grid, its
    /// scrollback and everything on screen; only what feeds them changes, from the
    /// multiplexer's relay to the terminal itself.
    pub(crate) fn handle_pane_grant(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.mux.clone() else {
            return;
        };
        // Only a pane this window is actually sharing can be taken back, and only
        // while it still has the terminal the grant was offered for.
        if !self.shared_panes.contains_key(&pane_id) {
            return;
        }
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        let Some(profile) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .map(|pane| pane.profile.clone())
        else {
            return;
        };
        let settings = TerminalSpawnSettings::current(cx);
        let options = terminal::AttachedOptions {
            shell: profile.command.clone(),
            env: HashMap::default(),
            cursor_shape: settings.cursor_shape,
            alternate_scroll: settings.alternate_scroll,
            max_scroll_history_lines: settings.max_scroll_history_lines,
            path_hyperlink_regexes: Vec::new(),
            path_hyperlink_timeout_ms: settings.path_hyperlink_timeout_ms,
            window_id: cx.entity_id().as_u64(),
        };
        let client = runtime.client().clone();
        cx.spawn_in(window, async move |this, cx| {
            let taken = cx
                .background_spawn(async move { client.take_exclusive(session_id, mux_pane_id) })
                .await;
            let attached = match taken {
                Ok(attached) => attached,
                Err(error) => {
                    // Declining is always safe: the multiplexer keeps the pane
                    // shared, and offers again the next time the viewers change.
                    log::debug!("could not take pane {mux_pane_id} back: {error:#}");
                    return;
                }
            };
            this.update_in(cx, |this, window, cx| {
                this.complete_grant_conversion(
                    tab_id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    attached,
                    terminal,
                    options,
                    &runtime,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Finishes taking a pane back: its terminal now reads the pty itself.
    #[allow(clippy::too_many_arguments)]
    fn complete_grant_conversion(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        attached: zmux::client::AttachedPane,
        terminal: Entity<Terminal>,
        options: terminal::AttachedOptions,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handover = crate::mux::attached_pane_handover(attached, runtime.client().clone());
        let child_events = match terminal.update(cx, |terminal, cx| {
            terminal.attach_pty(handover, options, cx)
        }) {
            Ok(events) => events,
            Err(error) => {
                // The pane is left as it was — still shared as far as this window
                // is concerned — but the multiplexer has already given the
                // descriptor away, so say so rather than leaving a pane that reads
                // nothing.
                self.pane_output_error = Some(format!(
                    "Could not take this pane's terminal back from the multiplexer: {error:#}"
                ));
                cx.notify();
                return;
            }
        };
        // The shared bookkeeping goes, and with it the shared connection: this
        // window no longer reads a relay. Done after the conversion, so a failure
        // above leaves the pane as it was.
        self.drop_shared_pane(pane_id);
        // Exits now arrive through the pty's own child-event channel again, and a
        // pane holding the descriptor has to be able to answer a future revoke.
        runtime.reporters().register(mux_pane_id, child_events);
        self.watch_for_revoke(
            tab_id,
            pane_id,
            session_id,
            mux_pane_id,
            runtime,
            window,
            cx,
        );
        cx.notify();
    }

    /// The multiplexer asked this window to hand an exclusively attached pane
    /// over: another client attached, and the pane is becoming shared.
    ///
    /// The holder has to stop reading the pty — synchronously, so the
    /// daemon's drain cannot lose output to this window's own loop — snapshot
    /// the grid, and re-attach as a shared client whose terminal reads the
    /// daemon's relay instead.
    pub(crate) fn handle_pane_revoke(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.mux.clone() else {
            return;
        };
        let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(pane_id))
            .and_then(|pane| pane.terminal.clone())
        else {
            return;
        };
        // The snapshot has to be a stable picture of what the daemon will
        // retain, and the pty loop is the only other reader of the master.
        if terminal
            .update(cx, |terminal, _| terminal.stop_pty_loop())
            .is_err()
        {
            return;
        }
        let (snapshot, columns, lines) = terminal.update(cx, |terminal, _| {
            let bounds = terminal.last_content().terminal_bounds;
            (
                terminal.ansi_snapshot(SNAPSHOT_LINES),
                bounds.num_columns() as u16,
                bounds.num_lines() as u16,
            )
        });
        // Built here, on the main thread, because it reads the live tab. The
        // daemon re-reads the session's state once this handover completes, so
        // sending it below is what makes the joining window rebuild the layout as
        // it is now rather than as it was when sharing was switched on.
        let refresh = self.shared_session_refresh(tab_id, cx);
        let client = runtime.client().clone();
        let terminal_handle = terminal.clone();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    if let Some((session_id, summary, state)) = refresh
                        && let Err(error) = client.share(session_id, summary, state, None, true)
                    {
                        // Not fatal: the handover still works, the joining
                        // window just rebuilds an older layout.
                        log::debug!("could not refresh shared session {session_id}: {error:#}");
                    }
                    client.send_snapshot(session_id, mux_pane_id, snapshot, columns, lines)?;
                    // Plain attach: it presents this process's id, so the
                    // daemon can tell the pane's holder is re-attaching. If
                    // the snapshot has not been processed yet the daemon
                    // waits for it before answering.
                    client.attach(session_id, Some(mux_pane_id), None)
                })
                .await
                .ok();
            let Some(zmux::client::AttachOutcome::SharedAttached { pane, .. }) = outcome else {
                // The handover failed; the daemon keeps the pane under revoke
                // until the next attach attempt times out.
                log::debug!("the multiplexer handover of pane {mux_pane_id} did not complete");
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.complete_revoke_conversion(
                    tab_id,
                    pane_id,
                    session_id,
                    mux_pane_id,
                    pane,
                    terminal_handle,
                    &runtime,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
    }

    /// Finishes the revoke handover: the pane's terminal now reads the
    /// multiplexer's relay, and the pane is registered as shared.
    #[allow(clippy::too_many_arguments)]
    fn complete_revoke_conversion(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        session_id: u64,
        mux_pane_id: u64,
        pane: zmux::client::SharedPane,
        terminal: Entity<Terminal>,
        runtime: &MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let pane = Arc::new(pane);
        // The daemon retained what the pane produced while this window's pty
        // loop was stopped; replay it into the grid, which holds the snapshot
        // but not those bytes.
        let reader: Box<dyn std::io::Read + Send> = Box::new(ReplayReader {
            pending: pane.replay.clone(),
            inner: pane.reader(),
        });
        let writer: Box<dyn std::io::Write + Send> =
            Box::new(SharedPaneWriter { pane: pane.clone() });
        if terminal
            .update(cx, |terminal, _| {
                terminal.attach_byte_stream(reader, writer)
            })
            .is_err()
        {
            return;
        }
        runtime.revoke_reporters().forget(mux_pane_id);
        self.register_shared_pane(
            tab_id,
            pane_id,
            session_id,
            mux_pane_id,
            &pane,
            runtime,
            window,
            cx,
        );
        // This pane's view was wired up when it was spawned, long before it
        // became shared, so its size reports have to be subscribed here rather
        // than in `connect_terminal_view`. Without this the pane joins size
        // arbitration silently: the daemon only ever knows the size it was
        // handed over at, so every other viewer is sized against a stale figure.
        self.subscribe_shared_pane_size(pane_id, &terminal, window, cx);
        cx.notify();
    }

    /// Reports this window's grid size for a shared pane whenever it changes.
    ///
    /// A shared pane's grid is laid out by this window, but the pty is the
    /// multiplexer's: the daemon has to be told this window's size so it can
    /// arbitrate every shared client down to the smallest of them. Called from
    /// both routes into shared mode — attaching into it, and being revoked into
    /// it — because a pane that never reports cannot be arbitrated for.
    fn subscribe_shared_pane_size(
        &mut self,
        pane_id: u64,
        terminal: &Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        watch_grid_size(terminal, window, cx, move |this, terminal, cx| {
            this.report_shared_pane_size(pane_id, terminal, cx);
        });
        // The size the pane is at right now, which no later event will repeat:
        // a pane that is already laid out emits `GridSizeChanged` only when it
        // next changes, so without this the daemon would arbitrate against the
        // handover size until the user happened to resize something.
        self.report_shared_pane_size(pane_id, terminal, cx);
    }
}

/// Calls `on_change` whenever `terminal`'s grid size changes.
///
/// The terminal comes from the subscription rather than from a captured handle,
/// and that is the whole point of this existing. GPUI keeps a subscription in
/// the *emitter's* list and drops it only when the emitter is released, so a
/// closure that captures its own emitter is a cycle: the terminal can never be
/// released, however thoroughly its tab is closed. A shared pane's terminal kept
/// alive that way never stops its byte stream, so its relay socket stays open
/// and the multiplexer goes on counting this window among the pane's viewers —
/// which is how unsharing came to be refused for a window that had closed the
/// tab.
pub(crate) fn watch_grid_size<V: 'static>(
    terminal: &Entity<Terminal>,
    window: &mut Window,
    cx: &mut Context<V>,
    mut on_change: impl FnMut(&mut V, &Entity<Terminal>, &mut Context<V>) + 'static,
) {
    cx.subscribe_in(
        terminal,
        window,
        move |view, terminal, event: &TerminalEvent, _window, cx| {
            if let TerminalEvent::GridSizeChanged = event {
                on_change(view, terminal, cx);
            }
        },
    )
    .detach();
}

impl Zetta {
    /// Whether the multiplexer is holding this session, as opposed to it being
    /// one this process kept in memory because the multiplexer was unreachable.
    ///
    /// Decided from the published catalog: it is the same source the reconnect
    /// picker lists, so the two cannot disagree about what exists, and it costs
    /// no round trip on a path that runs whenever the menu is built.
    pub(crate) fn multiplexer_holds_session(&self, session_id: u64) -> bool {
        if self.no_mux {
            return false;
        }
        crate::background_sessions::read_session_catalogs(
            &crate::background_sessions::session_catalog_dir(),
        )
        .is_ok_and(|catalogs| {
            crate::background_sessions::multiplexer_held_catalog_sessions(
                &catalogs,
                crate::background_sessions::process_is_zetta,
                std::process::id(),
            )
            .any(|(_, session)| session.id == session_id)
        })
    }
}
