//! Handing a session to the multiplexer and taking one back from it.
//!
//! An attached pane arrives as a descriptor — an exclusive pty, or the relay of
//! a shared pane — and becomes an ordinary terminal from there on. Shared panes
//! keep a live connection, and their arbitration lives in `shared_panes.rs`.

use super::*;

use super::shared_panes::SharedPaneWriter;

impl Zetta {
    /// Gives a detached tab to the multiplexer to hold.
    ///
    /// Returns `false` when explicit `--no-mux` mode selected the legacy
    /// in-process owner. Normal launches return an error when a pane cannot be
    /// handed to the daemon, so backgrounding never silently changes its
    /// lifetime guarantees.
    pub(super) fn hand_session_to_multiplexer(
        &mut self,
        tab: &mut Tab,
        authentication: Option<&SessionAuthentication>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<bool> {
        if self.no_mux {
            return Ok(false);
        }
        let (Some(runtime), Some(session_id)) = (
            self.mux_panes
                .runtime_for_tab(tab.id)
                .or_else(|| self.mux.clone()),
            self.mux_panes.session_id(tab.id),
        ) else {
            anyhow::bail!(
                "the tab has no daemon-owned session; start Zetta with --no-mux to use local session ownership"
            );
        };
        anyhow::ensure!(
            !runtime.is_remote(),
            "remote sessions are live-only and cannot be stored as local background sessions"
        );

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
    pub(super) fn session_publication(
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
        let mut state = crate::session_state::TabState::from_tab(tab, self.mux_panes.ids());
        state.pane_theme_source = Some(crate::session_state::PaneThemeSource {
            process_id: std::process::id(),
            runner_id: self.background_sessions.runner_id(),
            configuration_generation: self.configuration_generation,
        });
        let state = serde_json::to_value(state).context("serializing the session's tab state")?;
        Ok((summary, state))
    }
}

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
                zmux::retention::Retention::Memory {
                    bytes: self.launch_config.sessions.ring_bytes,
                },
            )?;
            #[cfg(not(feature = "session-persistence"))]
            let runtime = MuxRuntime::connect_with_retention(retention)?;
            self.install_mux_runtime(runtime, cx);
        }
        let Some(runtime) = self.mux.clone() else {
            anyhow::bail!("no multiplexer is running");
        };

        self.attach_multiplexer_session_with_runtime(session_id, secret, runtime, window, cx)
    }

    pub(crate) fn attach_remote_multiplexer_session(
        &mut self,
        target: zmux::remote::RemoteTarget,
        session_id: u64,
        secret: Option<SessionSecret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<AttachOutcomeSummary> {
        let runtime = MuxRuntime::connect_remote(target)?;
        self.attach_multiplexer_session_with_runtime(session_id, secret, runtime, window, cx)
    }

    fn attach_multiplexer_session_with_runtime(
        &mut self,
        session_id: u64,
        secret: Option<SessionSecret>,
        runtime: MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<AttachOutcomeSummary> {
        // Starts from the session: which panes it has is part of what a
        // protected session's secret protects, so the multiplexer resolves the
        // first pane itself once the secret has been checked.
        let first = runtime
            .client()
            .attach_with_secret(session_id, None, secret.as_ref())?;
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
        runtime.set_session_secret(secret.as_ref());

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
        let mut state: crate::session_state::TabState =
            serde_json::from_value(state).context("reading the session's tab state")?;
        if pane_theme_source_is_stale(
            state.pane_theme_source,
            std::process::id(),
            self.configuration_generation,
        ) {
            clear_session_theme_overrides(&mut state);
        }
        let restored_panes = restored_pane_metadata(&state, &summary);
        let restored_metadata = self.prepare_restored_panes(restored_panes.clone());
        let restored_profiles = self.restored_profiles(&restored_panes, &restored_metadata);
        let tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        let mut tab = state.clone().into_tab_by_pane(tab_id, |routing_id, name| {
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
        })?;
        tab.id = tab_id;
        // The catalog's title is what the user picked this session out of the
        // list by. A restored tab whose terminal has not yet reprinted its
        // title would otherwise fall back to a generic name, so the tab they
        // chose as "htop" comes back called "Terminal".
        if tab.custom_title.is_none() && !summary.title.is_empty() {
            tab.process_title = Some(summary.title.clone());
        }

        self.mux_panes
            .adopt_session_with_runtime(tab_id, session_id, runtime.clone());
        // Pair the pane the multiplexer chose with the tab pane that named it,
        // rather than assuming it was the first one listed.
        let first_pane = state
            .panes
            .iter()
            .find(|candidate| candidate.mux_pane_id == Some(pane.pane_id()))
            .map_or(state.panes[0].id, |candidate| candidate.id);
        let mut attached = vec![(first_pane, pane)];
        for pane_state in state.panes.iter().filter(|pane| pane.id != first_pane) {
            let Some(mux_pane_id) = pane_state.mux_pane_id else {
                continue;
            };
            match runtime.client().attach_with_secret(
                session_id,
                Some(mux_pane_id),
                secret.as_ref(),
            )? {
                zmux::client::AttachOutcome::Attached { pane, .. } => {
                    attached.push((pane_state.id, AttachedPaneKind::Exclusive(pane)));
                }
                zmux::client::AttachOutcome::SharedAttached { pane, .. } => {
                    attached.push((pane_state.id, AttachedPaneKind::Shared(pane)));
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
        self.bind_restored_projects(&tab, &restored_metadata);

        self.build_attached_panes(
            &mut tab,
            session_id,
            attached,
            &restored_metadata,
            &runtime,
            window,
            cx,
        );
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
    #[allow(clippy::too_many_arguments)]
    fn build_attached_panes(
        &mut self,
        tab: &mut Tab,
        session_id: u64,
        attached: Vec<(u64, AttachedPaneKind)>,
        restored: &RestoredPaneMetadata,
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
            let theme_override = pane.theme_override.clone();
            let tab_theme_override = tab.theme_override.clone();
            let routing_id = pane.routing_id;
            let project = self.projects.config_for_pane(pane_id).cloned();
            let theme = self.restored_terminal_theme(
                theme_override.as_deref(),
                tab_theme_override.as_deref(),
                &profile,
                project.as_deref(),
                cx,
            );
            let working_directory = restored.working_directory(routing_id);

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
                        crate::mux::attached_pane_handover_with_secret(
                            attached,
                            runtime.client().clone(),
                            runtime.session_secret(),
                        ),
                        options,
                        cx.background_executor(),
                        PathStyle::local(),
                    ) {
                        Ok(mut built) => {
                            built.builder = built
                                .builder
                                .with_working_directory(working_directory.clone());
                            (
                                mux_pane_id,
                                Some(built.builder),
                                Some(built.child_events),
                                None::<Arc<zmux::client::SharedPane>>,
                            )
                        }
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
                    .with_working_directory(working_directory.clone())
                    .with_replay(pane.replay.clone())
                    .with_pty_control(crate::mux::mux_pty_control_with_secret(
                        runtime.client().clone(),
                        session_id,
                        mux_pane_id,
                        runtime.session_secret(),
                    ));
                    (mux_pane_id, Some(built), None, Some(pane))
                }
            };
            let Some(built) = built else {
                continue;
            };
            crate::run_command::process_run_registry().pane_reopened(
                crate::run_command::RunPaneIdentity::new(tab.attention_id, routing_id),
            );
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
