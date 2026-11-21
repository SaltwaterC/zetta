//! Following a shared session: the watch connection, reattaching a stream
//! that failed, and applying the snapshots and pane arrivals it delivers.

use super::*;

impl Zetta {
    /// Keeps a shared tab subscribed to canonical collaboration snapshots.
    /// Requests that need a data stream (a pane added by another viewer) are
    /// performed on the background executor; applying the resulting terminal
    /// remains a window operation so local GPUI entities never cross threads.
    pub(in crate::background_session_ui) fn watch_shared_session(
        &mut self,
        key: &SharedSessionKey,
        runtime: MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        let receiver = runtime.shared_reports().register(session_id);
        self.watch_shared_session_with_receiver(key, runtime, receiver, window, cx);
    }

    pub(super) fn watch_shared_session_with_receiver(
        &mut self,
        key: &SharedSessionKey,
        runtime: MuxRuntime,
        receiver: async_channel::Receiver<zmux::client::SharedSessionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(watch_id) = self.shared_collaboration.begin_watch(key) else {
            return;
        };
        let connection = SharedWatchConnection {
            client: runtime.client().clone(),
            secret: runtime.session_secret(),
        };
        // Recorded for every caller that applies a snapshot, not just this
        // loop: a committed spawn and a conflict response both arrive with a
        // state that can name a pane this window still has to attach.
        self.shared_collaboration
            .set_connection(key, connection.clone());
        let key = key.clone();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                let current = this
                    .update(cx, |this, _| {
                        this.shared_collaboration.watch_is_current(&key, watch_id)
                    })
                    .unwrap_or(false);
                if !current {
                    break;
                }
                let task = match event {
                    zmux::client::SharedSessionEvent::Updated(state)
                    | zmux::client::SharedSessionEvent::PanesChanged { state, .. } => {
                        this.update_in(cx, |this, window, cx| {
                            this.apply_shared_snapshot(&key, state, window, cx);
                        })
                        .ok();
                        None
                    }
                    // A pane added by another viewer needs no handling of its
                    // own: applying the snapshot attaches every pane it names
                    // that this window does not hold, which is exactly this one.
                    zmux::client::SharedSessionEvent::PaneAdded { state, .. } => {
                        this.update_in(cx, |this, window, cx| {
                            this.apply_shared_snapshot(&key, state, window, cx);
                        })
                        .ok();
                        None
                    }
                    zmux::client::SharedSessionEvent::PaneRemoved {
                        session_id: _,
                        pane_id,
                        state,
                    } => {
                        this.update_in(cx, |this, window, cx| {
                            this.remove_incoming_shared_pane(&key, pane_id, state, window, cx);
                        })
                        .ok();
                        None
                    }
                    zmux::client::SharedSessionEvent::StreamFailed {
                        session_id: _,
                        pane_id,
                    } => this
                        .update_in(cx, |this, window, cx| {
                            this.handle_shared_stream_failure(
                                &key,
                                pane_id,
                                connection.clone(),
                                window,
                                cx,
                            )
                        })
                        .ok(),
                };
                if let Some(task) = task {
                    task.await;
                }
            }
            this.update(cx, |this, _| {
                this.shared_collaboration.end_watch(&key, watch_id);
            })
            .ok();
        })
        .detach();
    }

    /// Attaches every pane the snapshot holds that this window does not.
    ///
    /// One task per pane, each claimed through `begin_attach` so the several
    /// callers that apply a snapshot cannot attach the same pane twice. Each
    /// task re-applies the snapshot it ends up with, which is what installs the
    /// geometry that names the pane it just attached.
    pub(super) fn attach_panes_missing_locally(
        &mut self,
        key: &SharedSessionKey,
        state: &SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        let missing = self
            .shared_collaboration
            .missing_panes_for_snapshot(key, state);
        if missing.is_empty() {
            return;
        }
        let Some(connection) = self.shared_collaboration.connection(key) else {
            log::warn!(
                "shared session {session_id} has panes {missing:?} to attach but no daemon connection"
            );
            return;
        };
        // A pane another viewer added travels the same way as the ones this
        // window attached with: the runtime is what remembers which that is.
        let runtime = self
            .shared_collaboration
            .tab_id(key)
            .and_then(|tab_id| self.mux_panes.runtime_for_tab(tab_id));
        for mux_pane_id in missing {
            if !self.shared_collaboration.begin_attach(key, mux_pane_id) {
                continue;
            }
            let connection = connection.clone();
            let runtime = runtime.clone();
            let key = key.clone();
            cx.spawn_in(window, async move |this, cx| {
                let attached =
                    attach_shared_pane_with_retries(session_id, mux_pane_id, &connection, cx).await;
                // Only a session whose panes travel over Mosh takes the step
                // below, and the test is made here rather than inside it: what
                // follows this task is the snapshot that names the session's
                // active pane, so anything awaited in between delays *that*.
                // Waiting on a task that had nothing to do was enough to let a
                // snapshot land after the focus change it then undid — the
                // pane the user had just moved to lost the focus again.
                let relayed = runtime
                    .filter(|runtime| runtime.pane_transport().is_zosh())
                    .filter(|_| attached.as_ref().is_some_and(|(_, pane)| pane.is_some()));
                let mut pane_streams = match relayed {
                    Some(runtime) => {
                        // An SSH round trip, so not on the thread that draws.
                        cx.background_spawn(async move {
                            crate::remote_pane_transport::bootstrap_spawned_pane(
                                &runtime,
                                session_id,
                                mux_pane_id,
                            )
                        })
                        .await
                    }
                    None => crate::remote_pane_transport::RemotePaneStreams::default(),
                };
                this.update_in(cx, |this, window, cx| {
                    this.shared_collaboration
                        .end_attach(&key, mux_pane_id);
                    let Some((latest, pane)) = attached else {
                        return;
                    };
                    if let Some(pane) = pane
                        && let Err(error) = this.attach_incoming_shared_pane(
                            &key,
                            pane,
                            latest.clone(),
                            &mut pane_streams,
                            window,
                            cx,
                        )
                    {
                        log::warn!(
                            "could not attach shared pane {mux_pane_id} of session {session_id}: {error:#}"
                        );
                        return;
                    }
                    this.apply_shared_snapshot(&key, latest, window, cx);
                })
                .ok();
            })
            .detach();
        }
    }

    /// Whether a failed shared stream is one this window can put back.
    ///
    /// Only a pane this window is actually reading the multiplexer's stream for.
    /// A pane carried over Mosh is not: the stream it was attached with is
    /// released as soon as its link is up, because reading the remote host's
    /// output twice would pay for it twice — and the daemon cannot tell that
    /// release from a broken relay, so it reports it as a failure either way
    /// (`releasing_a_shared_stream_is_reported_to_its_own_client_as_a_failure`).
    ///
    /// Answering that report with a replacement is not merely useless, it is a
    /// loop that feeds itself: the replacement has nowhere to go, so it is
    /// dropped, and dropping it is another release the daemon reports the same
    /// way. Every attempt attached a stream, was sent the pane's whole retained
    /// screen over the session's one SSH forward, and dropped it again — and
    /// because these reports share a queue with the session's snapshot, pane-
    /// added and pane-removed events, the tab stopped being told anything about
    /// the session at all.
    pub(super) fn shared_stream_is_replaceable(
        &self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
    ) -> bool {
        self.shared_collaboration
            .local_pane_id(key, mux_pane_id)
            .is_some_and(|local_pane_id| self.shared_panes.contains_key(&local_pane_id))
    }

    /// Whether the pane already has a replacement stream its reader has not
    /// installed yet, which answers any failure reported in the meantime.
    pub(super) fn shared_stream_replacement_pending(
        &self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
    ) -> bool {
        self.shared_collaboration
            .local_pane_id(key, mux_pane_id)
            .and_then(|local_pane_id| self.shared_panes.get(&local_pane_id))
            .is_some_and(|entry| entry.pane.replacement_pending())
    }

    /// Replaces a stream the pane's own reader found broken.
    ///
    /// The daemon's `SharedStreamFailed` is what usually starts this, but only
    /// the daemon that served the stream can send it. One that replaced itself
    /// closed every relay on the way and has no record of them, so without
    /// this the pane waits forever on a connection that no longer exists:
    /// output stops, input goes nowhere, and the program carries on unseen.
    pub(crate) fn reattach_lost_shared_stream(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let Some(connection) = self.shared_collaboration.connection(key) else {
            return Task::ready(());
        };
        self.handle_shared_stream_failure(key, mux_pane_id, connection, window, cx)
    }

    pub(super) fn handle_shared_stream_failure(
        &mut self,
        key: &SharedSessionKey,
        pane_id: u64,
        connection: SharedWatchConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let session_id = key.session_id;
        if !self.shared_stream_is_replaceable(key, pane_id)
            || self.shared_stream_replacement_pending(key, pane_id)
            || !self.shared_collaboration.begin_reattach(key, pane_id)
        {
            return Task::ready(());
        }
        log::warn!("shared stream for session {session_id} pane {pane_id} failed; reattaching it");
        let executor = cx.background_executor().clone();
        let key = key.clone();
        cx.spawn_in(window, async move |this, cx| {
            for delay in [
                Duration::from_millis(50),
                Duration::from_millis(150),
                Duration::from_millis(400),
                Duration::from_millis(1_000),
                Duration::from_millis(2_000),
            ] {
                executor.timer(delay).await;
                let refreshed = cx
                    .background_spawn({
                        let connection = connection.clone();
                        async move {
                            let state = connection.client.shared_snapshot(session_id)?;
                            if !state.contains_pane(pane_id) {
                                return Ok::<_, anyhow::Error>((state, None));
                            }
                            let attached = connection.client.attach_shared_with_secret(
                                session_id,
                                pane_id,
                                connection.secret.as_ref(),
                            )?;
                            Ok((state, Some(attached)))
                        }
                    })
                    .await;
                let (state, pane) = match refreshed {
                    Ok((state, Some(zmux::client::AttachOutcome::SharedAttached { pane, .. }))) => {
                        (state, pane)
                    }
                    Ok((state, None)) => {
                        this.update_in(cx, |this, window, cx| {
                            this.apply_shared_snapshot(&key, state, window, cx);
                        })
                        .ok();
                        break;
                    }
                    _ => continue,
                };
                let replaced = this
                    .update_in(cx, |this, window, cx| {
                        let replaced =
                            this.replace_shared_pane_stream(&key, pane_id, pane, window, cx);
                        if replaced {
                            this.apply_shared_snapshot(&key, state, window, cx);
                        }
                        replaced
                    })
                    .unwrap_or(false);
                if !replaced {
                    log::warn!(
                        "shared pane {pane_id} of session {session_id} had nowhere to put a \
                         replacement stream; leaving it as it is"
                    );
                }
                // Either way, an attach that succeeded ends this. A replacement
                // the pane refused is one it has nowhere to put, and waiting
                // does not change that — while the refused stream has just been
                // dropped, which the daemon reports as one more failure.
                // Retrying is what turns one report into a loop. Only an attach
                // that *failed* is worth coming back for, which is the
                // `continue` above.
                break;
            }
            this.update(cx, |this, _| {
                this.shared_collaboration.end_reattach(&key, pane_id);
            })
            .ok();
        })
    }

    pub(crate) fn apply_shared_snapshot(
        &mut self,
        key: &SharedSessionKey,
        state: SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return;
        };
        if self.shared_collaboration.snapshot_is_stale(key, &state) {
            return;
        }
        self.attach_panes_missing_locally(key, &state, window, cx);
        if self.install_shared_snapshot(key, state, cx) {
            self.focus_active(window, cx);
            let this = cx.entity().downgrade();
            cx.defer(move |cx| {
                this.update(cx, |this, cx| {
                    this.report_all_shared_pane_sizes(tab_id, cx);
                })
                .ok();
            });
        }
    }

    /// The part of applying a snapshot that needs no window: drop the panes the
    /// snapshot no longer has, then install its geometry. Reports whether the
    /// snapshot was applied.
    ///
    /// Separate from [`Zetta::apply_shared_snapshot`] because the operation
    /// queue runs off the main render path and has no window to focus with,
    /// while still having to converge the tab on what the daemon answered.
    pub(crate) fn install_shared_snapshot(
        &mut self,
        key: &SharedSessionKey,
        state: SharedSessionState,
        cx: &mut Context<Self>,
    ) -> bool {
        let session_id = key.session_id;
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return false;
        };
        // Staleness is decided before anything is removed. A snapshot that will
        // not be applied must not take panes out of the tab: the layout is only
        // rewritten by the apply below, so removing for a snapshot that is then
        // discarded leaves the tab holding a layout entry with no pane behind it.
        if self.shared_collaboration.snapshot_is_stale(key, &state) {
            return false;
        }
        let revision = state.revision;
        let durable_state_has_no_panes = serde_json::from_value::<TabState>(state.state.clone())
            .ok()
            .is_some_and(|tab_state| tab_state.panes.is_empty());
        let stale = self
            .shared_collaboration
            .panes_missing_from_snapshot(key, &state);
        for local_pane_id in stale {
            self.remove_local_shared_pane(key, local_pane_id, cx);
        }
        let disposition = self.tabs.by_id_mut(tab_id).map(|tab| {
            self.shared_collaboration
                .apply_snapshot_to_tab(key, state, tab)
        });
        match disposition.transpose() {
            Ok(Some(SharedSnapshotDisposition::Applied)) => {
                cx.notify();
                true
            }
            Ok(Some(_)) | Ok(None) => false,
            // Not `debug`: this is the tab and the daemon disagreeing about the
            // session's shape, which is the failure that leaves panes on screen
            // that nobody can close.
            Err(error) => {
                log::warn!(
                    "could not apply shared session {session_id} snapshot at revision {}: {error:#}",
                    revision.0
                );
                if durable_state_has_no_panes
                    && self
                        .tabs
                        .by_id(tab_id)
                        .is_some_and(|tab| !tab.panes.is_empty())
                {
                    // Keep the live local panes intact and repair the
                    // daemon's malformed durable blob from them. The
                    // publication guard below prevents a concurrently empty
                    // reconstruction tab from overwriting this repair.
                    log::error!(
                        "shared session {session_id} published an empty durable pane list; restoring it from tab {tab_id}"
                    );
                    self.sync_shared_tab_state(tab_id, cx);
                }
                false
            }
        }
    }

    pub(super) fn remove_local_shared_pane(
        &mut self,
        key: &SharedSessionKey,
        local_pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        let tab_id = self.shared_collaboration.tab_id(key);
        self.drop_shared_pane(local_pane_id, cx);
        if let Some(tab_id) = tab_id
            && let Some(tab) = self.tabs.by_id_mut(tab_id)
        {
            detach_pane_from_tab(tab, local_pane_id);
        }
        self.closing_shared_panes.remove(&local_pane_id);
        self.mux_panes.forget_pane(local_pane_id);
        self.shared_collaboration
            .remove_local_pane(key, local_pane_id);
        if let Some(tab_id) = tab_id {
            self.sync_shared_tab_state(tab_id, cx);
        }
    }

    pub(super) fn replace_shared_pane_stream(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        replacement: zmux::client::SharedPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let session_id = key.session_id;
        let Some(local_pane_id) = self.shared_collaboration.local_pane_id(key, mux_pane_id) else {
            return false;
        };
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return false;
        };
        let Some(pane) = self
            .shared_panes
            .get(&local_pane_id)
            .map(|entry| entry.pane.clone())
        else {
            return false;
        };
        if let Err(error) = pane.replace_connection_from(&replacement) {
            log::warn!(
                "could not replace shared stream for session {session_id} pane {mux_pane_id}: {error:#}"
            );
            return false;
        }
        if let Some(terminal) = self
            .tabs
            .pane_in(tab_id, local_pane_id)
            .and_then(|pane| pane.terminal.clone())
        {
            self.schedule_shared_pane_size_report(local_pane_id, terminal, cx);
        }
        true
    }

    pub(super) fn attach_incoming_shared_pane(
        &mut self,
        key: &SharedSessionKey,
        pane: zmux::client::SharedPane,
        state: SharedSessionState,
        pane_streams: &mut crate::remote_pane_transport::RemotePaneStreams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let session_id = key.session_id;
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return Ok(());
        };
        let mux_pane_id = pane.pane_id();
        let local_pane_id = take_next_id(&mut self.next_pane_id);
        let canonical: TabState = serde_json::from_value(state.state.clone())
            .context("reading the added shared pane state")?;
        // The batch already carries canonical geometry and summary metadata,
        // while its opaque tab blob can still predate the requester's new local
        // pane id. Use the summary for that short interval; the follow-up opaque
        // state publication fills in its complete PaneState.
        let pane_state = canonical
            .panes
            .iter()
            .find(|pane| pane.mux_pane_id == Some(mux_pane_id));
        let summary_pane = state
            .summary
            .panes
            .iter()
            .find(|pane| pane.id == mux_pane_id);
        let profile_name = pane_state
            .map(|pane| pane.profile.as_str())
            .or_else(|| summary_pane.map(|pane| pane.profile.as_str()))
            .unwrap_or_default();
        // A profile of this name in *this* window supplies the theme and icon a
        // replicated pane is drawn with. Its command is local and stays local:
        // the pane's process runs on the session's host, and a split from it
        // sends this name back for that host to resolve. See
        // `terminal_spawn::shared_draft_process`.
        let configured_profiles = if self.project_context_policy(tab_id).is_remote() {
            &self.launch_config.profiles
        } else {
            &self.profiles
        };
        let profile = configured_profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(profile_name))
            .cloned()
            .unwrap_or_else(|| Profile {
                console_backend: Default::default(),
                name: profile_name.to_owned(),
                command: task::Shell::System,
                theme: None,
                dark_theme: None,
                // The program the daemon reported for the pane, which is the
                // one actually running, rather than a generic mark: a session
                // whose profiles this window does not have still shows its
                // panes as the shells they are.
                icon: summary_pane.map_or_else(ProfileIcon::default, |pane| {
                    ProfileIcon::automatic_for_program_name(&pane.application)
                }),
            });
        let mut local_pane = TerminalPane::new(local_pane_id, profile);
        if let Some(pane_state) = pane_state {
            local_pane.label_number = pane_state.label_number;
            local_pane.generated_label = pane_state.generated_label.clone();
            local_pane.custom_label = pane_state.custom_label.clone();
            local_pane.theme_override = pane_state.theme_override.clone();
            local_pane.environment_overrides = pane_state.environment_overrides.clone();
            apply_pane_overlay(&mut local_pane, pane_state.overlay.as_ref());
        } else if let Some(summary_pane) = summary_pane
            && !summary_pane.label.is_empty()
        {
            local_pane.generated_label = Some(summary_pane.label.clone());
        }
        let tab = self
            .tabs
            .by_id_mut(tab_id)
            .with_context(|| format!("shared session {session_id} tab is closed"))?;
        tab.push_pane(local_pane);
        self.mux_panes.record(local_pane_id, mux_pane_id);
        self.shared_collaboration
            .record_pane(key, mux_pane_id, local_pane_id);
        self.shared_collaboration.accept_pane_added(
            key,
            state.clone(),
            mux_pane_id,
            local_pane_id,
        )?;
        let attached = vec![(local_pane_id, AttachedPaneKind::Shared(pane))];
        let restored = RestoredPaneMetadata::default();
        let runtime = self
            .mux_panes
            .runtime_for_tab(tab_id)
            .or_else(|| self.mux.clone())
            .context("shared session runtime disappeared")?;
        // `build_attached_panes` borrows the tab while it registers all the
        // terminal-side observers, so find it again only after the registry
        // work above has completed.
        let tab_index = self
            .tab_index(tab_id)
            .expect("the shared tab was checked above");
        let mut tab = self.tabs.remove(tab_index);
        self.build_attached_panes(
            &mut tab,
            session_id,
            attached,
            &restored,
            &runtime,
            pane_streams,
            window,
            cx,
        );
        self.tabs.insert(tab_index, tab);
        self.report_transport_fallbacks(pane_streams.fallbacks(), cx);
        let view = self
            .tabs
            .pane_in(tab_id, local_pane_id)
            .and_then(|pane| pane.view.clone())
            .context("added shared pane did not build a terminal view")?;
        self.connect_terminal_view(tab_id, local_pane_id, view, window, cx);
        Ok(())
    }

    pub(super) fn remove_incoming_shared_pane(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        state: SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local_pane_id = self
            .shared_collaboration
            .accept_pane_removed(key, state.clone(), mux_pane_id)
            .ok()
            .flatten();
        if let Some(local_pane_id) = local_pane_id {
            self.drop_shared_pane(local_pane_id, cx);
        }
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return;
        };
        if let Some(local_pane_id) = local_pane_id {
            if let Some(tab) = self.tabs.by_id_mut(tab_id) {
                detach_pane_from_tab(tab, local_pane_id);
            }
            self.mux_panes.forget_pane(local_pane_id);
        }
        self.sync_shared_tab_state(tab_id, cx);
        self.apply_shared_snapshot(key, state, window, cx);
        cx.notify();
    }
}
