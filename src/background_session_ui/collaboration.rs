//! The window-side model for a daemon-owned shared session.
//!
//! `zmux` addresses panes by stable ids, while every Zetta window has its own
//! pane-id namespace. Keeping that translation here makes an incoming
//! snapshot safe to apply even when the session was published by another
//! window, and gives lifecycle events one place to update the mapping before
//! a terminal is attached.

use super::*;

use super::multiplexer::AttachedPaneKind;
use crate::session_state::{AxisState, LayoutState, PaneState, TabState};
use zmux::messages::SharedSessionState;
use zmux::protocol::{BackgroundPaneLayout, BackgroundPaneSummary};

#[derive(Default)]
pub(crate) struct SharedSessionCoordinator {
    sessions: HashMap<u64, SharedSessionBinding>,
    next_watch_id: u64,
}

struct SharedSessionBinding {
    tab_id: u64,
    state: SharedSessionState,
    mux_to_local: HashMap<u64, u64>,
    local_to_mux: HashMap<u64, u64>,
    sync_generation: u64,
    sync_in_flight: bool,
    sync_pending: bool,
    watch_id: Option<u64>,
}

#[derive(Clone)]
struct SharedWatchConnection {
    client: Arc<zmux::client::Client>,
    secret: Option<zmux::auth::SessionSecret>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SharedSnapshotDisposition {
    Applied,
    Stale,
    Unknown,
}

impl SharedSessionCoordinator {
    pub(crate) fn bind(
        &mut self,
        session_id: u64,
        tab_id: u64,
        state: SharedSessionState,
        mappings: impl IntoIterator<Item = (u64, u64)>,
    ) -> Result<()> {
        anyhow::ensure!(
            state.session_id == session_id,
            "shared snapshot belongs to session {}, expected {session_id}",
            state.session_id
        );
        let mut mux_to_local = HashMap::new();
        let mut local_to_mux = HashMap::new();
        for (mux_pane_id, local_pane_id) in mappings {
            if mux_to_local.insert(mux_pane_id, local_pane_id).is_some()
                || local_to_mux.insert(local_pane_id, mux_pane_id).is_some()
            {
                anyhow::bail!("shared pane mapping was not one-to-one");
            }
        }
        self.sessions.insert(
            session_id,
            SharedSessionBinding {
                tab_id,
                state,
                mux_to_local,
                local_to_mux,
                sync_generation: 0,
                sync_in_flight: false,
                sync_pending: false,
                watch_id: None,
            },
        );
        Ok(())
    }

    pub(crate) fn forget(&mut self, session_id: u64) {
        self.sessions.remove(&session_id);
    }

    pub(crate) fn tab_id(&self, session_id: u64) -> Option<u64> {
        self.sessions.get(&session_id).map(|session| session.tab_id)
    }

    pub(crate) fn is_bound(&self, session_id: u64) -> bool {
        self.sessions.contains_key(&session_id)
    }

    pub(crate) fn begin_watch(&mut self, session_id: u64) -> Option<u64> {
        if self
            .sessions
            .get(&session_id)
            .is_none_or(|session| session.watch_id.is_some())
        {
            return None;
        }
        self.next_watch_id = self.next_watch_id.wrapping_add(1);
        let watch_id = self.next_watch_id;
        self.sessions
            .get_mut(&session_id)
            .expect("shared session was checked above")
            .watch_id = Some(watch_id);
        Some(watch_id)
    }

    pub(crate) fn watch_is_current(&self, session_id: u64, watch_id: u64) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| session.watch_id == Some(watch_id))
    }

    pub(crate) fn end_watch(&mut self, session_id: u64, watch_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id)
            && session.watch_id == Some(watch_id)
        {
            session.watch_id = None;
        }
    }

    pub(crate) fn state(&self, session_id: u64) -> Option<&SharedSessionState> {
        self.sessions.get(&session_id).map(|session| &session.state)
    }

    pub(crate) fn schedule_sync(&mut self, session_id: u64) -> Option<u64> {
        let session = self.sessions.get_mut(&session_id)?;
        if session.sync_in_flight {
            session.sync_pending = true;
            return None;
        }
        session.sync_generation = session.sync_generation.wrapping_add(1);
        session.sync_in_flight = true;
        Some(session.sync_generation)
    }

    pub(crate) fn sync_is_current(&self, session_id: u64, generation: u64) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| session.sync_generation == generation)
    }

    pub(crate) fn finish_sync(&mut self, session_id: u64, generation: u64) -> bool {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return false;
        };
        session.sync_in_flight = false;
        let pending = session.sync_pending || session.sync_generation != generation;
        session.sync_pending = false;
        pending
    }

    pub(crate) fn may_report_size(&self, session_id: u64) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| !session.sync_in_flight)
    }

    pub(crate) fn local_pane_id(&self, session_id: u64, mux_pane_id: u64) -> Option<u64> {
        self.sessions
            .get(&session_id)
            .and_then(|session| session.mux_to_local.get(&mux_pane_id).copied())
    }

    pub(crate) fn mux_pane_id(&self, session_id: u64, local_pane_id: u64) -> Option<u64> {
        self.sessions
            .get(&session_id)
            .and_then(|session| session.local_to_mux.get(&local_pane_id).copied())
    }

    fn panes_missing_from_snapshot(&self, session_id: u64, state: &SharedSessionState) -> Vec<u64> {
        let Some(session) = self.sessions.get(&session_id) else {
            return Vec::new();
        };
        let present = state.pane_ids().collect::<HashSet<_>>();
        session
            .local_to_mux
            .iter()
            .filter_map(|(local_id, mux_id)| (!present.contains(mux_id)).then_some(*local_id))
            .collect()
    }

    fn missing_panes_for_snapshot(&self, session_id: u64, state: &SharedSessionState) -> Vec<u64> {
        let Some(session) = self.sessions.get(&session_id) else {
            return Vec::new();
        };
        state
            .pane_ids()
            .filter(|mux_id| !session.mux_to_local.contains_key(mux_id))
            .collect()
    }

    pub(crate) fn record_pane(&mut self, session_id: u64, mux_pane_id: u64, local_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            if let Some(previous_local) = session.mux_to_local.insert(mux_pane_id, local_pane_id) {
                session.local_to_mux.remove(&previous_local);
            }
            if let Some(previous_mux) = session.local_to_mux.insert(local_pane_id, mux_pane_id) {
                session.mux_to_local.remove(&previous_mux);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_pane(&mut self, session_id: u64, mux_pane_id: u64) -> Option<u64> {
        let session = self.sessions.get_mut(&session_id)?;
        let local_pane_id = session.mux_to_local.remove(&mux_pane_id)?;
        session.local_to_mux.remove(&local_pane_id);
        Some(local_pane_id)
    }

    fn remove_local_pane(&mut self, session_id: u64, local_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id)
            && let Some(mux_pane_id) = session.local_to_mux.remove(&local_pane_id)
        {
            session.mux_to_local.remove(&mux_pane_id);
        }
    }

    /// Accepts a complete canonical snapshot and maps its opaque tab state
    /// into this window's ids. A stale event is harmless; a caller that needs
    /// the full state after a gap asks `zmux` directly and calls this again.
    pub(crate) fn apply_snapshot_to_tab(
        &mut self,
        session_id: u64,
        state: SharedSessionState,
        tab: &mut Tab,
    ) -> Result<SharedSnapshotDisposition> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Ok(SharedSnapshotDisposition::Unknown);
        };
        if state.revision < session.state.revision {
            return Ok(SharedSnapshotDisposition::Stale);
        }
        anyhow::ensure!(
            tab.id == session.tab_id,
            "shared session {session_id} is bound to tab {}, not {}",
            session.tab_id,
            tab.id
        );
        let mut tab_state: TabState = match serde_json::from_value(state.state.clone()) {
            Ok(state) => state,
            Err(_error) if state.state.is_null() => {
                // Sessions published by an older daemon may not have carried a
                // durable tab blob. Preserve the local model while the summary
                // supplies the authoritative shared layout below.
                TabState::from_tab(tab, &session.local_to_mux)
            }
            Err(error) => return Err(error).context("reading the shared tab state"),
        };
        remap_tab_state(&mut tab_state, &state.presentation, &session.mux_to_local)?;
        apply_tab_state(tab, tab_state);
        apply_canonical_presentation(tab, &state, &session.mux_to_local)?;
        session.state = state;
        Ok(SharedSnapshotDisposition::Applied)
    }

    /// Records a pane before its terminal is built. This is intentionally
    /// separate from `apply_snapshot_to_tab`: a pane-added event carries a
    /// state that references the new stable id, while its relay attachment is
    /// still in flight.
    pub(crate) fn accept_pane_added(
        &mut self,
        session_id: u64,
        state: SharedSessionState,
        mux_pane_id: u64,
        local_pane_id: u64,
    ) -> Result<SharedSnapshotDisposition> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Ok(SharedSnapshotDisposition::Unknown);
        };
        if state.revision < session.state.revision {
            return Ok(SharedSnapshotDisposition::Stale);
        }
        session.state = state;
        if let Some(previous_local) = session.mux_to_local.insert(mux_pane_id, local_pane_id) {
            session.local_to_mux.remove(&previous_local);
        }
        if let Some(previous_mux) = session.local_to_mux.insert(local_pane_id, mux_pane_id) {
            session.mux_to_local.remove(&previous_mux);
        }
        Ok(SharedSnapshotDisposition::Applied)
    }

    pub(crate) fn accept_pane_removed(
        &mut self,
        session_id: u64,
        state: SharedSessionState,
        mux_pane_id: u64,
    ) -> Result<Option<u64>> {
        let Some(session) = self.sessions.get_mut(&session_id) else {
            return Ok(None);
        };
        if state.revision < session.state.revision {
            return Ok(None);
        }
        let local_pane_id = session.mux_to_local.remove(&mux_pane_id);
        if let Some(local_pane_id) = local_pane_id {
            session.local_to_mux.remove(&local_pane_id);
        }
        session.state = state;
        Ok(local_pane_id)
    }
}

impl Zetta {
    pub(crate) fn has_shared_tab_binding(&self, tab_id: u64) -> bool {
        self.mux_panes
            .session_id(tab_id)
            .is_some_and(|session_id| self.shared_collaboration.is_bound(session_id))
    }

    /// Binds a tab that has just been offered to the daemon's authoritative
    /// snapshot before any shared mutation can be made. The stable pane ids are
    /// paired with this window's ids once, then every later event uses that
    /// translation rather than guessing from the order panes happen to have.
    #[cfg(feature = "zmux")]
    pub(super) fn bind_shared_session(
        &mut self,
        tab_id: u64,
        runtime: MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let session_id = self
            .mux_panes
            .session_id(tab_id)
            .with_context(|| format!("tab {tab_id} has no shared multiplexer session"))?;
        // Subscribe before fetching the snapshot. An operation committed in
        // between is then queued behind the snapshot instead of being lost.
        runtime.shared_reports().forget(session_id);
        let receiver = runtime.shared_reports().register(session_id);
        let state = match runtime.client().shared_snapshot(session_id) {
            Ok(state) => state,
            Err(error) => {
                runtime.shared_reports().forget(session_id);
                return Err(error);
            }
        };
        let mappings = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| {
                self.mux_panes
                    .mux_pane_id(pane.id)
                    .map(|mux_pane_id| (mux_pane_id, pane.id))
            })
            .collect::<Vec<_>>();
        self.shared_collaboration
            .bind(session_id, tab_id, state, mappings)?;
        self.watch_shared_session_with_receiver(session_id, runtime, receiver, window, cx);
        Ok(())
    }

    /// Stops applying shared snapshots while leaving the daemon's pane
    /// attachments alone. Unsharing deliberately keeps the local shared pane
    /// alive until the daemon's grant handover converts it back to exclusive.
    #[cfg(feature = "zmux")]
    pub(super) fn forget_shared_session(&mut self, tab_id: u64) {
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        self.shared_collaboration.forget(session_id);
        if let Some(runtime) = self.mux_panes.runtime_for_tab(tab_id) {
            runtime.shared_reports().forget(session_id);
        }
    }

    /// Queues one canonical shared mutation after local state changes. Geometry
    /// uses typed operations; the opaque replacement is limited to tab state the
    /// daemon does not interpret, such as labels, icons, and themes.
    pub(crate) fn sync_shared_tab_state(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(_runtime) = self.mux_panes.runtime_for_tab(tab_id) else {
            return;
        };
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        let Some(generation) = self.shared_collaboration.schedule_sync(session_id) else {
            return;
        };
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            // Pointer-driven pane resizing can produce dozens of local layout
            // states per frame. One daemon operation for the settled state also
            // prevents those edits from all racing on the same revision.
            executor.timer(Duration::from_millis(100)).await;
            let request = this
                .update(cx, |this, cx| {
                    if !this
                        .shared_collaboration
                        .sync_is_current(session_id, generation)
                    {
                        return None;
                    }
                    this.shared_tab_state_request(tab_id, session_id, cx)
                })
                .ok()
                .flatten();
            let Some((client, request)) = request else {
                this.update(cx, |this, cx| {
                    if this
                        .shared_collaboration
                        .finish_sync(session_id, generation)
                    {
                        this.sync_shared_tab_state(tab_id, cx);
                    }
                })
                .ok();
                return;
            };
            let result = cx
                .background_spawn(async move { client.apply_shared_with_request(request) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(zmux::client::SharedOperationResult::Applied(state))
                    | Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        if let Some(bound_tab_id) = this.shared_collaboration.tab_id(session_id) {
                            if let Some(tab) =
                                this.tabs.iter_mut().find(|tab| tab.id == bound_tab_id)
                                && let Err(error) = this
                                    .shared_collaboration
                                    .apply_snapshot_to_tab(session_id, state, tab)
                            {
                                log::debug!("could not apply canonical shared response: {error:#}");
                            }
                            cx.notify();
                        }
                    }
                    Err(error) => {
                        log::debug!("could not publish shared tab state: {error:#}");
                    }
                }
                if this
                    .shared_collaboration
                    .finish_sync(session_id, generation)
                {
                    this.sync_shared_tab_state(tab_id, cx);
                }
                this.report_all_shared_pane_sizes(tab_id, cx);
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn shared_tab_state_request(
        &self,
        tab_id: u64,
        session_id: u64,
        cx: &App,
    ) -> Option<(
        Arc<zmux::client::Client>,
        zmux::messages::SharedSessionOperationRequest,
    )> {
        let runtime = self.mux_panes.runtime_for_tab(tab_id)?;
        let canonical = self.shared_collaboration.state(session_id)?;
        let base_revision = canonical.revision;
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        let protected = tab
            .close_policy
            .background_authentication()
            .flatten()
            .is_some();
        let mut summary = self.background_session_summary(tab, protected, cx);
        if let Err(error) = remap_summary_to_mux(&mut summary, self.mux_panes.ids()) {
            log::debug!("could not serialize shared tab state: {error:#}");
            return None;
        }
        summary.id = session_id;
        let state = match serde_json::to_value(crate::session_state::TabState::from_tab(
            tab,
            self.mux_panes.ids(),
        )) {
            Ok(state) => state,
            Err(error) => {
                log::debug!("could not serialize shared tab state: {error:#}");
                return None;
            }
        };
        let client = runtime.client().clone();
        let maximized_pane = tab
            .maximized_pane
            .and_then(|pane_id| self.mux_panes.mux_pane_id(pane_id));
        let minimized_panes = tab
            .minimized_panes
            .iter()
            .filter_map(|pane_id| self.mux_panes.mux_pane_id(*pane_id))
            .collect::<Vec<_>>();
        let operation = if summary.layout != canonical.presentation.layout {
            shared_layout_operation(
                &canonical.presentation.layout,
                &summary.layout,
                summary.active_pane,
            )
            .unwrap_or(zmux::messages::SharedSessionOperation::SetLayout {
                layout: summary.layout.clone(),
            })
        } else if summary.active_pane != canonical.presentation.active_pane {
            zmux::messages::SharedSessionOperation::SetFocus {
                pane_id: summary.active_pane,
            }
        } else if maximized_pane != canonical.presentation.maximized_pane {
            zmux::messages::SharedSessionOperation::SetMaximized {
                pane_id: maximized_pane,
            }
        } else if minimized_panes != canonical.presentation.minimized_panes {
            let changed = minimized_panes
                .iter()
                .chain(&canonical.presentation.minimized_panes)
                .copied()
                .find(|pane_id| {
                    minimized_panes.contains(pane_id)
                        != canonical.presentation.minimized_panes.contains(pane_id)
                })?;
            zmux::messages::SharedSessionOperation::SetMinimized {
                pane_id: changed,
                minimized: minimized_panes.contains(&changed),
            }
        } else {
            zmux::messages::SharedSessionOperation::ReplaceTab { summary, state }
        };
        let request = zmux::messages::SharedSessionOperationRequest {
            session_id,
            base_revision,
            operation_id: client.next_shared_operation_id(),
            operation,
        };
        Some((client, request))
    }

    /// Keeps a shared tab subscribed to canonical collaboration snapshots.
    /// Requests that need a data stream (a pane added by another viewer) are
    /// performed on the background executor; applying the resulting terminal
    /// remains a window operation so local GPUI entities never cross threads.
    pub(super) fn watch_shared_session(
        &mut self,
        session_id: u64,
        runtime: MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let receiver = runtime.shared_reports().register(session_id);
        self.watch_shared_session_with_receiver(session_id, runtime, receiver, window, cx);
    }

    fn watch_shared_session_with_receiver(
        &mut self,
        session_id: u64,
        runtime: MuxRuntime,
        receiver: async_channel::Receiver<zmux::client::SharedSessionEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(watch_id) = self.shared_collaboration.begin_watch(session_id) else {
            return;
        };
        let connection = SharedWatchConnection {
            client: runtime.client().clone(),
            secret: runtime.session_secret(),
        };
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = receiver.recv().await {
                let current = this
                    .update(cx, |this, _| {
                        this.shared_collaboration
                            .watch_is_current(session_id, watch_id)
                    })
                    .unwrap_or(false);
                if !current {
                    break;
                }
                let task = match event {
                    zmux::client::SharedSessionEvent::Updated(state)
                    | zmux::client::SharedSessionEvent::PanesChanged { state, .. } => this
                        .update_in(cx, |this, window, cx| {
                            this.handle_shared_snapshot_event(
                                session_id,
                                state,
                                connection.clone(),
                                window,
                                cx,
                            )
                        })
                        .ok(),
                    zmux::client::SharedSessionEvent::PaneAdded {
                        session_id,
                        pane_id,
                        state,
                        ..
                    } => this
                        .update_in(cx, |this, window, cx| {
                            this.handle_shared_pane_added(
                                session_id,
                                pane_id,
                                state,
                                connection.clone(),
                                window,
                                cx,
                            )
                        })
                        .ok(),
                    zmux::client::SharedSessionEvent::PaneRemoved {
                        session_id,
                        pane_id,
                        state,
                    } => {
                        this.update_in(cx, |this, window, cx| {
                            this.remove_incoming_shared_pane(
                                session_id, pane_id, state, window, cx,
                            );
                        })
                        .ok();
                        None
                    }
                    zmux::client::SharedSessionEvent::StreamFailed {
                        session_id,
                        pane_id,
                    } => this
                        .update_in(cx, |this, window, cx| {
                            this.handle_shared_stream_failure(
                                session_id,
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
                this.shared_collaboration.end_watch(session_id, watch_id);
            })
            .ok();
        })
        .detach();
    }

    fn handle_shared_snapshot_event(
        &mut self,
        session_id: u64,
        mut state: SharedSessionState,
        connection: SharedWatchConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let missing = self
            .shared_collaboration
            .panes_missing_from_snapshot(session_id, &state);
        for local_pane_id in missing {
            self.remove_local_shared_pane(session_id, local_pane_id, cx);
        }
        let missing = self
            .shared_collaboration
            .missing_panes_for_snapshot(session_id, &state);
        cx.spawn_in(window, async move |this, cx| {
            for pane_id in missing {
                let mut attached = None;
                for delay in [
                    Duration::ZERO,
                    Duration::from_millis(50),
                    Duration::from_millis(150),
                ] {
                    if !delay.is_zero() {
                        cx.background_executor().timer(delay).await;
                    }
                    let refreshed = cx
                        .background_spawn({
                            let connection = connection.clone();
                            async move {
                                let state = connection.client.shared_snapshot(session_id)?;
                                let pane = if state.contains_pane(pane_id) {
                                    Some(connection.client.attach_shared_with_secret(
                                        session_id,
                                        pane_id,
                                        connection.secret.as_ref(),
                                    )?)
                                } else {
                                    None
                                };
                                Ok::<_, anyhow::Error>((state, pane))
                            }
                        })
                        .await;
                    match refreshed {
                        Ok((
                            latest,
                            Some(zmux::client::AttachOutcome::SharedAttached { pane, .. }),
                        )) => {
                            state = latest;
                            attached = Some(pane);
                            break;
                        }
                        Ok((latest, None)) => {
                            state = latest;
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(pane) = attached {
                    this.update_in(cx, |this, window, cx| {
                        this.attach_incoming_shared_pane(
                            session_id,
                            pane,
                            state.clone(),
                            window,
                            cx,
                        )
                    })
                    .ok();
                }
            }
            this.update_in(cx, |this, window, cx| {
                this.apply_shared_snapshot(session_id, state, window, cx);
            })
            .ok();
        })
    }

    fn handle_shared_pane_added(
        &mut self,
        session_id: u64,
        pane_id: u64,
        state: SharedSessionState,
        connection: SharedWatchConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let should_attach = self
            .shared_collaboration
            .local_pane_id(session_id, pane_id)
            .is_none();
        cx.spawn_in(window, async move |this, cx| {
            if !should_attach {
                return;
            }
            let attached = cx
                .background_spawn(async move {
                    connection.client.attach_shared_with_secret(
                        session_id,
                        pane_id,
                        connection.secret.as_ref(),
                    )
                })
                .await;
            let Ok(zmux::client::AttachOutcome::SharedAttached { pane, .. }) = attached else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                let result =
                    this.attach_incoming_shared_pane(session_id, pane, state.clone(), window, cx);
                if result.is_ok() {
                    this.apply_shared_snapshot(session_id, state, window, cx);
                }
            })
            .ok();
        })
    }

    fn handle_shared_stream_failure(
        &mut self,
        session_id: u64,
        pane_id: u64,
        connection: SharedWatchConnection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let executor = cx.background_executor().clone();
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
                            this.apply_shared_snapshot(session_id, state, window, cx);
                        })
                        .ok();
                        break;
                    }
                    _ => continue,
                };
                let replaced = this
                    .update_in(cx, |this, window, cx| {
                        let replaced =
                            this.replace_shared_pane_stream(session_id, pane_id, pane, window, cx);
                        if replaced {
                            this.apply_shared_snapshot(session_id, state, window, cx);
                        }
                        replaced
                    })
                    .unwrap_or(false);
                if replaced {
                    break;
                }
            }
        })
    }

    pub(crate) fn apply_shared_snapshot(
        &mut self,
        session_id: u64,
        state: SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
            return;
        };
        let stale = self
            .shared_collaboration
            .panes_missing_from_snapshot(session_id, &state);
        for local_pane_id in stale {
            self.remove_local_shared_pane(session_id, local_pane_id, cx);
        }
        let disposition = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .map(|tab| {
                self.shared_collaboration
                    .apply_snapshot_to_tab(session_id, state, tab)
            });
        match disposition.transpose() {
            Ok(Some(SharedSnapshotDisposition::Applied)) => {
                self.focus_active(window, cx);
                cx.notify();
                let this = cx.entity().downgrade();
                cx.defer(move |cx| {
                    this.update(cx, |this, cx| {
                        this.report_all_shared_pane_sizes(tab_id, cx);
                    })
                    .ok();
                });
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(error) => log::debug!("could not apply shared session snapshot: {error:#}"),
        }
    }

    fn remove_local_shared_pane(
        &mut self,
        session_id: u64,
        local_pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        self.drop_shared_pane(local_pane_id, cx);
        if let Some(tab_id) = self.shared_collaboration.tab_id(session_id)
            && let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id)
        {
            tab.remove_pane(local_pane_id);
        }
        self.mux_panes.forget_pane(local_pane_id);
        self.shared_collaboration
            .remove_local_pane(session_id, local_pane_id);
    }

    fn replace_shared_pane_stream(
        &mut self,
        session_id: u64,
        mux_pane_id: u64,
        replacement: zmux::client::SharedPane,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(local_pane_id) = self
            .shared_collaboration
            .local_pane_id(session_id, mux_pane_id)
        else {
            return false;
        };
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
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
            log::debug!(
                "could not replace shared stream for session {session_id} pane {mux_pane_id}: {error:#}"
            );
            return false;
        }
        if let Some(terminal) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(local_pane_id))
            .and_then(|pane| pane.terminal.clone())
        {
            self.schedule_shared_pane_size_report(local_pane_id, terminal, cx);
        }
        true
    }

    fn attach_incoming_shared_pane(
        &mut self,
        session_id: u64,
        pane: zmux::client::SharedPane,
        state: SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
            return Ok(());
        };
        let mux_pane_id = pane.pane_id();
        let local_pane_id = self.next_pane_id;
        self.next_pane_id += 1;
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
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.name.eq_ignore_ascii_case(profile_name))
            .cloned()
            .unwrap_or_else(|| Profile {
                name: profile_name.to_owned(),
                command: task::Shell::System,
                theme: None,
                dark_theme: None,
                icon: ProfileIcon::default(),
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
            .iter_mut()
            .find(|tab| tab.id == tab_id)
            .with_context(|| format!("shared session {session_id} tab is closed"))?;
        tab.push_pane(local_pane);
        self.mux_panes.record(local_pane_id, mux_pane_id);
        self.shared_collaboration
            .record_pane(session_id, mux_pane_id, local_pane_id);
        self.shared_collaboration.accept_pane_added(
            session_id,
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
            .tabs
            .iter()
            .position(|tab| tab.id == tab_id)
            .expect("the shared tab was checked above");
        let mut tab = self.tabs.remove(tab_index);
        self.build_attached_panes(
            &mut tab, session_id, attached, &restored, &runtime, window, cx,
        );
        self.tabs.insert(tab_index, tab);
        let view = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(local_pane_id))
            .and_then(|pane| pane.view.clone())
            .context("added shared pane did not build a terminal view")?;
        self.connect_terminal_view(tab_id, local_pane_id, view, window, cx);
        Ok(())
    }

    fn remove_incoming_shared_pane(
        &mut self,
        session_id: u64,
        mux_pane_id: u64,
        state: SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local_pane_id = self
            .shared_collaboration
            .accept_pane_removed(session_id, state.clone(), mux_pane_id)
            .ok()
            .flatten();
        if let Some(local_pane_id) = local_pane_id {
            self.drop_shared_pane(local_pane_id, cx);
        }
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
            return;
        };
        if let Some(local_pane_id) = local_pane_id {
            if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                tab.remove_pane(local_pane_id);
            }
            self.mux_panes.forget_pane(local_pane_id);
        }
        self.apply_shared_snapshot(session_id, state, window, cx);
        cx.notify();
    }
}

fn shared_layout_operation(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    active_pane: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    let mut change = None;
    if find_single_ratio_change(current, desired, &mut change) {
        return change.map(|(first_pane_id, second_pane_id, first_ratio)| {
            zmux::messages::SharedSessionOperation::SetSplitRatio {
                first_pane_id,
                second_pane_id,
                first_ratio,
            }
        });
    }
    let current_ids = layout_ids(current);
    let desired_ids = layout_ids(desired);
    if let Some(operation) = find_directional_move(current, desired, active_pane) {
        return Some(operation);
    }
    let different = current_ids
        .iter()
        .zip(&desired_ids)
        .filter_map(|(current, desired)| (current != desired).then_some((*current, *desired)))
        .collect::<Vec<_>>();
    if same_layout_shape(current, desired)
        && different.len() == 2
        && different[0] == (different[1].1, different[1].0)
    {
        return Some(zmux::messages::SharedSessionOperation::SwapPanes {
            first_pane_id: different[0].0,
            second_pane_id: different[0].1,
        });
    }
    find_rotation(current, desired, active_pane)
}

fn find_directional_move(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    use zmux::messages::SharedPaneDirection;
    for direction in [
        SharedPaneDirection::Left,
        SharedPaneDirection::Right,
        SharedPaneDirection::Up,
        SharedPaneDirection::Down,
    ] {
        let mut candidate = current.clone();
        if zmux::messages::move_layout_pane(&mut candidate, pane_id, direction)
            && candidate == *desired
        {
            return Some(zmux::messages::SharedSessionOperation::MovePane { pane_id, direction });
        }
    }
    None
}

fn find_rotation(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    pane_id: u64,
) -> Option<zmux::messages::SharedSessionOperation> {
    use zmux::messages::SharedRotationDirection;
    for direction in [
        SharedRotationDirection::Clockwise,
        SharedRotationDirection::CounterClockwise,
    ] {
        let mut candidate = current.clone();
        if zmux::messages::rotate_layout_pane(&mut candidate, pane_id, direction)
            && candidate == *desired
        {
            return Some(zmux::messages::SharedSessionOperation::RotateSplit {
                pane_id,
                direction,
            });
        }
    }
    None
}

fn find_single_ratio_change(
    current: &BackgroundPaneLayout,
    desired: &BackgroundPaneLayout,
    change: &mut Option<(u64, u64, u16)>,
) -> bool {
    match (current, desired) {
        (
            BackgroundPaneLayout::Pane { pane_id: current },
            BackgroundPaneLayout::Pane { pane_id: desired },
        ) => current == desired,
        (
            BackgroundPaneLayout::Split {
                axis: current_axis,
                first_ratio: current_ratio,
                first: current_first,
                second: current_second,
            },
            BackgroundPaneLayout::Split {
                axis: desired_axis,
                first_ratio: desired_ratio,
                first: desired_first,
                second: desired_second,
            },
        ) => {
            if current_axis != desired_axis {
                return false;
            }
            if current_ratio != desired_ratio {
                if change.is_some() {
                    return false;
                }
                *change = Some((
                    first_layout_pane(current_first),
                    first_layout_pane(current_second),
                    *desired_ratio,
                ));
            }
            find_single_ratio_change(current_first, desired_first, change)
                && find_single_ratio_change(current_second, desired_second, change)
        }
        _ => false,
    }
}

fn first_layout_pane(layout: &BackgroundPaneLayout) -> u64 {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => *pane_id,
        BackgroundPaneLayout::Split { first, .. } => first_layout_pane(first),
    }
}

fn layout_ids(layout: &BackgroundPaneLayout) -> Vec<u64> {
    match layout {
        BackgroundPaneLayout::Pane { pane_id } => vec![*pane_id],
        BackgroundPaneLayout::Split { first, second, .. } => {
            let mut ids = layout_ids(first);
            ids.extend(layout_ids(second));
            ids
        }
    }
}

fn same_layout_shape(current: &BackgroundPaneLayout, desired: &BackgroundPaneLayout) -> bool {
    match (current, desired) {
        (BackgroundPaneLayout::Pane { .. }, BackgroundPaneLayout::Pane { .. }) => true,
        (
            BackgroundPaneLayout::Split {
                axis: current_axis,
                first_ratio: current_ratio,
                first: current_first,
                second: current_second,
            },
            BackgroundPaneLayout::Split {
                axis: desired_axis,
                first_ratio: desired_ratio,
                first: desired_first,
                second: desired_second,
            },
        ) => {
            current_axis == desired_axis
                && current_ratio == desired_ratio
                && same_layout_shape(current_first, desired_first)
                && same_layout_shape(current_second, desired_second)
        }
        _ => false,
    }
}

fn remap_tab_state(
    tab_state: &mut TabState,
    presentation: &zmux::messages::SharedPresentationState,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<()> {
    let canonical_to_mux = canonical_pane_mappings(tab_state);
    tab_state.panes.retain_mut(|pane| {
        let Ok(id) = remap_canonical_id(pane.id, pane.mux_pane_id, &canonical_to_mux, mux_to_local)
        else {
            return false;
        };
        pane.id = id;
        pane.stack.retain_mut(|entry| {
            let Ok(id) =
                remap_canonical_id(entry.id, entry.mux_pane_id, &canonical_to_mux, mux_to_local)
            else {
                return false;
            };
            entry.id = id;
            true
        });
        true
    });
    anyhow::ensure!(
        !tab_state.panes.is_empty(),
        "shared tab state has no panes attached in this window"
    );
    // A pane lifecycle event can legitimately carry the previous opaque tab
    // blob. The summary is canonical for topology, so use its mapped layout
    // and focus before applying the durable per-tab fields from the blob.
    tab_state.layout = map_background_layout_state(&presentation.layout, mux_to_local)?;
    tab_state.active_pane = mux_to_local
        .get(&presentation.active_pane)
        .copied()
        .with_context(|| {
            format!(
                "active shared pane {} is not attached",
                presentation.active_pane
            )
        })?;
    tab_state.focus_history = tab_state
        .focus_history
        .iter()
        .filter_map(|id| remap_id(*id, &canonical_to_mux, mux_to_local).ok())
        .collect();
    tab_state.maximized_pane = tab_state
        .maximized_pane
        .and_then(|id| remap_id(id, &canonical_to_mux, mux_to_local).ok());
    tab_state.minimized_panes = tab_state
        .minimized_panes
        .iter()
        .filter_map(|id| remap_id(*id, &canonical_to_mux, mux_to_local).ok())
        .collect();
    tab_state.selected_minimized_pane = tab_state
        .selected_minimized_pane
        .and_then(|id| remap_id(id, &canonical_to_mux, mux_to_local).ok());
    Ok(())
}

fn map_background_layout_state(
    layout: &BackgroundPaneLayout,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<LayoutState> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => LayoutState::Pane {
            pane_id: mux_to_local
                .get(pane_id)
                .copied()
                .with_context(|| format!("shared pane {pane_id} is not attached"))?,
        },
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => LayoutState::Split {
            axis: if axis.eq_ignore_ascii_case("vertical") {
                AxisState::Vertical
            } else {
                AxisState::Horizontal
            },
            first_ratio: *first_ratio,
            first: Box::new(map_background_layout_state(first, mux_to_local)?),
            second: Box::new(map_background_layout_state(second, mux_to_local)?),
        },
    })
}

fn canonical_pane_mappings(tab_state: &TabState) -> HashMap<u64, u64> {
    tab_state
        .panes
        .iter()
        .flat_map(|pane| {
            std::iter::once((pane.id, pane.mux_pane_id))
                .chain(pane.stack.iter().map(|entry| (entry.id, entry.mux_pane_id)))
        })
        .filter_map(|(local_id, mux_id)| mux_id.map(|mux_id| (local_id, mux_id)))
        .collect()
}

fn remap_canonical_id(
    canonical_id: u64,
    mux_id: Option<u64>,
    canonical_to_mux: &HashMap<u64, u64>,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<u64> {
    let mux_id = mux_id
        .or_else(|| canonical_to_mux.get(&canonical_id).copied())
        .with_context(|| format!("shared pane {canonical_id} has no stable multiplexer id"))?;
    remap_id(canonical_id, canonical_to_mux, mux_to_local).or_else(|_| {
        mux_to_local
            .get(&mux_id)
            .copied()
            .with_context(|| format!("shared pane {mux_id} is not attached in this window"))
    })
}

fn remap_id(
    canonical_id: u64,
    canonical_to_mux: &HashMap<u64, u64>,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<u64> {
    let mux_id = canonical_to_mux
        .get(&canonical_id)
        .with_context(|| format!("shared pane {canonical_id} has no stable mapping"))?;
    mux_to_local
        .get(mux_id)
        .copied()
        .with_context(|| format!("shared pane {mux_id} is not attached in this window"))
}

fn apply_canonical_presentation(
    tab: &mut Tab,
    state: &SharedSessionState,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<()> {
    tab.layout = map_background_layout(&state.presentation.layout, mux_to_local)?;
    tab.active_pane = mux_to_local
        .get(&state.presentation.active_pane)
        .copied()
        .with_context(|| {
            format!(
                "active shared pane {} is not attached",
                state.presentation.active_pane
            )
        })?;
    tab.maximized_pane = state
        .presentation
        .maximized_pane
        .and_then(|pane_id| mux_to_local.get(&pane_id).copied());
    tab.minimized_panes = state
        .presentation
        .minimized_panes
        .iter()
        .filter_map(|pane_id| mux_to_local.get(pane_id).copied())
        .collect();
    tab.selected_minimized_pane = tab
        .selected_minimized_pane
        .filter(|pane_id| tab.minimized_panes.contains(pane_id));
    for pane in &state.summary.panes {
        if let Some(local_id) = mux_to_local.get(&pane.id).copied()
            && let Some(local) = tab.pane_mut(local_id)
        {
            apply_pane_summary(local, pane);
        }
    }
    Ok(())
}

fn map_background_layout(
    layout: &BackgroundPaneLayout,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<PaneLayout> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => PaneLayout::Pane(
            mux_to_local
                .get(pane_id)
                .copied()
                .with_context(|| format!("shared pane {pane_id} is not attached"))?,
        ),
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => PaneLayout::Split {
            axis: if axis.eq_ignore_ascii_case("vertical") {
                SplitAxis::Vertical
            } else {
                SplitAxis::Horizontal
            },
            first_ratio: *first_ratio,
            first: Box::new(map_background_layout(first, mux_to_local)?),
            second: Box::new(map_background_layout(second, mux_to_local)?),
        },
    })
}

fn remap_summary_to_mux(
    summary: &mut BackgroundSessionSummary,
    local_to_mux: &HashMap<u64, u64>,
) -> Result<()> {
    summary.active_pane = local_to_mux
        .get(&summary.active_pane)
        .copied()
        .context("the active shared pane has no multiplexer id")?;
    for pane in &mut summary.panes {
        pane.id = local_to_mux
            .get(&pane.id)
            .copied()
            .with_context(|| format!("shared pane {} has no multiplexer id", pane.id))?;
    }
    summary.layout = remap_summary_layout(&summary.layout, local_to_mux)?;
    Ok(())
}

fn remap_summary_layout(
    layout: &BackgroundPaneLayout,
    local_to_mux: &HashMap<u64, u64>,
) -> Result<BackgroundPaneLayout> {
    Ok(match layout {
        BackgroundPaneLayout::Pane { pane_id } => BackgroundPaneLayout::Pane {
            pane_id: local_to_mux
                .get(pane_id)
                .copied()
                .with_context(|| format!("shared pane {pane_id} has no multiplexer id"))?,
        },
        BackgroundPaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        } => BackgroundPaneLayout::Split {
            axis: axis.clone(),
            first_ratio: *first_ratio,
            first: Box::new(remap_summary_layout(first, local_to_mux)?),
            second: Box::new(remap_summary_layout(second, local_to_mux)?),
        },
    })
}

fn apply_tab_state(tab: &mut Tab, state: TabState) {
    let icon = state
        .icon
        .as_deref()
        .and_then(crate::tab_icon_picker::parse_tab_icon_name);
    let icon_override = match state.icon_override {
        None => TabIconOverride::None,
        Some(None) => TabIconOverride::Hidden,
        Some(Some(name)) => crate::tab_icon_picker::parse_tab_icon_name(&name)
            .map(TabIconOverride::Icon)
            .unwrap_or_default(),
    };
    tab.icon = match icon_override {
        TabIconOverride::None => icon,
        TabIconOverride::Icon(icon) => Some(icon),
        TabIconOverride::Hidden => None,
    };
    tab.icon_override = icon_override;
    tab.next_pane_label = state.next_pane_label;
    tab.layout = state.layout.into_layout();
    tab.active_pane = state.active_pane;
    tab.focus_history = state.focus_history;
    tab.maximized_pane = state.maximized_pane;
    tab.minimized_panes = state.minimized_panes;
    tab.selected_minimized_pane = state.selected_minimized_pane;
    tab.broadcast_input = state.broadcast_input;
    tab.silent_mode = state.silent_mode;
    tab.shared = state.shared;
    tab.close_policy = if state.keep_running {
        TabClosePolicy::Background {
            authentication: None,
        }
    } else {
        TabClosePolicy::Close
    };
    tab.custom_title = state.custom_title;
    tab.worktree_seed_title = state.worktree_seed_title;
    tab.process_title = state.process_title;
    tab.pinned = state.pinned;
    tab.theme_override = state.theme_override;
    for pane_state in state.panes {
        let Some(pane) = tab.pane_mut(pane_state.id) else {
            continue;
        };
        apply_pane_state(pane, pane_state);
    }
}

fn apply_pane_state(pane: &mut TerminalPane, state: PaneState) {
    pane.label_number = state.label_number;
    pane.generated_label = state.generated_label;
    pane.custom_label = state.custom_label;
    pane.theme_override = state.theme_override;
    pane.environment_overrides = state.environment_overrides;
    pane.exit = state.exit;
    pane.base_exited = state.base_exited;
    pane.pending_command = state.pending_command;
    pane.active_command = state.active_command;
    pane.detected_worktree_title = state.detected_worktree_title;
    apply_pane_overlay(pane, state.overlay.as_ref());
}

fn apply_pane_overlay(
    pane: &mut TerminalPane,
    overlay: Option<&crate::session_state::OverlayState>,
) {
    let Some(overlay) = overlay else {
        pane.overlay_text = None;
        pane.overlay_font_size = None;
        pane.overlay_opacity = None;
        pane.overlay_color = None;
        return;
    };
    pane.overlay_text = Some(overlay.text.clone());
    pane.overlay_font_size = overlay
        .font_size
        .as_deref()
        .and_then(crate::OverlayFontSize::parse);
    pane.overlay_opacity = overlay.opacity;
    pane.overlay_color = overlay.color.map(|[h, s, l, a]| gpui::Hsla { h, s, l, a });
}

fn apply_pane_summary(pane: &mut TerminalPane, summary: &BackgroundPaneSummary) {
    if pane.custom_label.is_none() && !summary.label.is_empty() {
        pane.generated_label = Some(summary.label.clone());
    }
    pane.base_exited = matches!(summary.state, BackgroundPaneState::Exited);
    pane.exit = summary.exit.clone();
}

#[cfg(test)]
#[path = "../tests/background_session_ui/collaboration.rs"]
mod tests;
