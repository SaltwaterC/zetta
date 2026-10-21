//! Window-local pane and tab identities mapped to daemon sessions.
//!
//! This registry also fences asynchronous pane removals: a later session
//! publication must wait for every pane it omits to leave the daemon.

use super::*;

/// Where each visible pane's terminal lives in the multiplexer.
///
/// Kept beside the tabs rather than inside them because a pane's identity in
/// the multiplexer outlives the `TerminalPane` that displays it: the same
/// terminal is shown by a new pane after a session is attached in another
/// window.
#[derive(Default)]
pub(crate) struct MuxPanes {
    /// Local pane id to the pane id its multiplexer knows it by. Keyed by the
    /// local id only: a multiplexer pane id is unique within one daemon, and a
    /// window can hold panes of several — its own, and every remote host it
    /// has a tab on — whose numbering all starts at 1. A reverse map keyed by
    /// that number let recording a remote pane silently unmap the local pane
    /// that shared its number; see [`Self::local_pane_id_among`].
    panes: HashMap<u64, u64>,
    sessions: HashMap<u64, MuxSession>,
    runtimes: HashMap<u64, MuxRuntime>,
    pending_closes: HashMap<u64, Vec<PendingPaneClose>>,
}

/// A close finishes off the GUI thread. Session publications wait for its
/// acknowledgement so an immediate detach cannot restore the obsolete pane.
pub(crate) type PendingPaneClose =
    futures::future::Shared<futures::channel::oneshot::Receiver<std::result::Result<(), String>>>;

impl MuxPanes {
    /// The session a tab's panes belong to, creating one if this is the tab's
    /// first pane.
    pub(crate) fn session_for_tab(&mut self, tab_id: u64) -> MuxSession {
        self.sessions.entry(tab_id).or_default().clone()
    }

    pub(crate) fn adopt_session(&mut self, tab_id: u64, session_id: u64) {
        self.sessions
            .insert(tab_id, MuxSession::from_id(session_id));
    }

    pub(crate) fn adopt_session_with_runtime(
        &mut self,
        tab_id: u64,
        session_id: u64,
        runtime: MuxRuntime,
    ) {
        self.adopt_session(tab_id, session_id);
        self.runtimes.insert(tab_id, runtime);
    }

    pub(crate) fn runtime_for_tab(&self, tab_id: u64) -> Option<MuxRuntime> {
        self.runtimes.get(&tab_id).cloned()
    }

    pub(crate) fn is_remote_tab(&self, tab_id: u64) -> bool {
        self.runtimes
            .get(&tab_id)
            .is_some_and(MuxRuntime::is_remote)
    }

    /// The SSH destination a tab's session lives behind, for a tab attached to
    /// a remote multiplexer. `None` for a local tab, so this answers both
    /// "is it remote" and "which host" in one lookup.
    pub(crate) fn remote_tab_destination(&self, tab_id: u64) -> Option<&str> {
        self.runtimes
            .get(&tab_id)
            .and_then(MuxRuntime::remote_destination)
    }

    pub(crate) fn session_id(&self, tab_id: u64) -> Option<u64> {
        self.sessions.get(&tab_id)?.id()
    }

    /// A tab's session as a key that says which daemon it belongs to. A tab
    /// with no runtime of its own is on this window's local daemon.
    pub(crate) fn shared_key(&self, tab_id: u64) -> Option<SharedSessionKey> {
        Some(SharedSessionKey {
            daemon: self
                .runtimes
                .get(&tab_id)
                .map_or(MuxDaemon::Local, MuxRuntime::daemon),
            session_id: self.session_id(tab_id)?,
        })
    }

    /// Whether a tab in this window is already showing this session.
    ///
    /// The reconnect picker needs this: attaching a session the same *process*
    /// already holds is not a join, because the multiplexer recognises its own
    /// holder and hands the terminal straight back — so it would open a second
    /// tab reading the same pty as the first. Two readers of one terminal split
    /// its output between them arbitrarily.
    pub(crate) fn holds_session(&self, session_id: u64) -> bool {
        self.sessions.iter().any(|(tab_id, session)| {
            self.runtimes
                .get(tab_id)
                .is_none_or(|runtime| !runtime.is_remote())
                && session.id() == Some(session_id)
        })
    }

    pub(crate) fn record(&mut self, pane_id: u64, mux_pane_id: u64) {
        self.panes.insert(pane_id, mux_pane_id);
    }

    pub(crate) fn mux_pane_id(&self, pane_id: u64) -> Option<u64> {
        self.panes.get(&pane_id).copied()
    }

    /// The local pane, among `pane_ids`, that its multiplexer knows as
    /// `mux_pane_id`. Asked of one tab's panes, never of the window: the same
    /// multiplexer pane id names a different pane on every daemon.
    pub(crate) fn local_pane_id_among(
        &self,
        pane_ids: impl IntoIterator<Item = u64>,
        mux_pane_id: u64,
    ) -> Option<u64> {
        pane_ids
            .into_iter()
            .find(|pane_id| self.mux_pane_id(*pane_id) == Some(mux_pane_id))
    }

    pub(crate) fn ids(&self) -> &HashMap<u64, u64> {
        &self.panes
    }

    pub(crate) fn forget_pane(&mut self, pane_id: u64) {
        self.panes.remove(&pane_id);
    }

    pub(crate) fn forget_tab(&mut self, tab_id: u64) {
        self.sessions.remove(&tab_id);
        self.runtimes.remove(&tab_id);
        self.pending_closes.remove(&tab_id);
    }

    pub(crate) fn record_pending_close(&mut self, tab_id: u64, close: PendingPaneClose) {
        let closes = self.pending_closes.entry(tab_id).or_default();
        closes.retain(|close| !matches!(close.clone().now_or_never(), Some(Ok(Ok(())))));
        closes.push(close);
    }

    pub(crate) fn pending_closes(&self, tab_id: u64) -> Vec<PendingPaneClose> {
        self.pending_closes
            .get(&tab_id)
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
#[path = "../tests/mux/panes.rs"]
mod tests;
