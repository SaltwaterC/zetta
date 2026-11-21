//! Delivering pane exits and signals to whoever is waiting on them.

use super::*;
use zetta_sync::RecoverPoison as _;

/// How a pane's exit is reported once the multiplexer says it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneExitReport {
    /// The raw `waitpid` status, as the multiplexer observed it.
    pub raw_status: Option<i32>,
    /// Whether any shared client typed into the pane — the multiplexer's own
    /// attribution, which no single client can assemble by itself.
    pub input_sent: bool,
    /// The multiplexer's connection died without reporting an exit.
    pub disconnected: bool,
}

/// Where a pane's exit is delivered once the multiplexer reports it.
///
/// An attached terminal reads the real PTY, so it never learns the exit status
/// by itself: only the multiplexer is the child's parent. An exclusively
/// attached terminal learns it through the child-event channel its event loop
/// polls; a shared terminal has no event loop, so its holder registers a
/// channel and is woken directly.
#[derive(Default)]
pub(super) struct ExitReporterState {
    pub(super) reporters: HashMap<u64, alacritty_terminal::tty::AttachedChildEvents>,
    pub(super) shared: HashMap<u64, async_channel::Sender<PaneExitReport>>,
    /// Exits reported for a pane that had no reporter yet.
    ///
    /// The multiplexer starts the process the moment it is asked, so a pane can
    /// end before the terminal showing it has been built — a bad shell, a
    /// failing `exec`, an instant command, a fast Ctrl-D. Dropping the report
    /// then left the terminal waiting for an event that had already happened,
    /// with nothing able to produce it a second time.
    pub(super) pending: HashMap<u64, PaneExitReport>,
}

#[derive(Default)]
pub struct ExitReporters {
    /// Registration and pending delivery must be one atomic state transition:
    /// otherwise a report can observe no reporter, registration can observe no
    /// pending report, and the report can then be inserted after registration
    /// has finished checking.
    pub(super) state: Mutex<ExitReporterState>,
}

impl ExitReporters {
    pub fn register(&self, pane_id: u64, reporter: alacritty_terminal::tty::AttachedChildEvents) {
        let delivery = {
            let mut state = self.state.lock().recover_poison();
            state.reporters.insert(pane_id, reporter);
            state.pending.remove(&pane_id).map(|report| {
                let reporter = state
                    .reporters
                    .remove(&pane_id)
                    .expect("the attached reporter was just registered");
                (reporter, report)
            })
        };
        if let Some((reporter, report)) = delivery {
            Self::deliver_attached(reporter, report);
        }
    }

    pub fn forget(&self, pane_id: u64) {
        let mut state = self.state.lock().recover_poison();
        state.reporters.remove(&pane_id);
        state.pending.remove(&pane_id);
    }

    /// The panes still waiting to be told their process ended.
    ///
    /// Both registries, because either kind of holder needs catching up after a
    /// subscription is re-established.
    pub fn registered(&self) -> Vec<u64> {
        let state = self.state.lock().recover_poison();
        let mut ids = state.reporters.keys().copied().collect::<Vec<_>>();
        ids.extend(state.shared.keys().copied());
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Routes a shared pane's exit to a channel the pane's holder drains.
    ///
    /// The channel is asynchronous so the holder can await it from its main
    /// task; the sender is used from the subscription thread, where
    /// [`async_channel::Sender::try_send`] cannot fail on an unbounded
    /// channel unless the receiver is gone.
    pub fn register_shared(&self, pane_id: u64, reporter: async_channel::Sender<PaneExitReport>) {
        let delivery = {
            let mut state = self.state.lock().recover_poison();
            state.shared.insert(pane_id, reporter);
            state.pending.remove(&pane_id).map(|report| {
                let reporter = state
                    .shared
                    .remove(&pane_id)
                    .expect("the shared reporter was just registered");
                (reporter, report)
            })
        };
        if let Some((reporter, report)) = delivery {
            Self::deliver_shared(reporter, report);
        }
    }

    pub fn forget_shared(&self, pane_id: u64) {
        let mut state = self.state.lock().recover_poison();
        state.shared.remove(&pane_id);
        state.pending.remove(&pane_id);
    }

    pub(super) fn report(&self, pane_id: u64, raw_status: Option<i32>, input_sent: bool) {
        let report = PaneExitReport {
            raw_status,
            input_sent,
            disconnected: false,
        };
        let (reporter, shared) = {
            let mut state = self.state.lock().recover_poison();
            let reporter = state.reporters.remove(&pane_id);
            let shared = state.shared.remove(&pane_id);
            if reporter.is_none() && shared.is_none() {
                state.pending.insert(pane_id, report);
            }
            (reporter, shared)
        };
        if let Some(reporter) = reporter {
            Self::deliver_attached(reporter, report);
        }
        if let Some(reporter) = shared {
            Self::deliver_shared(reporter, report);
        }
    }

    pub(super) fn report_all_disconnected(&self) {
        let (reporters, shared) = {
            let mut state = self.state.lock().recover_poison();
            (
                state
                    .reporters
                    .drain()
                    .map(|(_, reporter)| reporter)
                    .collect::<Vec<_>>(),
                state
                    .shared
                    .drain()
                    .map(|(_, reporter)| reporter)
                    .collect::<Vec<_>>(),
            )
        };
        for mut reporter in reporters {
            let _ = reporter.report_watcher_disconnected();
        }
        for reporter in shared {
            let _ = reporter.try_send(PaneExitReport {
                raw_status: None,
                input_sent: false,
                disconnected: true,
            });
        }
    }

    pub(super) fn deliver_attached(
        mut reporter: alacritty_terminal::tty::AttachedChildEvents,
        report: PaneExitReport,
    ) {
        let _ = if report.disconnected {
            reporter.report_watcher_disconnected()
        } else {
            match report.raw_status {
                Some(status) => reporter.report_exit(status),
                None => reporter.report_status_unavailable(),
            }
        };
    }

    pub(super) fn deliver_shared(
        reporter: async_channel::Sender<PaneExitReport>,
        report: PaneExitReport,
    ) {
        let _ = reporter.try_send(report);
    }
}

/// Where a pane's revoke — the notice that another client attached and the
/// pane is becoming shared — is delivered.
///
/// The holder must stop reading the pane, snapshot its screen, and re-attach
/// in shared mode, all of which need the application's main thread, so this is
/// a channel rather than a child-event stream: the app drains it from a
/// background task that hops back onto the main thread.
#[derive(Default)]
pub struct PaneSignals {
    pub(super) reporters: Mutex<HashMap<u64, async_channel::Sender<()>>>,
    /// Signals that arrived before the pane had somewhere to deliver them.
    ///
    /// The multiplexer gives a holder a bounded time to answer a revoke, so a
    /// revoke dropped for a pane that was still being built costs the attaching
    /// client that whole timeout and then fails it.
    pub(super) pending: Mutex<std::collections::HashSet<u64>>,
}

impl PaneSignals {
    pub fn register(&self, pane_id: u64, reporter: async_channel::Sender<()>) {
        self.reporters
            .lock()
            .recover_poison()
            .insert(pane_id, reporter);
        if self.pending.lock().recover_poison().remove(&pane_id) {
            self.report(pane_id);
        }
    }

    pub fn forget(&self, pane_id: u64) {
        self.reporters.lock().recover_poison().remove(&pane_id);
        self.pending.lock().recover_poison().remove(&pane_id);
    }

    pub(super) fn report(&self, pane_id: u64) {
        let mut reporters = self.reporters.lock().recover_poison();
        if let Some(reporter) = reporters.remove(&pane_id) {
            let _ = reporter.try_send(());
            return;
        }
        drop(reporters);
        self.pending.lock().recover_poison().insert(pane_id);
    }
}
