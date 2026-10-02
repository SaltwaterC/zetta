//! Session transitions that wait on pane readers or the daemon, run off the
//! window's thread.
//!
//! Detaching a tab and sharing one both end in daemon requests, and detaching
//! first has to wait for every pane's pty loop to end and serialize its grid.
//! Done inline, one slow pane or one slow daemon stalled the whole window — and
//! every other window of the process, which shares the GPUI thread.
//!
//! Each transition runs in three phases:
//!
//! 1. **Preparation**, on the window's thread: validate, build the publication
//!    from the live tab, and retire each pane's reader without waiting for it
//!    ([`terminal::Terminal::retire_pty_loop`]). Nothing here blocks.
//! 2. **Work**, on a thread of its own: finish the retired readers — each pane
//!    on its own thread, since one pane's join or snapshot has nothing to do
//!    with another's — serialize the grids once nothing can still write to
//!    them, and make the requests in the order the daemon needs them. A thread
//!    rather than the background executor, because a drain can take seconds
//!    and a slow daemon longer, and neither should hold an executor worker.
//! 3. **Commit**, back on the window's thread, and only for the transition that
//!    is still current: the [`SessionHandovers`] entry it was started under
//!    must still be there with the same generation.
//!
//! A tab with a transition in flight refuses another lifecycle action rather
//! than racing it. Closing such a tab, and closing a window, settle the
//! transition synchronously first: a closing window may be followed by the
//! process quitting, and a detach that has not reached the daemon by then is a
//! session lost.

use super::*;

use std::sync::{Arc, mpsc};

use futures::channel::oneshot;
use terminal::{GridSnapshotSource, RetiredReader};
use zmux::client::{SharedSessionEvent, SharedSessionReports};
use zmux::messages::SharedSessionState;

/// Shown when a lifecycle action reaches a tab whose last one has not finished.
const HANDOVER_IN_FLIGHT: &str =
    "This tab's session is still being handed over to the multiplexer; try again in a moment.";

/// The daemon requests a handover makes, as a trait so a test can stand in a
/// slow or failing daemon. Implemented by [`zmux::client::Client`], each call of
/// which opens its own connection, so independent panes can be sent at once.
pub(crate) trait HandoverDaemon: Send + Sync {
    fn close_pane(&self, session_id: u64, mux_pane_id: u64) -> anyhow::Result<()>;
    fn send_snapshot(&self, session_id: u64, checkpoint: PaneCheckpointData) -> anyhow::Result<()>;
    fn share(
        &self,
        session_id: u64,
        publication: SessionPublication,
        authentication: Option<&SessionAuthentication>,
        offered: bool,
    ) -> anyhow::Result<()>;
    fn detach(
        &self,
        session_id: u64,
        publication: SessionPublication,
        authentication: Option<&SessionAuthentication>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> anyhow::Result<()>;
    fn shared_snapshot(&self, session_id: u64) -> anyhow::Result<SharedSessionState>;
}

/// What the daemon lists a session as, and the state a window rebuilds it from.
pub(crate) struct SessionPublication {
    pub(crate) summary: BackgroundSessionSummary,
    pub(crate) state: serde_json::Value,
}

/// One pane's screen, serialized, at the size it was drawn.
pub(crate) struct PaneCheckpointData {
    pub(crate) mux_pane_id: u64,
    pub(crate) snapshot: Vec<u8>,
    pub(crate) columns: u16,
    pub(crate) lines: u16,
}

impl HandoverDaemon for zmux::client::Client {
    fn close_pane(&self, session_id: u64, mux_pane_id: u64) -> anyhow::Result<()> {
        zmux::client::Client::close_pane(self, session_id, mux_pane_id)
    }

    fn send_snapshot(&self, session_id: u64, checkpoint: PaneCheckpointData) -> anyhow::Result<()> {
        zmux::client::Client::send_snapshot(
            self,
            session_id,
            checkpoint.mux_pane_id,
            checkpoint.snapshot,
            checkpoint.columns,
            checkpoint.lines,
        )
    }

    fn share(
        &self,
        session_id: u64,
        publication: SessionPublication,
        authentication: Option<&SessionAuthentication>,
        offered: bool,
    ) -> anyhow::Result<()> {
        zmux::client::Client::share(
            self,
            session_id,
            publication.summary,
            publication.state,
            authentication,
            offered,
        )
    }

    fn detach(
        &self,
        session_id: u64,
        publication: SessionPublication,
        authentication: Option<&SessionAuthentication>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> anyhow::Result<()> {
        zmux::client::Client::detach(
            self,
            session_id,
            publication.summary,
            publication.state,
            authentication,
            snapshots,
        )
    }

    fn shared_snapshot(&self, session_id: u64) -> anyhow::Result<SharedSessionState> {
        zmux::client::Client::shared_snapshot(self, session_id)
    }
}

/// One pane's part in a detach: the reader it retired and, when the daemon
/// keeps screens, the grid to snapshot once that reader has ended.
pub(crate) struct PaneRetirement {
    pub(crate) mux_pane_id: u64,
    pub(crate) reader: RetiredReader,
    pub(crate) snapshot: Option<GridSnapshotSource>,
}

impl PaneRetirement {
    /// The barrier, then the snapshot: a grid serialized before its reader
    /// ended could miss bytes the daemon will not have either.
    fn finish(self) -> anyhow::Result<Option<(u64, Vec<u8>)>> {
        self.reader
            .finish()
            .with_context(|| format!("stopping pane {} before detach", self.mux_pane_id))?;
        Ok(self
            .snapshot
            .map(|source| (self.mux_pane_id, source.ansi_snapshot(SNAPSHOT_LINES))))
    }
}

/// A stacked task terminal released with its tab. See
/// [`Zetta::hand_session_to_multiplexer`] for why these are closed rather than
/// published.
pub(crate) struct StackedRelease {
    pub(crate) entry_id: u64,
    pub(crate) mux_pane_id: u64,
    pub(crate) reader: RetiredReader,
}

/// Everything the background phase of a detach needs, owned.
pub(crate) struct DetachWork {
    pub(crate) daemon: Arc<dyn HandoverDaemon>,
    pub(crate) session_id: u64,
    pub(crate) stacked: Vec<StackedRelease>,
    pub(crate) panes: Vec<PaneRetirement>,
    pub(crate) publication: SessionPublication,
    pub(crate) authentication: Option<SessionAuthentication>,
}

/// What a detach did. `released_stacked` is reported whatever the result, so a
/// failed detach can restore the mappings of the stacked panes the daemon still
/// holds.
pub(crate) struct DetachOutcome {
    pub(crate) released_stacked: Vec<u64>,
    pub(crate) result: anyhow::Result<()>,
}

impl DetachWork {
    /// Blocking: joins readers and talks to the daemon.
    pub(crate) fn run(self) -> DetachOutcome {
        let Self {
            daemon,
            session_id,
            stacked,
            panes,
            publication,
            authentication,
        } = self;
        let mut released_stacked = Vec::new();
        let snapshots = std::thread::scope(|scope| {
            // The visible panes finish while the stacked ones are released;
            // neither waits on the other until the detach itself.
            let finishing = panes
                .into_iter()
                .map(|pane| scope.spawn(move || pane.finish()))
                .collect::<Vec<_>>();
            let released = release_stacked(&*daemon, session_id, stacked, &mut released_stacked);
            let snapshots = join_all(finishing);
            released.and(snapshots)
        });
        let result = snapshots.and_then(|snapshots| {
            daemon.detach(
                session_id,
                publication,
                authentication.as_ref(),
                snapshots.into_iter().flatten().collect(),
            )
        });
        DetachOutcome {
            released_stacked,
            result,
        }
    }
}

/// Stops and closes each stacked pane, in order, until one fails.
fn release_stacked(
    daemon: &dyn HandoverDaemon,
    session_id: u64,
    stacked: Vec<StackedRelease>,
    released: &mut Vec<u64>,
) -> anyhow::Result<()> {
    for StackedRelease {
        entry_id,
        mux_pane_id,
        reader,
    } in stacked
    {
        reader
            .finish()
            .context("stopping a stacked terminal before detach")?;
        daemon
            .close_pane(session_id, mux_pane_id)
            .with_context(|| format!("closing stacked daemon pane {mux_pane_id}"))?;
        released.push(entry_id);
    }
    Ok(())
}

/// Every thread's result, in the order they were spawned; the first error wins.
fn join_all<T>(
    threads: Vec<std::thread::ScopedJoinHandle<'_, anyhow::Result<T>>>,
) -> anyhow::Result<Vec<T>> {
    threads
        .into_iter()
        .map(|thread| {
            thread
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("a pane's handover panicked")))
        })
        .collect()
}

/// A live pane's screen, checkpointed with the daemon before a session is
/// offered. Its reader keeps running: this is a picture, not a handover.
pub(crate) struct PaneCheckpoint {
    pub(crate) mux_pane_id: u64,
    pub(crate) source: GridSnapshotSource,
    pub(crate) columns: u16,
    pub(crate) lines: u16,
}

/// Everything the background phase of sharing or unsharing needs, owned.
pub(crate) struct OfferWork {
    pub(crate) daemon: Arc<dyn HandoverDaemon>,
    pub(crate) reports: Arc<SharedSessionReports>,
    pub(crate) session_id: u64,
    pub(crate) offered: bool,
    pub(crate) checkpoints: Vec<PaneCheckpoint>,
    pub(crate) publication: SessionPublication,
    pub(crate) authentication: Option<SessionAuthentication>,
}

/// What offering a session did. `binding` is present only for an offer the
/// daemon accepted, and failing it does not undo the offer: the tab is shared,
/// it just could not start following the session's canonical state.
pub(crate) struct OfferOutcome {
    pub(crate) published: anyhow::Result<()>,
    pub(crate) binding: Option<anyhow::Result<SharedBinding>>,
}

/// The subscription and first snapshot a shared tab is bound with. Subscribed
/// before the snapshot is fetched, so an operation committed in between is
/// queued behind the snapshot instead of being lost.
pub(crate) struct SharedBinding {
    pub(crate) receiver: async_channel::Receiver<SharedSessionEvent>,
    pub(crate) state: SharedSessionState,
}

impl OfferWork {
    /// Blocking: serializes grids and talks to the daemon.
    pub(crate) fn run(self) -> OfferOutcome {
        let Self {
            daemon,
            reports,
            session_id,
            offered,
            checkpoints,
            publication,
            authentication,
        } = self;
        let published = checkpoint_panes(&*daemon, session_id, checkpoints)
            .and_then(|()| daemon.share(session_id, publication, authentication.as_ref(), offered));
        let binding = (published.is_ok() && offered)
            .then(|| fetch_shared_binding(&*daemon, &reports, session_id));
        OfferOutcome { published, binding }
    }
}

/// Snapshots and sends each pane's screen, all at once: the daemon takes each
/// on its own connection, and only the offer that follows needs them all.
fn checkpoint_panes(
    daemon: &dyn HandoverDaemon,
    session_id: u64,
    checkpoints: Vec<PaneCheckpoint>,
) -> anyhow::Result<()> {
    std::thread::scope(|scope| {
        let sending = checkpoints
            .into_iter()
            .map(|checkpoint| {
                scope.spawn(move || {
                    let mux_pane_id = checkpoint.mux_pane_id;
                    daemon
                        .send_snapshot(
                            session_id,
                            PaneCheckpointData {
                                mux_pane_id,
                                snapshot: checkpoint.source.ansi_snapshot(SNAPSHOT_LINES),
                                columns: checkpoint.columns,
                                lines: checkpoint.lines,
                            },
                        )
                        .with_context(|| {
                            format!(
                                "checkpointing pane {mux_pane_id} before sharing session {session_id}"
                            )
                        })
                })
            })
            .collect::<Vec<_>>();
        join_all(sending).map(|_| ())
    })
}

/// Subscribes to a shared session and fetches its canonical state. Blocking.
pub(crate) fn fetch_shared_binding(
    daemon: &dyn HandoverDaemon,
    reports: &SharedSessionReports,
    session_id: u64,
) -> anyhow::Result<SharedBinding> {
    reports.forget(session_id);
    let receiver = reports.register(session_id);
    match daemon.shared_snapshot(session_id) {
        Ok(state) => Ok(SharedBinding { receiver, state }),
        Err(error) => {
            reports.forget(session_id);
            Err(error)
        }
    }
}

/// Runs `work` on a thread of its own. The outcome is read from the first
/// receiver — by the commit once the second says it is there, or by a window
/// that is closing and has to wait for it.
fn spawn_handover_worker<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> (mpsc::Receiver<T>, oneshot::Receiver<()>) {
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let (done_tx, done_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("zetta-session-handover".to_owned())
        .spawn(move || {
            outcome_tx.send(work()).ok();
            done_tx.send(()).ok();
        })
        .expect("spawning a session handover worker");
    (outcome_rx, done_rx)
}

enum PendingHandover {
    Detach {
        tab: Box<Tab>,
        origin: HandoverOrigin,
        /// Stacked panes whose mappings preparation forgot so the publication
        /// would not name them, restored for those a refusal leaves open.
        stacked: Vec<(u64, u64)>,
        outcome: mpsc::Receiver<DetachOutcome>,
    },
    Offer {
        offered: bool,
        previous_shared: bool,
        outcome: mpsc::Receiver<OfferOutcome>,
    },
}

/// The transitions a window has in flight, one per tab at most.
#[derive(Default)]
pub(crate) struct SessionHandovers {
    generation: u64,
    pending: HashMap<u64, (u64, PendingHandover)>,
}

impl SessionHandovers {
    fn begin(&mut self, tab_id: u64, pending: PendingHandover) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        let previous = self.pending.insert(tab_id, (self.generation, pending));
        debug_assert!(
            previous.is_none(),
            "tab {tab_id} already had a transition in flight"
        );
        self.generation
    }

    pub(crate) fn in_flight(&self, tab_id: u64) -> bool {
        self.pending.contains_key(&tab_id)
    }

    /// The transition for `tab_id`, if it is the one `generation` names, or any
    /// transition when `generation` is `None`.
    fn take(&mut self, tab_id: u64, generation: Option<u64>) -> Option<PendingHandover> {
        let (current, _) = self.pending.get(&tab_id)?;
        if generation.is_some_and(|generation| generation != *current) {
            return None;
        }
        self.pending.remove(&tab_id).map(|(_, pending)| pending)
    }

    fn tab_ids(&self) -> Vec<u64> {
        self.pending.keys().copied().collect()
    }
}

impl Zetta {
    pub(crate) fn session_handover_in_flight(&self, tab_id: u64) -> bool {
        self.session_handovers.in_flight(tab_id)
    }

    /// Refuses a lifecycle action on a tab whose last one has not finished.
    /// Returns whether it refused.
    pub(crate) fn refuse_during_session_handover(
        &mut self,
        tab_id: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let busy = self.session_handover_in_flight(tab_id);
        if busy {
            self.show_notice(HANDOVER_IN_FLIGHT, cx);
        }
        busy
    }

    /// Runs a prepared detach and commits it once the daemon answers.
    ///
    /// The tab is out of `self.tabs` from here on and held by the pending
    /// entry, so nothing else in the window can reach it until the commit
    /// either drops it or puts it back.
    pub(super) fn await_multiplexer_handover(
        &mut self,
        tab: Tab,
        origin: HandoverOrigin,
        prepared: PreparedDetach,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let PreparedDetach { work, stacked } = prepared;
        let tab_id = tab.id;
        let (outcome, done) = spawn_handover_worker(move || work.run());
        let generation = self.session_handovers.begin(
            tab_id,
            PendingHandover::Detach {
                tab: Box::new(tab),
                origin,
                stacked,
                outcome,
            },
        );
        self.commit_session_handover_when_done(tab_id, generation, done, window, cx);
    }

    /// Runs a prepared offer or withdrawal and commits it once the daemon
    /// answers. The tab stays on screen and keeps working throughout.
    pub(super) fn await_session_offer(
        &mut self,
        tab_id: u64,
        offered: bool,
        previous_shared: bool,
        work: OfferWork,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (outcome, done) = spawn_handover_worker(move || work.run());
        let generation = self.session_handovers.begin(
            tab_id,
            PendingHandover::Offer {
                offered,
                previous_shared,
                outcome,
            },
        );
        self.commit_session_handover_when_done(tab_id, generation, done, Some(window), cx);
    }

    fn commit_session_handover_when_done(
        &mut self,
        tab_id: u64,
        generation: u64,
        done: oneshot::Receiver<()>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        match window {
            Some(window) => cx
                .spawn_in(window, async move |this, cx| {
                    done.await.ok();
                    this.update_in(cx, |this, window, cx| {
                        this.settle_session_handover(tab_id, Some(generation), Some(window), cx);
                    })
                    .ok();
                })
                .detach(),
            None => cx
                .spawn(async move |this, cx| {
                    done.await.ok();
                    this.update(cx, |this, cx| {
                        this.settle_session_handover(tab_id, Some(generation), None, cx);
                    })
                    .ok();
                })
                .detach(),
        }
    }

    /// Commits `tab_id`'s transition, waiting for its worker if it has not
    /// finished. Called by the commit itself, when the worker already has, and
    /// by anything that has to see the transition finished before it acts —
    /// closing the tab, or the window.
    ///
    /// Without a window a successful offer is not bound to the session's
    /// canonical state: only a window that is going away commits that way.
    pub(crate) fn settle_session_handover(
        &mut self,
        tab_id: u64,
        generation: Option<u64>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.session_handovers.take(tab_id, generation) else {
            return;
        };
        match pending {
            PendingHandover::Detach {
                tab,
                origin,
                stacked,
                outcome,
            } => {
                let outcome = outcome.recv().unwrap_or_else(|_| DetachOutcome {
                    released_stacked: Vec::new(),
                    result: Err(anyhow::anyhow!(
                        "the handover worker ended without an answer"
                    )),
                });
                self.finish_multiplexer_handover(*tab, origin, &stacked, outcome, window, cx);
            }
            PendingHandover::Offer {
                offered,
                previous_shared,
                outcome,
            } => {
                let outcome = outcome.recv().unwrap_or_else(|_| OfferOutcome {
                    published: Err(anyhow::anyhow!(
                        "the handover worker ended without an answer"
                    )),
                    binding: None,
                });
                self.finish_session_offer(tab_id, offered, previous_shared, outcome, window, cx);
            }
        }
    }

    /// Settles every transition this window has in flight, waiting for each.
    /// For a window that is closing, which cannot leave them to a commit that
    /// may never run.
    pub(crate) fn settle_session_handovers(&mut self, cx: &mut Context<Self>) {
        for tab_id in self.session_handovers.tab_ids() {
            self.settle_session_handover(tab_id, None, None, cx);
        }
    }

    /// The commit of a detach: forget a tab the daemon now holds, or put back
    /// one it refused.
    fn finish_multiplexer_handover(
        &mut self,
        tab: Tab,
        origin: HandoverOrigin,
        stacked: &[(u64, u64)],
        outcome: DetachOutcome,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let DetachOutcome {
            released_stacked,
            result,
        } = outcome;
        for (entry_id, mux_pane_id) in stacked {
            if !released_stacked.contains(entry_id) {
                self.mux_panes.record(*entry_id, *mux_pane_id);
            }
        }
        match result {
            Ok(()) => self.forget_handed_over_tab(&tab),
            Err(error) => self.recover_refused_handover(tab, origin, &error, window, cx),
        }
        self.finish_background_session_change(cx);
        cx.notify();
    }

    /// Everything that pointed at a tab the multiplexer now holds. Dropping
    /// the tab then drops the PTY descriptors this process was holding, and
    /// the multiplexer resumes reading them.
    fn forget_handed_over_tab(&mut self, tab: &Tab) {
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
        // Retire the foreground tab's event routes before forgetting its pane
        // mappings. Registering the same daemon pane after a reconnect replaces
        // these senders; leaving them behind made their old watchers wake on
        // channel closure and mistake that lifecycle event for a real revoke or
        // grant.
        if let Some(runtime) = self
            .mux_panes
            .runtime_for_tab(tab.id)
            .or_else(|| self.mux.clone())
        {
            for pane in &tab.panes {
                let Some(mux_pane_id) = self.mux_panes.mux_pane_id(pane.id) else {
                    continue;
                };
                runtime.reporters().forget(mux_pane_id);
                runtime.revoke_reporters().forget(mux_pane_id);
                runtime.grant_reporters().forget(mux_pane_id);
            }
        }
        self.mux_panes.forget_tab(tab.id);
        for pane in &tab.panes {
            self.mux_panes.forget_pane(pane.id);
        }
    }

    /// A normal launch must not silently turn a failed daemon handoff into an
    /// in-process background session. Put the tab back where it was so the
    /// user can retry after fixing the daemon. Its readers were stopped for
    /// the handover, so what was typed at it since is dropped rather than held
    /// for a backend that is not coming.
    fn recover_refused_handover(
        &mut self,
        tab: Tab,
        origin: HandoverOrigin,
        error: &anyhow::Error,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        for terminal in tab.panes.iter().flat_map(|pane| {
            pane.terminal.iter().chain(
                pane.stack
                    .entries
                    .iter()
                    .filter_map(|entry| entry.terminal.as_ref()),
            )
        }) {
            terminal.update(cx, |terminal, _| terminal.discard_held_input());
        }
        let HandoverOrigin::Tab { index, shared } = origin else {
            log::warn!(
                "could not hand tab {} to the multiplexer as its window closed: {error:#}",
                tab.id
            );
            return;
        };
        self.show_error_notice(
            format!(
                "Could not hand the session to the multiplexer; it remains in this window: {error:#}"
            ),
            cx,
        );
        let tab_id = tab.id;
        let insertion_index = index.min(self.tabs.len());
        self.tabs.insert(insertion_index, tab);
        self.active_tab = insertion_index;
        let Some(window) = window else {
            return;
        };
        if shared
            && let Some(runtime) = self
                .mux_panes
                .runtime_for_tab(tab_id)
                .or_else(|| self.mux.clone())
        {
            self.rebind_shared_session(tab_id, runtime, window, cx);
        }
        self.focus_active(window, cx);
    }

    /// The commit of an offer or a withdrawal.
    fn finish_session_offer(
        &mut self,
        tab_id: u64,
        offered: bool,
        previous_shared: bool,
        outcome: OfferOutcome,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let OfferOutcome { published, binding } = outcome;
        let runtime = self
            .mux_panes
            .runtime_for_tab(tab_id)
            .or_else(|| self.mux.clone());
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            // The tab left the window while the request was in flight. A
            // subscription nobody will watch must not keep collecting events.
            if let (Some(Ok(_)), Some(runtime), Some(session_id)) =
                (&binding, &runtime, self.mux_panes.session_id(tab_id))
            {
                runtime.shared_reports().forget(session_id);
            }
            return;
        };
        if let Err(error) = published {
            self.tabs[index].shared = previous_shared;
            // A refused *unshare* is guidance, not a failure: the multiplexer
            // only scopes a session back to one window while one window has
            // it, and it says which. The tab stays shared, which is what the
            // menu then shows.
            if offered {
                self.show_error_notice(format!("Could not share this tab: {error:#}"), cx);
            } else {
                self.show_error_notice(format!("{error:#}"), cx);
            }
            cx.notify();
            return;
        }
        let setup = match (binding, runtime, window) {
            (None, ..) => {
                self.forget_shared_session(tab_id);
                Ok(())
            }
            (Some(binding), Some(runtime), Some(window)) => binding.and_then(|binding| {
                self.install_shared_binding(tab_id, runtime, binding, window, cx)
            }),
            (Some(binding), runtime, None) => {
                // Only a closing window commits without one, and it is about
                // to leave the session anyway.
                if let (Ok(_), Some(runtime), Some(session_id)) =
                    (binding, runtime, self.mux_panes.session_id(tab_id))
                {
                    runtime.shared_reports().forget(session_id);
                }
                Ok(())
            }
            (Some(_), None, Some(_)) => {
                Err(anyhow::anyhow!("shared tab has no multiplexer runtime"))
            }
        };
        let setup_succeeded = match setup {
            Ok(()) => true,
            Err(error) => {
                self.show_error_notice(
                    format!("Could not initialize shared tab collaboration: {error:#}"),
                    cx,
                );
                false
            }
        };
        self.finish_background_session_change(cx);
        // Nothing about the tab changes when it is shared, so without saying
        // so the toggle has no visible effect at all beyond a checkmark in a
        // menu that has already closed.
        if setup_succeeded {
            self.show_notice(
                if offered {
                    "This tab can now be joined from another Zetta window."
                } else {
                    "This tab is no longer shared, and belongs to this window again."
                },
                cx,
            );
        }
        cx.notify();
    }

    /// Binds a tab to its shared session again, off the window's thread: the
    /// counterpart of an offer's binding for a tab that was already shared, and
    /// whose binding a refused detach took down.
    pub(super) fn rebind_shared_session(
        &mut self,
        tab_id: u64,
        runtime: MuxRuntime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        let daemon: Arc<dyn HandoverDaemon> = runtime.client().clone();
        let reports = runtime.shared_reports().clone();
        cx.spawn_in(window, async move |this, cx| {
            let binding = cx
                .background_spawn(
                    async move { fetch_shared_binding(&*daemon, &reports, session_id) },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                // Still the tab it was fetched for, and nothing has bound or
                // re-offered it in the meantime.
                let current = this.tabs.iter().any(|tab| tab.id == tab_id)
                    && !this.session_handover_in_flight(tab_id)
                    && !this.has_shared_tab_binding(tab_id)
                    && this.mux_panes.session_id(tab_id) == Some(session_id);
                if !current {
                    if binding.is_ok() {
                        runtime.shared_reports().forget(session_id);
                    }
                    return;
                }
                if let Err(error) = binding.and_then(|binding| {
                    this.install_shared_binding(tab_id, runtime, binding, window, cx)
                }) {
                    this.show_error_notice(
                        format!(
                            "Could not restore shared tab collaboration after the failed handoff: \
                             {error:#}"
                        ),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
#[path = "../tests/background_session_ui/handover.rs"]
mod tests;
