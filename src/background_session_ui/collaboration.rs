//! The window-side model for a daemon-owned shared session.
//!
//! `zmux` addresses panes by stable ids, while every Zetta window has its own
//! pane-id namespace. Keeping that translation here makes an incoming
//! snapshot safe to apply even when the session was published by another
//! window, and gives lifecycle events one place to update the mapping before
//! a terminal is attached.

use super::*;

use super::multiplexer::AttachedPaneKind;
use crate::mux::SharedSessionKey;
use crate::session_state::{LayoutState, PaneState, TabState};
use zmux::messages::SharedSessionState;
use zmux::protocol::{BackgroundPaneLayout, BackgroundPaneSummary};

mod coordinator;
pub(crate) use coordinator::*;
mod layout_mapping;
pub(crate) use layout_mapping::*;
mod watch;

/// How long local tab state settles before it is published. Pointer-driven
/// pane resizing produces dozens of layout states per frame and only the last
/// one is worth a round trip.
const SHARED_PUBLICATION_DEBOUNCE: Duration = Duration::from_millis(100);

/// How many times a pane close is asked for before the pane is given back to
/// the user with an explanation.
const SHARED_CLOSE_ATTEMPTS: u32 = 4;

/// How many requests one publication may take. A publication converges the
/// daemon on one dimension at a time — geometry first, then the durable tab
/// state — and re-queues itself until nothing differs. The bound is what stops
/// geometry the daemon will not accept from re-queuing for ever.
const SHARED_PUBLISH_ATTEMPTS: u32 = 4;

/// Multiplied by the attempt number, so the retries are spread rather than
/// stacked on a daemon that is busy committing somebody else's operation.
const SHARED_CLOSE_RETRY_BACKOFF: Duration = Duration::from_millis(120);

/// Focus travels independently from the debounced tab publication, but uses
/// the same bounded retry policy as a shared close.
const SHARED_FOCUS_ATTEMPTS: u32 = 4;

impl Zetta {
    pub(crate) fn has_shared_tab_binding(&self, tab_id: u64) -> bool {
        self.mux_panes
            .shared_key(tab_id)
            .is_some_and(|key| self.shared_collaboration.is_bound(&key))
    }

    /// Removes a pane the daemon never committed, and says why.
    ///
    /// A split in a shared tab creates its pane locally first and offers it to
    /// the session as a draft — that placeholder is what the proposed layout is
    /// written around. When the session refuses the proposal, the draft exists
    /// on no viewer and has a place in no canonical layout, so it is taken back
    /// rather than left as a pane that renders nowhere and still counts.
    pub(crate) fn discard_uncommitted_shared_pane(
        &mut self,
        tab_id: u64,
        pane_id: u64,
        reason: String,
        cx: &mut Context<Self>,
    ) {
        if self.mux_panes.mux_pane_id(pane_id).is_some() {
            // It committed after all, on another path. Report against the pane.
            self.report_pane_spawn_error(tab_id, pane_id, reason, cx);
            return;
        }
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == tab_id && tab.panes.len() > 1)
        {
            detach_pane_from_tab(tab, pane_id);
        }
        self.projects.forget_pane(pane_id);
        self.forget_pane_controls([pane_id]);
        self.closing_shared_panes.remove(&pane_id);
        self.show_error_notice(reason, cx);
    }

    /// Drops this window's view of any pane the session no longer holds.
    ///
    /// Geometry is proposed in the session's pane ids, translated from this
    /// window's own, so a translation kept for a pane the session dropped is
    /// not geometry the session can accept — it refuses the whole request, and
    /// goes on refusing it, because nothing about the window has changed. Run
    /// before a proposal is built rather than after it is rejected, and against
    /// the snapshot the window already holds, so it costs no round trip.
    ///
    /// `install_shared_snapshot` handles the panes this window knows are the
    /// session's. The loop before it is for the ones it does not: the map that
    /// translates a proposal is `mux_panes`, which outlives the collaboration
    /// binding's own, and an entry only in there is invisible to a snapshot.
    pub(crate) fn reconcile_shared_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(key) = self.mux_panes.shared_key(tab_id) else {
            return;
        };
        let session_id = key.session_id;
        let Some(state) = self.shared_collaboration.state(&key).cloned() else {
            return;
        };
        let stale = self
            .tabs
            .by_id(tab_id)
            .map(|tab| panes_the_session_lost(tab, self.mux_panes.ids(), &state))
            .unwrap_or_default();
        if !stale.is_empty() {
            log::warn!(
                "shared session {session_id} no longer holds the pane(s) behind local pane(s) \
                 {stale:?}; dropping this window's view of them"
            );
        }
        for pane_id in stale {
            self.remove_local_shared_pane(&key, pane_id, cx);
        }
        // The session's own layout is deliberately *not* installed here. This
        // runs while a pane the session has not been told about is already in
        // the tab — the draft a split is about to propose — and the session's
        // layout has no place for one. Installing it took the draft out of the
        // layout, and the proposal built from that layout then had nothing to
        // describe: every split in a shared tab failed. Canonical geometry
        // arrives through the snapshot path, which runs when the session has
        // something to say.
    }

    /// Whether every pane a proposal names is one the session holds. The safety
    /// net behind [`Zetta::reconcile_shared_tab`]: a proposal that fails this
    /// would be refused in full, and refusing it here keeps the refusal out of
    /// the session's history.
    pub(crate) fn shared_geometry_is_current(&self, key: &SharedSessionKey, named: &[u64]) -> bool {
        let session_id = key.session_id;
        let Some(state) = self.shared_collaboration.state(key) else {
            return false;
        };
        let stale = named
            .iter()
            .filter(|pane_id| !state.contains_pane(**pane_id))
            .collect::<Vec<_>>();
        if !stale.is_empty() {
            log::warn!(
                "shared session {session_id} does not hold pane(s) {stale:?} that this window's \
                 layout names; the proposal was not sent"
            );
        }
        stale.is_empty()
    }

    /// Whether this pane is waiting for the daemon to confirm its close. Such a
    /// pane is still on screen and still receiving its session's output, but it
    /// takes no input and cannot be closed again.
    pub(crate) fn shared_pane_is_closing(&self, pane_id: u64) -> bool {
        // Asked once per pane per frame, and the set is empty in every frame
        // but the few between asking for a close and hearing back. The emptiness
        // test is free; hashing the id is not.
        !self.closing_shared_panes.is_empty() && self.closing_shared_panes.contains(&pane_id)
    }

    /// Forgets a pane's stable-id mapping when it leaves a tab by a route that
    /// is not the shared-session remover — an ordinary local close, or a tab
    /// being closed. See [`SharedSessionCoordinator::remove_local_pane`].
    pub(crate) fn forget_shared_pane_mapping(&mut self, tab_id: u64, pane_id: u64) {
        let Some(key) = self.mux_panes.shared_key(tab_id) else {
            return;
        };
        let key = &key;
        self.shared_collaboration.remove_local_pane(key, pane_id);
    }

    /// Binds a tab that has just been offered to the daemon's authoritative
    /// snapshot before any shared mutation can be made. The stable pane ids are
    /// paired with this window's ids once, then every later event uses that
    /// translation rather than guessing from the order panes happen to have.
    ///
    /// The subscription and snapshot were fetched off this thread by
    /// [`super::handover::fetch_shared_binding`]; this only installs them.
    #[cfg(feature = "zmux")]
    pub(super) fn install_shared_binding(
        &mut self,
        tab_id: u64,
        runtime: MuxRuntime,
        binding: super::handover::SharedBinding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let session_id = self
            .mux_panes
            .session_id(tab_id)
            .with_context(|| format!("tab {tab_id} has no shared multiplexer session"))?;
        let super::handover::SharedBinding { receiver, state } = binding;
        let mappings = self
            .tabs
            .by_id(tab_id)
            .into_iter()
            .flat_map(|tab| tab.panes.iter())
            .filter_map(|pane| {
                self.mux_panes
                    .mux_pane_id(pane.id)
                    .map(|mux_pane_id| (mux_pane_id, pane.id))
            })
            .collect::<Vec<_>>();
        let key = SharedSessionKey::new(&runtime, session_id);
        if let Err(error) = self
            .shared_collaboration
            .bind(&key, tab_id, state, mappings)
        {
            runtime.shared_reports().forget(session_id);
            return Err(error);
        }
        self.watch_shared_session_with_receiver(&key, runtime, receiver, window, cx);
        Ok(())
    }

    /// Stops applying shared snapshots while leaving the daemon's pane
    /// attachments alone. Unsharing deliberately keeps the local shared pane
    /// alive until the daemon's grant handover converts it back to exclusive.
    #[cfg(feature = "zmux")]
    pub(super) fn forget_shared_session(&mut self, tab_id: u64) {
        let Some(key) = self.mux_panes.shared_key(tab_id) else {
            return;
        };
        let key = &key;
        let session_id = key.session_id;
        self.shared_collaboration.forget(key);
        if let Some(runtime) = self.mux_panes.runtime_for_tab(tab_id) {
            runtime.shared_reports().forget(session_id);
        }
    }

    /// Queues one canonical publication after local state changes. Geometry uses
    /// typed operations; everything else the daemon stores without reading —
    /// labels, icons, titles, themes and overlays — travels as the opaque tab
    /// blob. A publication converges both, in that order, so a change to one is
    /// never dropped because the other also changed.
    ///
    /// Debounced rather than sent: pointer-driven pane resizing produces dozens
    /// of local layout states per frame, and only the settled one is worth a
    /// round trip.
    pub(crate) fn sync_shared_tab_state(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let Some(key) = self.mux_panes.shared_key(tab_id) else {
            return;
        };
        let key = &key;
        if !self.shared_collaboration.schedule_publication(key) {
            return;
        }
        let executor = cx.background_executor().clone();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            executor.timer(SHARED_PUBLICATION_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.shared_collaboration.clear_publication_schedule(&key);
                this.shared_collaboration
                    .enqueue(&key, SharedOperation::PublishState { attempts: 0 });
                this.pump_shared_operations(tab_id, &key, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Sends keyboard pane focus through the shared-operation queue without
    /// waiting for the normal state-publication debounce. Local pointer focus
    /// continues to use the ordinary publication path; this is deliberately
    /// called only by the keyboard pane-navigation action.
    pub(crate) fn focus_shared_pane(
        &mut self,
        tab_id: u64,
        local_pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = self.mux_panes.shared_key(tab_id) else {
            return;
        };
        let key = &key;
        let Some(mux_pane_id) = self.shared_collaboration.mux_pane_id(key, local_pane_id) else {
            return;
        };
        if self
            .shared_collaboration
            .request_focus(key, mux_pane_id)
            .is_some()
        {
            self.pump_shared_operations(tab_id, key, cx);
        }
    }

    /// Runs the session's next queued mutation, if one is not already running.
    ///
    /// Each operation is built from the tab and rebased on the canonical
    /// revision at the moment it is sent, never at the moment it was queued, so
    /// waiting behind another operation cannot make it stale.
    pub(crate) fn pump_shared_operations(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        cx: &mut Context<Self>,
    ) {
        let Some(operation) = self.shared_collaboration.take_next_operation(key) else {
            return;
        };
        match operation {
            SharedOperation::PublishState { attempts } => {
                self.run_shared_publication(tab_id, key, attempts, cx);
            }
            SharedOperation::FocusPane {
                mux_pane_id,
                generation,
                attempts,
            } => self.run_shared_focus(tab_id, key, mux_pane_id, generation, attempts, cx),
            SharedOperation::ClosePane {
                local_pane_id,
                mux_pane_id,
                attempts,
            } => self.run_shared_pane_close(tab_id, key, local_pane_id, mux_pane_id, attempts, cx),
        }
    }

    fn finish_shared_operation(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        cx: &mut Context<Self>,
    ) {
        self.shared_collaboration.finish_operation(key);
        self.report_all_shared_pane_sizes(tab_id, cx);
        self.pump_shared_operations(tab_id, key, cx);
    }

    /// Converges the tab on what the daemon answered a publication with, and
    /// records the blob as published when it was the daemon that accepted it.
    fn install_shared_publication_response(
        &mut self,
        key: &SharedSessionKey,
        published_state: Option<serde_json::Value>,
        state: SharedSessionState,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        if let Some(published_state) = published_state {
            self.shared_collaboration
                .record_published_state(key, published_state);
        }
        let Some(tab_id) = self.shared_collaboration.tab_id(key) else {
            return;
        };
        if let Some(tab) = self.tabs.by_id_mut(tab_id)
            && let Err(error) = self
                .shared_collaboration
                .apply_snapshot_to_tab(key, state, tab)
        {
            log::warn!(
                "could not apply the canonical response to publishing shared session \
                 {session_id}: {error:#}"
            );
        }
        cx.notify();
    }

    /// Sends one of a publication's two halves and queues the rest of it.
    ///
    /// Geometry goes first, because the durable half is built from the tab as it
    /// stands once the daemon's answer has been installed. A publication that
    /// sent geometry therefore re-queues itself: the follow-up finds geometry
    /// converged and carries the tab blob. A publication that sent the blob is
    /// the end of the chain unless the daemon refused it for a revision that
    /// moved underneath it.
    fn run_shared_publication(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        let Some((client, request, published_state)) =
            self.shared_publication_request(tab_id, key, cx)
        else {
            self.finish_shared_operation(tab_id, key, cx);
            return;
        };
        let sent_durable_state = published_state.is_some();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.apply_shared_with_request(request) })
                .await;
            this.update(cx, |this, cx| {
                let settled = match result {
                    Ok(zmux::client::SharedOperationResult::Applied(state)) => {
                        this.install_shared_publication_response(&key, published_state, state, cx);
                        sent_durable_state
                    }
                    // Refused for a revision that moved underneath the request,
                    // which for an exact-revision operation needs no more than
                    // another viewer changing the geometry. Worth one more try
                    // from the state the refusal carried.
                    Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        this.install_shared_publication_response(&key, None, state, cx);
                        false
                    }
                    // Nothing came back to converge on, so there is nothing to
                    // rebase a second attempt against either. The next local
                    // change publishes again.
                    Err(error) => {
                        log::warn!(
                            "could not publish shared session {session_id} tab state: {error:#}"
                        );
                        true
                    }
                };
                if !settled && let Some(retry) = publication_retry(attempts) {
                    this.shared_collaboration.enqueue(&key, retry);
                }
                this.finish_shared_operation(tab_id, &key, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Sends a keyboard focus request against the revision current at send
    /// time. A newer keystroke may replace the intent while an older request is
    /// in flight; its response is then allowed to update canonical metadata but
    /// cannot settle the newer intent.
    fn run_shared_focus(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        generation: u64,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        let Some((client, request)) =
            self.shared_focus_request(tab_id, key, mux_pane_id, generation)
        else {
            self.finish_shared_operation(tab_id, key, cx);
            return;
        };
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { client.apply_shared_with_request(request) })
                .await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok(zmux::client::SharedOperationResult::Applied(state))
                    | Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        this.install_shared_snapshot(&key, state, cx);
                    }
                    Err(error) => log::warn!(
                        "could not focus shared pane {mux_pane_id} of session {session_id}: {error:#}"
                    ),
                }
                if this.shared_collaboration.focus_intent_is_current(
                    &key,
                    mux_pane_id,
                    generation,
                ) {
                    if let Some(SharedOperation::FocusPane { attempts, .. }) =
                        focus_retry(mux_pane_id, generation, attempts)
                    {
                        this.schedule_shared_focus_retry(
                            tab_id,
                            &key,
                            mux_pane_id,
                            generation,
                            attempts,
                            cx,
                        );
                    } else {
                        this.shared_collaboration.clear_focus_intent(
                            &key,
                            mux_pane_id,
                            generation,
                        );
                    }
                }
                this.finish_shared_operation(tab_id, &key, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Wait before re-queueing a failed focus request rather than holding the
    /// session queue in flight. The next run therefore reads the latest
    /// canonical revision immediately before it sends.
    fn schedule_shared_focus_retry(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        generation: u64,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let executor = cx.background_executor().clone();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            executor.timer(SHARED_CLOSE_RETRY_BACKOFF * attempts).await;
            this.update(cx, |this, cx| {
                if this
                    .shared_collaboration
                    .focus_intent_is_current(&key, mux_pane_id, generation)
                {
                    this.shared_collaboration.enqueue(
                        &key,
                        SharedOperation::FocusPane {
                            mux_pane_id,
                            generation,
                            attempts,
                        },
                    );
                    this.pump_shared_operations(tab_id, &key, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Asks the daemon to close a pane for every viewer, and keeps asking.
    ///
    /// The pane is not removed here. It leaves this window when the canonical
    /// state says it is gone — either through the snapshot this call answers
    /// with, or through the `SharedPanesChanged` event the daemon broadcasts —
    /// so a close that the daemon refuses leaves a pane that is still usable
    /// rather than a region nothing draws.
    fn run_shared_pane_close(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        local_pane_id: u64,
        mux_pane_id: u64,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        let Some(runtime) = self.mux_panes.runtime_for_tab(tab_id) else {
            self.abandon_shared_pane_close(tab_id, key, local_pane_id, cx);
            // Still finished, even though nothing was sent: an operation taken
            // off the queue holds the session until it reports back, so
            // returning here would wedge every later close and publication.
            self.finish_shared_operation(tab_id, key, cx);
            return;
        };
        let client = runtime.client().clone();
        let base_revision = self
            .shared_collaboration
            .state(key)
            .map(|state| state.revision);
        let executor = cx.background_executor().clone();
        let key = key.clone();
        cx.spawn(async move |this, cx| {
            if attempts > 0 {
                executor
                    .timer(SHARED_CLOSE_RETRY_BACKOFF * attempts)
                    .await;
            }
            let outcome = cx
                .background_spawn(async move {
                    let base_revision = match base_revision {
                        Some(revision) => revision,
                        None => client.shared_snapshot(session_id)?.revision,
                    };
                    client.apply_shared_with_request(
                        zmux::messages::SharedSessionOperationRequest {
                            session_id,
                            base_revision,
                            operation_id: client.next_shared_operation_id(),
                            operation: zmux::messages::SharedSessionOperation::ClosePane {
                                pane_id: mux_pane_id,
                            },
                        },
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let settled = match outcome {
                    Ok(zmux::client::SharedOperationResult::Applied(state)) => {
                        this.install_shared_snapshot(&key, state, cx);
                        true
                    }
                    Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        // The daemon validated the close against a tree this
                        // window had not caught up with. If the pane is gone
                        // from it, somebody else already closed it.
                        let gone = !state.contains_pane(mux_pane_id);
                        this.install_shared_snapshot(&key, state, cx);
                        gone
                    }
                    Err(error) => {
                        log::warn!(
                            "could not close shared pane {mux_pane_id} of session {session_id}: {error:#}"
                        );
                        false
                    }
                };
                if !settled {
                    if attempts + 1 >= SHARED_CLOSE_ATTEMPTS {
                        this.abandon_shared_pane_close(tab_id, &key, local_pane_id, cx);
                    } else {
                        this.shared_collaboration.enqueue(
                            &key,
                            SharedOperation::ClosePane {
                                local_pane_id,
                                mux_pane_id,
                                attempts: attempts + 1,
                            },
                        );
                    }
                }
                this.finish_shared_operation(tab_id, &key, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Gives up on a close the daemon will not commit. The pane goes back to
    /// being usable and says why, which is the one outcome that does not leave
    /// the window and the daemon disagreeing.
    fn abandon_shared_pane_close(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        local_pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        let session_id = key.session_id;
        self.closing_shared_panes.remove(&local_pane_id);
        let label = self
            .tabs
            .pane_in(tab_id, local_pane_id)
            .map_or_else(|| local_pane_id.to_string(), TerminalPane::label);
        log::warn!(
            "shared session {session_id} would not close pane {local_pane_id}; leaving it open"
        );
        self.show_error_notice(format!(
            "The session's multiplexer would not close pane {label}. It is still running for every viewer."
        ), cx);
    }

    /// The next request a publication owes the daemon, and — when that request
    /// is the durable half — the blob it carries, so the caller can record it as
    /// published once the daemon has taken it.
    ///
    /// `None` means this window and the daemon already agree and nothing needs
    /// sending.
    fn shared_publication_request(
        &self,
        tab_id: u64,
        key: &SharedSessionKey,
        cx: &App,
    ) -> Option<(
        Arc<zmux::client::Client>,
        zmux::messages::SharedSessionOperationRequest,
        Option<serde_json::Value>,
    )> {
        let session_id = key.session_id;
        let runtime = self.mux_panes.runtime_for_tab(tab_id)?;
        let canonical = self.shared_collaboration.state(key)?;
        let base_revision = canonical.revision;
        let tab = self.tabs.by_id(tab_id)?;
        if tab.panes.is_empty() {
            // A tab can briefly be empty while an attach/rebuild is in flight.
            // Its delayed publication must never replace a session that still
            // owns panes: doing so loses the durable mapping needed to attach
            // those terminals later.
            log::error!("refusing to publish empty tab {tab_id} over shared session {session_id}");
            return None;
        }
        let summary = self.shared_summary_in_mux_ids(tab, key, cx);
        let maximized_pane = tab
            .maximized_pane
            .and_then(|pane_id| self.mux_panes.mux_pane_id(pane_id));
        let minimized_panes = tab
            .minimized_panes
            .iter()
            .filter_map(|pane_id| self.mux_panes.mux_pane_id(*pane_id))
            .collect::<Vec<_>>();
        let geometry = summary.as_ref().and_then(|summary| {
            shared_geometry_operation(
                &canonical.presentation,
                summary,
                maximized_pane,
                &minimized_panes,
                self.shared_collaboration.has_pending_focus(key),
            )
        });
        let (operation, published_state) = match geometry {
            Some(operation) => (operation, None),
            None => {
                let (operation, state) = self.shared_durable_operation(tab, key, summary)?;
                (operation, Some(state))
            }
        };
        let client = runtime.client().clone();
        let request = zmux::messages::SharedSessionOperationRequest {
            session_id,
            base_revision,
            operation_id: client.next_shared_operation_id(),
            operation,
        };
        Some((client, request, published_state))
    }

    /// Builds the explicit keyboard-focus request only while it still names
    /// this session's newest intent. It reads the canonical revision here,
    /// immediately before the request is spawned, rather than when the key was
    /// pressed or when a previous queued operation completed.
    fn shared_focus_request(
        &mut self,
        tab_id: u64,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        generation: u64,
    ) -> Option<(
        Arc<zmux::client::Client>,
        zmux::messages::SharedSessionOperationRequest,
    )> {
        let session_id = key.session_id;
        if !self
            .shared_collaboration
            .focus_intent_is_current(key, mux_pane_id, generation)
        {
            return None;
        }
        if !self
            .shared_collaboration
            .state(key)
            .is_some_and(|state| state.contains_pane(mux_pane_id))
        {
            self.shared_collaboration
                .clear_focus_intent(key, mux_pane_id, generation);
            return None;
        }
        let canonical = self.shared_collaboration.state(key)?;
        let client = self.mux_panes.runtime_for_tab(tab_id)?.client().clone();
        Some((
            client.clone(),
            zmux::messages::SharedSessionOperationRequest {
                session_id,
                base_revision: canonical.revision,
                operation_id: client.next_shared_operation_id(),
                operation: zmux::messages::SharedSessionOperation::SetFocus {
                    pane_id: mux_pane_id,
                },
            },
        ))
    }

    /// This window's summary of the tab, in the daemon's pane ids.
    ///
    /// `None` when a pane of the tab has no multiplexer id yet — the draft of a
    /// split still in flight. That makes the summary undescribable, and with it
    /// every geometry operation, but not the durable tab state: the blob records
    /// `mux_pane_id: None` for such a pane and the receiving side already drops
    /// it, so the icon, the titles and the pane names still travel.
    fn shared_summary_in_mux_ids(
        &self,
        tab: &Tab,
        key: &SharedSessionKey,
        cx: &App,
    ) -> Option<BackgroundSessionSummary> {
        let session_id = key.session_id;
        let protected = tab
            .close_policy
            .background_authentication()
            .flatten()
            .is_some();
        self.multiplexer_session_summary(tab, session_id, protected, cx)
            .map_err(|error| {
                log::warn!(
                    "could not describe shared session {session_id} in the multiplexer's pane ids: {error:#}"
                );
            })
            .ok()
    }

    /// The durable tab state to publish and the blob it carries, or `None` when
    /// this window already published exactly these bytes.
    fn shared_durable_operation(
        &self,
        tab: &Tab,
        key: &SharedSessionKey,
        summary: Option<BackgroundSessionSummary>,
    ) -> Option<(zmux::messages::SharedSessionOperation, serde_json::Value)> {
        let session_id = key.session_id;
        let state = match serde_json::to_value(crate::session_state::TabState::from_tab(
            tab,
            self.mux_panes.ids(),
        )) {
            Ok(state) => state,
            Err(error) => {
                log::warn!("could not serialize shared session {session_id} tab state: {error:#}");
                return None;
            }
        };
        if !self
            .shared_collaboration
            .durable_state_is_unpublished(key, &state)
        {
            return None;
        }
        Some((durable_state_operation(state.clone(), summary), state))
    }
}

/// Asks the daemon for a pane's stream, retrying a few times.
///
/// The attach can arrive before the daemon has finished committing the pane, so
/// a failure is retried rather than reported. `Some((state, None))` means the
/// pane is no longer in the canonical state and there is nothing to attach —
/// the caller still applies that state, because it is newer than the one that
/// named the pane.
async fn attach_shared_pane_with_retries(
    session_id: u64,
    mux_pane_id: u64,
    connection: &SharedWatchConnection,
    cx: &mut gpui::AsyncApp,
) -> Option<(SharedSessionState, Option<zmux::client::SharedPane>)> {
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
                    let pane = if state.contains_pane(mux_pane_id) {
                        Some(connection.client.attach_shared_with_secret(
                            session_id,
                            mux_pane_id,
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
            Ok((state, Some(zmux::client::AttachOutcome::SharedAttached { pane, .. }))) => {
                return Some((state, Some(pane)));
            }
            Ok((state, None)) => return Some((state, None)),
            Ok((_, Some(_))) => {
                log::warn!(
                    "shared pane {mux_pane_id} of session {session_id} did not attach as a stream"
                );
            }
            Err(error) => log::debug!(
                "could not attach shared pane {mux_pane_id} of session {session_id}: {error:#}"
            ),
        }
    }
    log::warn!("gave up attaching shared pane {mux_pane_id} of session {session_id}");
    None
}

#[cfg(test)]
#[path = "../tests/background_session_ui/collaboration.rs"]
mod tests;
