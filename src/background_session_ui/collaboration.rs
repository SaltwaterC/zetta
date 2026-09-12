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

#[derive(Default)]
pub(crate) struct SharedSessionCoordinator {
    sessions: HashMap<u64, SharedSessionBinding>,
    next_watch_id: u64,
}

struct SharedSessionBinding {
    tab_id: u64,
    state: SharedSessionState,
    /// The opaque tab blob this window last wrote into its tab.
    ///
    /// Only `ReplaceTab` and `SetTabState` replace the daemon's copy, so every
    /// geometry revision carries the blob from whichever publication set it
    /// last. Applying that again would rewrite the icon, the titles, the pane
    /// names and the overlays from it — undoing a local change that has not
    /// been published yet. Compared rather than the revision because both sides
    /// are the same daemon field in the publisher's own id space.
    applied_state: serde_json::Value,
    /// The blob this window last published, so a publication driven by geometry
    /// churn does not send identical bytes again.
    last_published_state: Option<serde_json::Value>,
    mux_to_local: HashMap<u64, u64>,
    local_to_mux: HashMap<u64, u64>,
    /// Canonical mutations waiting their turn. Every change this window asks
    /// the daemon for goes through here in order — a state publication and a
    /// pane close racing on the same revision is how a close came to be
    /// rejected and then silently forgotten.
    queue: VecDeque<SharedOperation>,
    in_flight: bool,
    /// Whether a debounce timer is already running for a state publication, so
    /// a gesture that notifies dozens of times starts one timer, not dozens.
    publication_scheduled: bool,
    watch_id: Option<u64>,
    /// The stable ids this window is in the middle of attaching. Snapshots are
    /// applied from several places at once, so without this the same pane is
    /// attached twice when two of them see it missing before either finishes.
    attaching: HashSet<u64>,
    /// How to reach the daemon for this session. Kept here because a snapshot
    /// is applied from callers that have no connection of their own — a
    /// committed spawn, a conflict response — and every one of them may have to
    /// attach a pane the snapshot introduced.
    connection: Option<SharedWatchConnection>,
}

#[derive(Clone)]
struct SharedWatchConnection {
    client: Arc<zmux::client::Client>,
    secret: Option<zmux::auth::SessionSecret>,
}

/// One canonical mutation this window has asked the daemon for.
///
/// The daemon serializes operations against its own tree, but the *window* also
/// has to: a publication built from the local tab and a close of a pane in it
/// describe two different pane sets, and sending both at once means whichever
/// arrives second is rejected for disagreeing with a tree the first just
/// changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SharedOperation {
    /// Publish this window's view of the tab: the geometry the daemon does not
    /// already own, and then the durable state it stores without reading —
    /// labels, icons, titles, themes and overlays.
    ///
    /// The two halves are separate canonical operations, so one publication can
    /// need two requests. It re-queues itself between them, counting attempts
    /// so geometry that never converges cannot loop.
    PublishState { attempts: u32 },
    /// Close a pane for every viewer.
    ClosePane {
        local_pane_id: u64,
        mux_pane_id: u64,
        attempts: u32,
    },
}

impl SharedOperation {
    /// Whether two queued operations would do the same work. Compared instead
    /// of equality because a retried close differs only in its attempt count,
    /// and re-queuing it must not leave the original behind.
    fn is_same_work(self, other: Self) -> bool {
        match (self, other) {
            (Self::PublishState { .. }, Self::PublishState { .. }) => true,
            (
                Self::ClosePane { local_pane_id, .. },
                Self::ClosePane {
                    local_pane_id: other,
                    ..
                },
            ) => local_pane_id == other,
            _ => false,
        }
    }
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
                // Whoever binds has the tab this blob describes: a joiner built
                // it from exactly these bytes, and a sharer published them.
                applied_state: state.state.clone(),
                last_published_state: None,
                state,
                mux_to_local,
                local_to_mux,
                queue: VecDeque::new(),
                in_flight: false,
                publication_scheduled: false,
                watch_id: None,
                attaching: HashSet::new(),
                connection: None,
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

    /// Whether this blob says something the daemon has not been told yet.
    ///
    /// Compared against what *this window* last published rather than against
    /// the canonical copy: the two windows write their own pane ids into the
    /// blob, so the canonical copy differs from ours whenever the other window
    /// published last, even when both describe the same tab.
    pub(crate) fn durable_state_is_unpublished(
        &self,
        session_id: u64,
        state: &serde_json::Value,
    ) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| session.last_published_state.as_ref() != Some(state))
    }

    pub(crate) fn record_published_state(&mut self, session_id: u64, state: serde_json::Value) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.last_published_state = Some(state);
        }
    }

    /// Whether this snapshot has already been superseded. Checked *before*
    /// anything is removed for it: a snapshot that will not be applied must not
    /// take panes out of the tab, because nothing would then put the layout
    /// back together.
    pub(crate) fn snapshot_is_stale(&self, session_id: u64, state: &SharedSessionState) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| state.revision < session.state.revision)
    }

    /// Claims a pane for attachment, or reports that another task already has
    /// it. Released by [`Self::end_attach`] whether the attachment succeeded or
    /// not, so a failure is retried by the next snapshot rather than wedged.
    pub(crate) fn begin_attach(&mut self, session_id: u64, mux_pane_id: u64) -> bool {
        self.sessions
            .get_mut(&session_id)
            .is_some_and(|session| session.attaching.insert(mux_pane_id))
    }

    pub(crate) fn end_attach(&mut self, session_id: u64, mux_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.attaching.remove(&mux_pane_id);
        }
    }

    fn set_connection(&mut self, session_id: u64, connection: SharedWatchConnection) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.connection = Some(connection);
        }
    }

    fn connection(&self, session_id: u64) -> Option<SharedWatchConnection> {
        self.sessions
            .get(&session_id)
            .and_then(|session| session.connection.clone())
    }

    /// Claims the debounce for a state publication. `false` means one is
    /// already scheduled and this caller has nothing to do.
    pub(crate) fn schedule_publication(&mut self, session_id: u64) -> bool {
        self.sessions
            .get_mut(&session_id)
            .is_some_and(|session| !std::mem::replace(&mut session.publication_scheduled, true))
    }

    pub(crate) fn clear_publication_schedule(&mut self, session_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.publication_scheduled = false;
        }
    }

    /// Queues a canonical mutation, collapsing it into an equivalent one
    /// already waiting. A second publication would send the same tab state
    /// twice, and a second close of the same pane would be refused by the
    /// daemon for naming a pane it no longer holds.
    pub(crate) fn enqueue(&mut self, session_id: u64, operation: SharedOperation) {
        if let Some(session) = self.sessions.get_mut(&session_id)
            && !session
                .queue
                .iter()
                .any(|queued| queued.is_same_work(operation))
        {
            session.queue.push_back(operation);
        }
    }

    /// Takes the next operation to run, or `None` while one is still in flight.
    /// Taking one marks the session busy until [`Self::finish_operation`].
    pub(crate) fn take_next_operation(&mut self, session_id: u64) -> Option<SharedOperation> {
        let session = self.sessions.get_mut(&session_id)?;
        if session.in_flight {
            return None;
        }
        let operation = session.queue.pop_front()?;
        session.in_flight = true;
        Some(operation)
    }

    pub(crate) fn finish_operation(&mut self, session_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id) {
            session.in_flight = false;
        }
    }

    pub(crate) fn may_report_size(&self, session_id: u64) -> bool {
        self.sessions
            .get(&session_id)
            .is_some_and(|session| !session.in_flight)
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
            .filter(|mux_id| {
                !session.mux_to_local.contains_key(mux_id) && !session.attaching.contains(mux_id)
            })
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

    /// Forgets a pane this window no longer shows.
    ///
    /// Called wherever a pane leaves a tab, including the ordinary local close.
    /// A mapping left behind resolves a canonical snapshot that still names the
    /// pane onto a local id with no `TerminalPane`, which is how a closed pane
    /// came back into the layout as a region nothing draws.
    pub(crate) fn remove_local_pane(&mut self, session_id: u64, local_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(&session_id)
            && let Some(mux_pane_id) = session.local_to_mux.remove(&local_pane_id)
        {
            session.mux_to_local.remove(&mux_pane_id);
        }
    }

    /// Accepts a complete canonical snapshot and maps its opaque tab state
    /// into this window's ids. A stale event is harmless; a caller that needs
    /// the full state after a gap asks `zmux` directly and calls this again.
    ///
    /// Geometry is installed from every snapshot, the durable tab state only
    /// from one that carries a blob this window has not already applied. Only
    /// `ReplaceTab` and `SetTabState` replace the daemon's copy of that blob, so
    /// a geometry revision hands back whichever one was published last —
    /// applying it again would undo an icon, a title or a rename that is still
    /// waiting for its own publication.
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
        // Geometry is canonical on every snapshot; the durable half is only
        // rewritten when the daemon's copy of it actually changed. Every
        // geometry operation leaves that copy alone and hands it back
        // unchanged, so applying it again would undo a local icon, title or
        // rename that has not had its own publication yet.
        let tab_state = if state.state == session.applied_state {
            None
        } else {
            let mut tab_state: TabState = match serde_json::from_value(state.state.clone()) {
                Ok(state) => state,
                Err(_error) if state.state.is_null() => {
                    // Sessions published by an older daemon may not have carried
                    // a durable tab blob. Preserve the local model while the
                    // summary supplies the authoritative shared layout below.
                    TabState::from_tab(tab, &session.local_to_mux)
                }
                Err(error) => return Err(error).context("reading the shared tab state"),
            };
            remap_tab_state(&mut tab_state, &state.presentation, &session.mux_to_local)?;
            Some(tab_state)
        };
        // Mapped and checked before anything is written. A layout naming a pane
        // this tab does not hold renders as a region nothing draws and never
        // gives its space back, so it is refused rather than installed;
        // `apply_tab_state` derives its own layout from the same presentation,
        // so one check covers both writes.
        let mut layout = map_background_layout(&state.presentation.layout, &session.mux_to_local)?;
        // Panes this tab holds that the session has not accepted yet — the
        // drafts of splits still in flight — go back where they were before the
        // session's own layout is installed over the top of them.
        for pane_id in tab
            .panes
            .iter()
            .map(|pane| pane.id)
            .filter(|pane_id| !layout.contains_pane(*pane_id))
            .collect::<Vec<_>>()
        {
            if !reinsert_unplaced_pane(&tab.layout, &mut layout, pane_id) {
                log::debug!(
                    "pane {pane_id} of tab {} has no place in the session's layout and nothing \
                     to sit beside",
                    tab.id
                );
            }
        }
        ensure_layout_covers_tab(tab, &layout)?;
        if let Some(tab_state) = tab_state {
            apply_tab_state(tab, tab_state);
            session.applied_state = state.state.clone();
        }
        apply_canonical_presentation(tab, &state, layout, &session.mux_to_local)?;
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
        self.pane_output_error = Some(reason);
        cx.notify();
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
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        let Some(state) = self.shared_collaboration.state(session_id).cloned() else {
            return;
        };
        let stale = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| panes_the_session_lost(tab, self.mux_panes.ids(), &state))
            .unwrap_or_default();
        if !stale.is_empty() {
            log::warn!(
                "shared session {session_id} no longer holds the pane(s) behind local pane(s) \
                 {stale:?}; dropping this window's view of them"
            );
        }
        for pane_id in stale {
            self.remove_local_shared_pane(session_id, pane_id, cx);
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
    pub(crate) fn shared_geometry_is_current(&self, session_id: u64, named: &[u64]) -> bool {
        let Some(state) = self.shared_collaboration.state(session_id) else {
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
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        self.shared_collaboration
            .remove_local_pane(session_id, pane_id);
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
        let Some(session_id) = self.mux_panes.session_id(tab_id) else {
            return;
        };
        if !self.shared_collaboration.schedule_publication(session_id) {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            executor.timer(SHARED_PUBLICATION_DEBOUNCE).await;
            this.update(cx, |this, cx| {
                this.shared_collaboration
                    .clear_publication_schedule(session_id);
                this.shared_collaboration
                    .enqueue(session_id, SharedOperation::PublishState { attempts: 0 });
                this.pump_shared_operations(tab_id, session_id, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Runs the session's next queued mutation, if one is not already running.
    ///
    /// Each operation is built from the tab and rebased on the canonical
    /// revision at the moment it is sent, never at the moment it was queued, so
    /// waiting behind another operation cannot make it stale.
    pub(crate) fn pump_shared_operations(
        &mut self,
        tab_id: u64,
        session_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(operation) = self.shared_collaboration.take_next_operation(session_id) else {
            return;
        };
        match operation {
            SharedOperation::PublishState { attempts } => {
                self.run_shared_publication(tab_id, session_id, attempts, cx);
            }
            SharedOperation::ClosePane {
                local_pane_id,
                mux_pane_id,
                attempts,
            } => self.run_shared_pane_close(
                tab_id,
                session_id,
                local_pane_id,
                mux_pane_id,
                attempts,
                cx,
            ),
        }
    }

    fn finish_shared_operation(&mut self, tab_id: u64, session_id: u64, cx: &mut Context<Self>) {
        self.shared_collaboration.finish_operation(session_id);
        self.report_all_shared_pane_sizes(tab_id, cx);
        self.pump_shared_operations(tab_id, session_id, cx);
    }

    /// Converges the tab on what the daemon answered a publication with, and
    /// records the blob as published when it was the daemon that accepted it.
    fn install_shared_publication_response(
        &mut self,
        session_id: u64,
        published_state: Option<serde_json::Value>,
        state: SharedSessionState,
        cx: &mut Context<Self>,
    ) {
        if let Some(published_state) = published_state {
            self.shared_collaboration
                .record_published_state(session_id, published_state);
        }
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
            return;
        };
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == tab_id)
            && let Err(error) = self
                .shared_collaboration
                .apply_snapshot_to_tab(session_id, state, tab)
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
        session_id: u64,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let Some((client, request, published_state)) =
            self.shared_publication_request(tab_id, session_id, cx)
        else {
            self.finish_shared_operation(tab_id, session_id, cx);
            return;
        };
        let sent_durable_state = published_state.is_some();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.apply_shared_with_request(request) })
                .await;
            this.update(cx, |this, cx| {
                let settled = match result {
                    Ok(zmux::client::SharedOperationResult::Applied(state)) => {
                        this.install_shared_publication_response(
                            session_id,
                            published_state,
                            state,
                            cx,
                        );
                        sent_durable_state
                    }
                    // Refused for a revision that moved underneath the request,
                    // which for an exact-revision operation needs no more than
                    // another viewer changing the geometry. Worth one more try
                    // from the state the refusal carried.
                    Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        this.install_shared_publication_response(session_id, None, state, cx);
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
                    this.shared_collaboration.enqueue(session_id, retry);
                }
                this.finish_shared_operation(tab_id, session_id, cx);
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
        session_id: u64,
        local_pane_id: u64,
        mux_pane_id: u64,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        let Some(runtime) = self.mux_panes.runtime_for_tab(tab_id) else {
            self.abandon_shared_pane_close(tab_id, session_id, local_pane_id, cx);
            // Still finished, even though nothing was sent: an operation taken
            // off the queue holds the session until it reports back, so
            // returning here would wedge every later close and publication.
            self.finish_shared_operation(tab_id, session_id, cx);
            return;
        };
        let client = runtime.client().clone();
        let base_revision = self
            .shared_collaboration
            .state(session_id)
            .map(|state| state.revision);
        let executor = cx.background_executor().clone();
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
                        this.install_shared_snapshot(session_id, state, cx);
                        true
                    }
                    Ok(zmux::client::SharedOperationResult::Conflict(state)) => {
                        // The daemon validated the close against a tree this
                        // window had not caught up with. If the pane is gone
                        // from it, somebody else already closed it.
                        let gone = !state.contains_pane(mux_pane_id);
                        this.install_shared_snapshot(session_id, state, cx);
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
                        this.abandon_shared_pane_close(tab_id, session_id, local_pane_id, cx);
                    } else {
                        this.shared_collaboration.enqueue(
                            session_id,
                            SharedOperation::ClosePane {
                                local_pane_id,
                                mux_pane_id,
                                attempts: attempts + 1,
                            },
                        );
                    }
                }
                this.finish_shared_operation(tab_id, session_id, cx);
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
        session_id: u64,
        local_pane_id: u64,
        cx: &mut Context<Self>,
    ) {
        self.closing_shared_panes.remove(&local_pane_id);
        let label = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.pane(local_pane_id))
            .map_or_else(|| local_pane_id.to_string(), TerminalPane::label);
        log::warn!(
            "shared session {session_id} would not close pane {local_pane_id}; leaving it open"
        );
        self.pane_output_error = Some(format!(
            "The session's multiplexer would not close pane {label}. It is still running for every viewer."
        ));
        cx.notify();
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
        session_id: u64,
        cx: &App,
    ) -> Option<(
        Arc<zmux::client::Client>,
        zmux::messages::SharedSessionOperationRequest,
        Option<serde_json::Value>,
    )> {
        let runtime = self.mux_panes.runtime_for_tab(tab_id)?;
        let canonical = self.shared_collaboration.state(session_id)?;
        let base_revision = canonical.revision;
        let tab = self.tabs.iter().find(|tab| tab.id == tab_id)?;
        let summary = self.shared_summary_in_mux_ids(tab, session_id, cx);
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
            )
        });
        let (operation, published_state) = match geometry {
            Some(operation) => (operation, None),
            None => {
                let (operation, state) = self.shared_durable_operation(tab, session_id, summary)?;
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
        session_id: u64,
        cx: &App,
    ) -> Option<BackgroundSessionSummary> {
        let protected = tab
            .close_policy
            .background_authentication()
            .flatten()
            .is_some();
        let mut summary = self.background_session_summary(tab, protected, cx);
        if let Err(error) = remap_summary_to_mux(&mut summary, self.mux_panes.ids()) {
            // Not `debug`: this is the window and the daemon disagreeing about
            // which panes the session holds, which is what leaves geometry
            // diverged with nothing saying so.
            log::warn!(
                "could not describe shared session {session_id} in the multiplexer's pane ids: \
                 {error:#}"
            );
            return None;
        }
        summary.id = session_id;
        Some(summary)
    }

    /// The durable tab state to publish and the blob it carries, or `None` when
    /// this window already published exactly these bytes.
    fn shared_durable_operation(
        &self,
        tab: &Tab,
        session_id: u64,
        summary: Option<BackgroundSessionSummary>,
    ) -> Option<(zmux::messages::SharedSessionOperation, serde_json::Value)> {
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
            .durable_state_is_unpublished(session_id, &state)
        {
            return None;
        }
        Some((durable_state_operation(state.clone(), summary), state))
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
        // Recorded for every caller that applies a snapshot, not just this
        // loop: a committed spawn and a conflict response both arrive with a
        // state that can name a pane this window still has to attach.
        self.shared_collaboration
            .set_connection(session_id, connection.clone());
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
                    | zmux::client::SharedSessionEvent::PanesChanged { state, .. } => {
                        this.update_in(cx, |this, window, cx| {
                            this.apply_shared_snapshot(session_id, state, window, cx);
                        })
                        .ok();
                        None
                    }
                    // A pane added by another viewer needs no handling of its
                    // own: applying the snapshot attaches every pane it names
                    // that this window does not hold, which is exactly this one.
                    zmux::client::SharedSessionEvent::PaneAdded {
                        session_id, state, ..
                    } => {
                        this.update_in(cx, |this, window, cx| {
                            this.apply_shared_snapshot(session_id, state, window, cx);
                        })
                        .ok();
                        None
                    }
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

    /// Attaches every pane the snapshot holds that this window does not.
    ///
    /// One task per pane, each claimed through `begin_attach` so the several
    /// callers that apply a snapshot cannot attach the same pane twice. Each
    /// task re-applies the snapshot it ends up with, which is what installs the
    /// geometry that names the pane it just attached.
    fn attach_panes_missing_locally(
        &mut self,
        session_id: u64,
        state: &SharedSessionState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let missing = self
            .shared_collaboration
            .missing_panes_for_snapshot(session_id, state);
        if missing.is_empty() {
            return;
        }
        let Some(connection) = self.shared_collaboration.connection(session_id) else {
            log::warn!(
                "shared session {session_id} has panes {missing:?} to attach but no daemon connection"
            );
            return;
        };
        for mux_pane_id in missing {
            if !self
                .shared_collaboration
                .begin_attach(session_id, mux_pane_id)
            {
                continue;
            }
            let connection = connection.clone();
            cx.spawn_in(window, async move |this, cx| {
                let attached =
                    attach_shared_pane_with_retries(session_id, mux_pane_id, &connection, cx).await;
                this.update_in(cx, |this, window, cx| {
                    this.shared_collaboration
                        .end_attach(session_id, mux_pane_id);
                    let Some((latest, pane)) = attached else {
                        return;
                    };
                    if let Some(pane) = pane
                        && let Err(error) =
                            this.attach_incoming_shared_pane(session_id, pane, latest.clone(), window, cx)
                    {
                        log::warn!(
                            "could not attach shared pane {mux_pane_id} of session {session_id}: {error:#}"
                        );
                        return;
                    }
                    this.apply_shared_snapshot(session_id, latest, window, cx);
                })
                .ok();
            })
            .detach();
        }
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
        if self
            .shared_collaboration
            .snapshot_is_stale(session_id, &state)
        {
            return;
        }
        self.attach_panes_missing_locally(session_id, &state, window, cx);
        if self.install_shared_snapshot(session_id, state, cx) {
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
        session_id: u64,
        state: SharedSessionState,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(tab_id) = self.shared_collaboration.tab_id(session_id) else {
            return false;
        };
        // Staleness is decided before anything is removed. A snapshot that will
        // not be applied must not take panes out of the tab: the layout is only
        // rewritten by the apply below, so removing for a snapshot that is then
        // discarded leaves the tab holding a layout entry with no pane behind it.
        if self
            .shared_collaboration
            .snapshot_is_stale(session_id, &state)
        {
            return false;
        }
        let revision = state.revision;
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
                false
            }
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
            detach_pane_from_tab(tab, local_pane_id);
        }
        self.closing_shared_panes.remove(&local_pane_id);
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
        // A profile of this name in *this* window supplies the theme and icon a
        // replicated pane is drawn with. Its command is local and stays local:
        // the pane's process runs on the session's host, and a split from it
        // sends this name back for that host to resolve. See
        // `terminal_spawn::shared_draft_process`.
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
                detach_pane_from_tab(tab, local_pane_id);
            }
            self.mux_panes.forget_pane(local_pane_id);
        }
        self.apply_shared_snapshot(session_id, state, window, cx);
        cx.notify();
    }
}

/// Puts back a pane the session's layout does not place.
///
/// A split puts its pane in the tab before the session has accepted it, so any
/// snapshot arriving in between describes a tab without that pane. Installing
/// one verbatim drops it out of the layout, and the split — whose whole
/// proposal is "put this pane next to that one" — then has nothing to describe.
///
/// It goes back beside whatever it was split from, read out of the layout being
/// replaced, which is where the session is about to put it too. Reports whether
/// there was somewhere to put it; there is not if the pane it was split from
/// has itself gone.
fn reinsert_unplaced_pane(previous: &PaneLayout, layout: &mut PaneLayout, pane_id: u64) -> bool {
    let Some((axis, first_ratio, draft_is_first, sibling)) = pane_split_context(previous, pane_id)
    else {
        return false;
    };
    let anchor = sibling.first_pane();
    if !layout.contains_pane(anchor) {
        return false;
    }
    let pane = Box::new(PaneLayout::Pane(pane_id));
    let anchor_layout = Box::new(PaneLayout::Pane(anchor));
    let (first, second) = if draft_is_first {
        (pane, anchor_layout)
    } else {
        (anchor_layout, pane)
    };
    layout.replace(
        anchor,
        PaneLayout::Split {
            axis,
            first_ratio,
            first,
            second,
        },
    )
}

/// The split that holds `pane_id` directly: its axis and ratio, whether the
/// pane is the first child, and the subtree on the other side.
fn pane_split_context(
    layout: &PaneLayout,
    pane_id: u64,
) -> Option<(SplitAxis, u16, bool, &PaneLayout)> {
    let PaneLayout::Split {
        axis,
        first_ratio,
        first,
        second,
    } = layout
    else {
        return None;
    };
    if matches!(first.as_ref(), PaneLayout::Pane(id) if *id == pane_id) {
        return Some((*axis, *first_ratio, true, second));
    }
    if matches!(second.as_ref(), PaneLayout::Pane(id) if *id == pane_id) {
        return Some((*axis, *first_ratio, false, first));
    }
    pane_split_context(first, pane_id).or_else(|| pane_split_context(second, pane_id))
}

/// The panes in this tab whose session pane is gone.
///
/// A pane with no translation at all is *not* one of them: it is a draft the
/// session has not been told about yet, which is exactly the state a tab is in
/// while a split is being proposed. Treating "no translation" as "lost" would
/// delete the pane the proposal is about.
fn panes_the_session_lost(
    tab: &Tab,
    local_to_mux: &HashMap<u64, u64>,
    state: &SharedSessionState,
) -> Vec<u64> {
    tab.panes
        .iter()
        .filter(|pane| {
            local_to_mux
                .get(&pane.id)
                .is_some_and(|mux_pane_id| !state.contains_pane(*mux_pane_id))
        })
        .map(|pane| pane.id)
        .collect()
}

/// Takes a pane out of a tab and out of its layout.
///
/// `Tab::remove_pane` deliberately leaves the layout alone, because most callers
/// reshape it themselves. The shared-session removers do not have a reshape of
/// their own — they expect the canonical snapshot to supply one — so they have
/// to collapse the split here. A snapshot that turns out to be unapplicable
/// would otherwise leave the pane's region reserved forever.
fn detach_pane_from_tab(tab: &mut Tab, pane_id: u64) {
    tab.remove_pane(pane_id);
    if let Some(layout) = tab.layout.clone().without(pane_id) {
        tab.layout = layout;
    }
    tab.restore_focus_after_close(pane_id, tab.layout.first_pane());
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

/// The one canonical geometry change a publication owes, or `None` when the
/// daemon's geometry already matches this window's.
///
/// Geometry converges a dimension at a time, deliberately: each operation names
/// what changed rather than replacing the tree, so two viewers moving different
/// dividers do not undo one another. The caller sends the durable tab state
/// once this returns `None`, which is what keeps an icon or a rename from being
/// dropped because the layout happened to differ in the same publication.
fn shared_geometry_operation(
    canonical: &zmux::messages::SharedPresentationState,
    summary: &BackgroundSessionSummary,
    maximized_pane: Option<u64>,
    minimized_panes: &[u64],
) -> Option<zmux::messages::SharedSessionOperation> {
    if summary.layout != canonical.layout {
        return Some(
            shared_layout_operation(&canonical.layout, &summary.layout, summary.active_pane)
                .unwrap_or(zmux::messages::SharedSessionOperation::SetLayout {
                    layout: summary.layout.clone(),
                }),
        );
    }
    if summary.active_pane != canonical.active_pane {
        return Some(zmux::messages::SharedSessionOperation::SetFocus {
            pane_id: summary.active_pane,
        });
    }
    if maximized_pane != canonical.maximized_pane {
        return Some(zmux::messages::SharedSessionOperation::SetMaximized {
            pane_id: maximized_pane,
        });
    }
    if minimized_panes == canonical.minimized_panes {
        return None;
    }
    let changed = minimized_panes
        .iter()
        .chain(&canonical.minimized_panes)
        .copied()
        .find(|pane_id| {
            minimized_panes.contains(pane_id) != canonical.minimized_panes.contains(pane_id)
        })?;
    Some(zmux::messages::SharedSessionOperation::SetMinimized {
        pane_id: changed,
        minimized: minimized_panes.contains(&changed),
    })
}

/// How the tab state the daemon stores without reading it — icons, titles, pane
/// names, themes and overlays — is carried to the other viewers.
///
/// `ReplaceTab` when the summary can be described, because that also refreshes
/// what the catalog and the pickers show; `SetTabState` otherwise, so a tab
/// holding a pane the daemon has not committed yet still publishes what the user
/// chose. The daemon discards a `ReplaceTab`'s layout and focus in favour of its
/// own presentation, so neither form can move geometry — which is why the two
/// halves of a publication are independent.
fn durable_state_operation(
    state: serde_json::Value,
    summary: Option<BackgroundSessionSummary>,
) -> zmux::messages::SharedSessionOperation {
    match summary {
        Some(summary) => zmux::messages::SharedSessionOperation::ReplaceTab { summary, state },
        None => zmux::messages::SharedSessionOperation::SetTabState { state },
    }
}

/// The publication to queue when this one has not converged yet, or `None` once
/// it has asked as many times as it may.
///
/// Geometry the daemon will not accept would otherwise re-queue for ever: each
/// answer installs the canonical state, the tab still disagrees, and the next
/// publication proposes the same change again.
fn publication_retry(attempts: u32) -> Option<SharedOperation> {
    (attempts + 1 < SHARED_PUBLISH_ATTEMPTS).then_some(SharedOperation::PublishState {
        attempts: attempts + 1,
    })
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

/// Refuses a layout that names a pane the tab does not hold.
///
/// `render_pane_leaf` draws an empty region for a layout id with no
/// `TerminalPane`, and the split node holding it goes on reserving the space,
/// so the survivors never grow back. The reverse direction — a pane the layout
/// does not place — is reported but not refused, because a pane whose creation
/// the daemon has not committed yet is legitimately in neither.
fn ensure_layout_covers_tab(tab: &Tab, layout: &PaneLayout) -> Result<()> {
    let placed = layout.pane_ids();
    if let Some(missing) = placed.iter().find(|pane_id| tab.pane(**pane_id).is_none()) {
        anyhow::bail!(
            "shared layout places pane {missing}, which tab {} does not hold",
            tab.id
        );
    }
    let unplaced = tab
        .panes
        .iter()
        .filter(|pane| !placed.contains(&pane.id))
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    if !unplaced.is_empty() {
        log::warn!(
            "shared layout for tab {} does not place pane(s) {unplaced:?}; they are awaiting a commit",
            tab.id
        );
    }
    Ok(())
}

fn apply_canonical_presentation(
    tab: &mut Tab,
    state: &SharedSessionState,
    layout: PaneLayout,
    mux_to_local: &HashMap<u64, u64>,
) -> Result<()> {
    tab.layout = layout;
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

pub(crate) fn remap_summary_to_mux(
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
