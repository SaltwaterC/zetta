//! The coordinator's bookkeeping for each shared session this window is bound
//! to: the pane-id translation, the operations waiting to be published, the
//! watch connection, and the focus a click asked for.

use super::*;

#[derive(Default)]
pub(crate) struct SharedSessionCoordinator {
    /// Keyed by daemon as well as number: see [`SharedSessionKey`].
    pub(super) sessions: HashMap<SharedSessionKey, SharedSessionBinding>,
    pub(super) next_watch_id: u64,
}

pub(super) struct SharedSessionBinding {
    pub(super) tab_id: u64,
    pub(super) state: SharedSessionState,
    /// The opaque tab blob this window last wrote into its tab.
    ///
    /// Only `ReplaceTab` and `SetTabState` replace the daemon's copy, so every
    /// geometry revision carries the blob from whichever publication set it
    /// last. Applying that again would rewrite the icon, the titles, the pane
    /// names and the overlays from it — undoing a local change that has not
    /// been published yet. Compared rather than the revision because both sides
    /// are the same daemon field in the publisher's own id space.
    pub(super) applied_state: serde_json::Value,
    /// The blob this window last published, so a publication driven by geometry
    /// churn does not send identical bytes again.
    pub(super) last_published_state: Option<serde_json::Value>,
    pub(super) mux_to_local: HashMap<u64, u64>,
    pub(super) local_to_mux: HashMap<u64, u64>,
    /// Canonical mutations waiting their turn. Every change this window asks
    /// the daemon for goes through here in order — a state publication and a
    /// pane close racing on the same revision is how a close came to be
    /// rejected and then silently forgotten.
    pub(super) queue: VecDeque<SharedOperation>,
    pub(super) in_flight: bool,
    /// The newest keyboard focus choice that has not yet been reflected by a
    /// canonical snapshot. A recovery snapshot may legitimately repeat the
    /// previous focus at the same revision, so it must not undo this choice
    /// while its explicit request is still queued or in flight.
    pub(super) pending_focus: Option<PendingFocusIntent>,
    pub(super) next_focus_generation: u64,
    /// Whether a debounce timer is already running for a state publication, so
    /// a gesture that notifies dozens of times starts one timer, not dozens.
    pub(super) publication_scheduled: bool,
    pub(super) watch_id: Option<u64>,
    /// The stable ids this window is in the middle of attaching. Snapshots are
    /// applied from several places at once, so without this the same pane is
    /// attached twice when two of them see it missing before either finishes.
    pub(super) attaching: HashSet<u64>,
    /// The stable ids whose broken stream this window is replacing. A stream
    /// can be reported broken twice — by the daemon, and by the pane's own
    /// reader — and answering both attaches a second replacement that nothing
    /// reads.
    pub(super) reattaching: HashSet<u64>,
    /// How to reach the daemon for this session. Kept here because a snapshot
    /// is applied from callers that have no connection of their own — a
    /// committed spawn, a conflict response — and every one of them may have to
    /// attach a pane the snapshot introduced.
    pub(super) connection: Option<SharedWatchConnection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PendingFocusIntent {
    pub(super) mux_pane_id: u64,
    pub(super) generation: u64,
}

#[derive(Clone)]
pub(super) struct SharedWatchConnection {
    pub(super) client: Arc<zmux::client::Client>,
    pub(super) secret: Option<zmux::auth::SessionSecret>,
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
    /// A keyboard focus choice. Unlike [`Self::PublishState`], it is not
    /// debounced: it guards local focus against recovery snapshots until the
    /// daemon has acknowledged this exact generation.
    FocusPane {
        mux_pane_id: u64,
        generation: u64,
        attempts: u32,
    },
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
    pub(super) fn is_same_work(self, other: Self) -> bool {
        match (self, other) {
            (Self::PublishState { .. }, Self::PublishState { .. }) => true,
            (
                Self::FocusPane { generation, .. },
                Self::FocusPane {
                    generation: other, ..
                },
            ) => generation == other,
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
        key: &SharedSessionKey,
        tab_id: u64,
        state: SharedSessionState,
        mappings: impl IntoIterator<Item = (u64, u64)>,
    ) -> Result<()> {
        anyhow::ensure!(
            state.session_id == key.session_id,
            "shared snapshot belongs to session {}, expected {}",
            state.session_id,
            key.session_id
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
            key.clone(),
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
                pending_focus: None,
                next_focus_generation: 0,
                publication_scheduled: false,
                watch_id: None,
                attaching: HashSet::new(),
                reattaching: HashSet::new(),
                connection: None,
            },
        );
        Ok(())
    }

    pub(crate) fn forget(&mut self, key: &SharedSessionKey) {
        self.sessions.remove(key);
    }

    pub(crate) fn tab_id(&self, key: &SharedSessionKey) -> Option<u64> {
        self.sessions.get(key).map(|session| session.tab_id)
    }

    pub(crate) fn is_bound(&self, key: &SharedSessionKey) -> bool {
        self.sessions.contains_key(key)
    }

    pub(crate) fn begin_watch(&mut self, key: &SharedSessionKey) -> Option<u64> {
        if self
            .sessions
            .get(key)
            .is_none_or(|session| session.watch_id.is_some())
        {
            return None;
        }
        self.next_watch_id = self.next_watch_id.wrapping_add(1);
        let watch_id = self.next_watch_id;
        self.sessions
            .get_mut(key)
            .expect("shared session was checked above")
            .watch_id = Some(watch_id);
        Some(watch_id)
    }

    pub(crate) fn watch_is_current(&self, key: &SharedSessionKey, watch_id: u64) -> bool {
        self.sessions
            .get(key)
            .is_some_and(|session| session.watch_id == Some(watch_id))
    }

    pub(crate) fn end_watch(&mut self, key: &SharedSessionKey, watch_id: u64) {
        if let Some(session) = self.sessions.get_mut(key)
            && session.watch_id == Some(watch_id)
        {
            session.watch_id = None;
        }
    }

    pub(crate) fn state(&self, key: &SharedSessionKey) -> Option<&SharedSessionState> {
        self.sessions.get(key).map(|session| &session.state)
    }

    /// Whether this blob says something the daemon has not been told yet.
    ///
    /// Compared against what *this window* last published rather than against
    /// the canonical copy: the two windows write their own pane ids into the
    /// blob, so the canonical copy differs from ours whenever the other window
    /// published last, even when both describe the same tab.
    pub(crate) fn durable_state_is_unpublished(
        &self,
        key: &SharedSessionKey,
        state: &serde_json::Value,
    ) -> bool {
        self.sessions
            .get(key)
            .is_some_and(|session| session.last_published_state.as_ref() != Some(state))
    }

    pub(crate) fn record_published_state(
        &mut self,
        key: &SharedSessionKey,
        state: serde_json::Value,
    ) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.last_published_state = Some(state);
        }
    }

    /// Whether this snapshot has already been superseded. Checked *before*
    /// anything is removed for it: a snapshot that will not be applied must not
    /// take panes out of the tab, because nothing would then put the layout
    /// back together.
    pub(crate) fn snapshot_is_stale(
        &self,
        key: &SharedSessionKey,
        state: &SharedSessionState,
    ) -> bool {
        self.sessions
            .get(key)
            .is_some_and(|session| state.revision < session.state.revision)
    }

    /// Claims a pane for attachment, or reports that another task already has
    /// it. Released by [`Self::end_attach`] whether the attachment succeeded or
    /// not, so a failure is retried by the next snapshot rather than wedged.
    pub(crate) fn begin_attach(&mut self, key: &SharedSessionKey, mux_pane_id: u64) -> bool {
        self.sessions
            .get_mut(key)
            .is_some_and(|session| session.attaching.insert(mux_pane_id))
    }

    pub(crate) fn end_attach(&mut self, key: &SharedSessionKey, mux_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.attaching.remove(&mux_pane_id);
        }
    }

    /// Claims a pane's stream replacement, or reports that another task is
    /// already replacing it. Released by [`Self::end_reattach`].
    pub(super) fn begin_reattach(&mut self, key: &SharedSessionKey, mux_pane_id: u64) -> bool {
        self.sessions
            .get_mut(key)
            .is_some_and(|session| session.reattaching.insert(mux_pane_id))
    }

    pub(super) fn end_reattach(&mut self, key: &SharedSessionKey, mux_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.reattaching.remove(&mux_pane_id);
        }
    }

    pub(super) fn set_connection(
        &mut self,
        key: &SharedSessionKey,
        connection: SharedWatchConnection,
    ) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.connection = Some(connection);
        }
    }

    pub(super) fn connection(&self, key: &SharedSessionKey) -> Option<SharedWatchConnection> {
        self.sessions
            .get(key)
            .and_then(|session| session.connection.clone())
    }

    /// Claims the debounce for a state publication. `false` means one is
    /// already scheduled and this caller has nothing to do.
    pub(crate) fn schedule_publication(&mut self, key: &SharedSessionKey) -> bool {
        self.sessions
            .get_mut(key)
            .is_some_and(|session| !std::mem::replace(&mut session.publication_scheduled, true))
    }

    pub(crate) fn clear_publication_schedule(&mut self, key: &SharedSessionKey) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.publication_scheduled = false;
        }
    }

    /// Records a local keyboard focus choice and replaces an unsent older
    /// choice. An older request already in flight stays ordered before this
    /// one, but its generation cannot acknowledge the newer intent.
    pub(crate) fn request_focus(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
    ) -> Option<u64> {
        let session = self.sessions.get_mut(key)?;
        if !session.state.contains_pane(mux_pane_id) {
            return None;
        }
        let Some(generation) = session.next_focus_generation.checked_add(1) else {
            log::warn!(
                "shared session {} exhausted its focus intent generations",
                key.session_id
            );
            return None;
        };
        session.next_focus_generation = generation;
        session.pending_focus = Some(PendingFocusIntent {
            mux_pane_id,
            generation,
        });
        session
            .queue
            .retain(|operation| !matches!(operation, SharedOperation::FocusPane { .. }));
        session.queue.push_back(SharedOperation::FocusPane {
            mux_pane_id,
            generation,
            attempts: 0,
        });
        Some(generation)
    }

    pub(super) fn focus_intent_is_current(
        &self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        generation: u64,
    ) -> bool {
        self.sessions.get(key).is_some_and(|session| {
            session.pending_focus
                == Some(PendingFocusIntent {
                    mux_pane_id,
                    generation,
                })
        })
    }

    pub(super) fn has_pending_focus(&self, key: &SharedSessionKey) -> bool {
        self.sessions
            .get(key)
            .is_some_and(|session| session.pending_focus.is_some())
    }

    /// Keeps a current local target through a recovery snapshot, but only for
    /// a pane that remains visible in the canonical presentation. A matching
    /// canonical focus is its acknowledgement and returns control to normal
    /// snapshot ordering.
    pub(super) fn pending_focus_for_snapshot(
        session: &mut SharedSessionBinding,
        state: &SharedSessionState,
        tab: &Tab,
        layout: &PaneLayout,
    ) -> Option<u64> {
        let intent = session.pending_focus?;
        if state.presentation.active_pane == intent.mux_pane_id {
            session.pending_focus = None;
            return None;
        }
        let Some(local_pane_id) = session.mux_to_local.get(&intent.mux_pane_id).copied() else {
            session.pending_focus = None;
            return None;
        };
        let still_visible = state.contains_pane(intent.mux_pane_id)
            && layout.contains_pane(local_pane_id)
            && tab.pane(local_pane_id).is_some();
        if still_visible {
            Some(local_pane_id)
        } else {
            session.pending_focus = None;
            None
        }
    }

    pub(super) fn clear_focus_intent(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        generation: u64,
    ) {
        if self.focus_intent_is_current(key, mux_pane_id, generation)
            && let Some(session) = self.sessions.get_mut(key)
        {
            session.pending_focus = None;
        }
    }

    /// Queues a canonical mutation, collapsing it into an equivalent one
    /// already waiting. A second publication would send the same tab state
    /// twice, and a second close of the same pane would be refused by the
    /// daemon for naming a pane it no longer holds.
    pub(crate) fn enqueue(&mut self, key: &SharedSessionKey, operation: SharedOperation) {
        if let Some(session) = self.sessions.get_mut(key)
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
    pub(crate) fn take_next_operation(
        &mut self,
        key: &SharedSessionKey,
    ) -> Option<SharedOperation> {
        let session = self.sessions.get_mut(key)?;
        if session.in_flight {
            return None;
        }
        let operation = session.queue.pop_front()?;
        session.in_flight = true;
        Some(operation)
    }

    pub(crate) fn finish_operation(&mut self, key: &SharedSessionKey) {
        if let Some(session) = self.sessions.get_mut(key) {
            session.in_flight = false;
        }
    }

    pub(crate) fn may_report_size(&self, key: &SharedSessionKey) -> bool {
        self.sessions
            .get(key)
            .is_some_and(|session| !session.in_flight)
    }

    pub(crate) fn local_pane_id(&self, key: &SharedSessionKey, mux_pane_id: u64) -> Option<u64> {
        self.sessions
            .get(key)
            .and_then(|session| session.mux_to_local.get(&mux_pane_id).copied())
    }

    pub(crate) fn mux_pane_id(&self, key: &SharedSessionKey, local_pane_id: u64) -> Option<u64> {
        self.sessions
            .get(key)
            .and_then(|session| session.local_to_mux.get(&local_pane_id).copied())
    }

    pub(super) fn panes_missing_from_snapshot(
        &self,
        key: &SharedSessionKey,
        state: &SharedSessionState,
    ) -> Vec<u64> {
        let Some(session) = self.sessions.get(key) else {
            return Vec::new();
        };
        let present = state.pane_ids().collect::<HashSet<_>>();
        session
            .local_to_mux
            .iter()
            .filter_map(|(local_id, mux_id)| (!present.contains(mux_id)).then_some(*local_id))
            .collect()
    }

    pub(super) fn missing_panes_for_snapshot(
        &self,
        key: &SharedSessionKey,
        state: &SharedSessionState,
    ) -> Vec<u64> {
        let Some(session) = self.sessions.get(key) else {
            return Vec::new();
        };
        state
            .pane_ids()
            .filter(|mux_id| {
                !session.mux_to_local.contains_key(mux_id) && !session.attaching.contains(mux_id)
            })
            .collect()
    }

    pub(crate) fn record_pane(
        &mut self,
        key: &SharedSessionKey,
        mux_pane_id: u64,
        local_pane_id: u64,
    ) {
        if let Some(session) = self.sessions.get_mut(key) {
            if let Some(previous_local) = session.mux_to_local.insert(mux_pane_id, local_pane_id) {
                session.local_to_mux.remove(&previous_local);
            }
            if let Some(previous_mux) = session.local_to_mux.insert(local_pane_id, mux_pane_id) {
                session.mux_to_local.remove(&previous_mux);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn remove_pane(&mut self, key: &SharedSessionKey, mux_pane_id: u64) -> Option<u64> {
        let session = self.sessions.get_mut(key)?;
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
    pub(crate) fn remove_local_pane(&mut self, key: &SharedSessionKey, local_pane_id: u64) {
        if let Some(session) = self.sessions.get_mut(key)
            && let Some(mux_pane_id) = session.local_to_mux.remove(&local_pane_id)
        {
            session.mux_to_local.remove(&mux_pane_id);
            if session
                .pending_focus
                .is_some_and(|intent| intent.mux_pane_id == mux_pane_id)
            {
                session.pending_focus = None;
            }
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
        key: &SharedSessionKey,
        state: SharedSessionState,
        tab: &mut Tab,
    ) -> Result<SharedSnapshotDisposition> {
        let Some(session) = self.sessions.get_mut(key) else {
            return Ok(SharedSnapshotDisposition::Unknown);
        };
        if state.revision < session.state.revision {
            return Ok(SharedSnapshotDisposition::Stale);
        }
        anyhow::ensure!(
            tab.id == session.tab_id,
            "shared session {} is bound to tab {}, not {}",
            key.session_id,
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
        let active_pane = Self::pending_focus_for_snapshot(session, &state, tab, &layout);
        apply_canonical_presentation(tab, &state, layout, &session.mux_to_local, active_pane)?;
        session.state = state;
        Ok(SharedSnapshotDisposition::Applied)
    }

    /// Records a pane before its terminal is built. This is intentionally
    /// separate from `apply_snapshot_to_tab`: a pane-added event carries a
    /// state that references the new stable id, while its relay attachment is
    /// still in flight.
    pub(crate) fn accept_pane_added(
        &mut self,
        key: &SharedSessionKey,
        state: SharedSessionState,
        mux_pane_id: u64,
        local_pane_id: u64,
    ) -> Result<SharedSnapshotDisposition> {
        let Some(session) = self.sessions.get_mut(key) else {
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
        key: &SharedSessionKey,
        state: SharedSessionState,
        mux_pane_id: u64,
    ) -> Result<Option<u64>> {
        let Some(session) = self.sessions.get_mut(key) else {
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
