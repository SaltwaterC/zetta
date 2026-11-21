//! The long-lived subscription that carries what is not an answer to any
//! request: panes exiting, shared sessions changing, and the daemon being
//! superseded.

use super::*;
use zetta_sync::RecoverPoison as _;

/// The registries a subscription delivers into, handed out together because the
/// events arrive on one connection and a caller needs all of them.
#[derive(Clone)]
pub struct Subscription {
    pub exits: Arc<ExitReporters>,
    pub revokes: Arc<PaneSignals>,
    pub grants: Arc<PaneSignals>,
    pub shared: Arc<SharedSessionReports>,
    pub superseded: Arc<SupersededMultiplexer>,
}

/// Says, once, that the multiplexer has been replaced by one speaking a
/// protocol this build does not.
///
/// It is not a lost multiplexer, and the difference is the whole reason this
/// exists. A lost one may come back, so the panes are held open for it and told
/// only after the grace period that nothing is coming. A superseded one is
/// already back, answering, and refusing this build for good: retrying cannot
/// help, and reporting the panes disconnected would end terminals that are
/// working perfectly well — an exclusively attached pane reads its own
/// descriptor and needs the multiplexer for nothing but its eventual exit
/// status. So the subscription stops here and says so, and whoever holds the
/// window turns that into something the user can act on.
#[derive(Default)]
pub struct SupersededMultiplexer {
    pub(super) state: Mutex<SupersededState>,
}

#[derive(Default)]
pub(super) struct SupersededState {
    pub(super) superseded: bool,
    /// The protocol the replacement published, when it could be read.
    pub(super) protocol_version: Option<u32>,
    pub(super) watchers: Vec<async_channel::Sender<Option<u32>>>,
}

impl SupersededMultiplexer {
    /// A channel that carries the replacement's protocol version, immediately
    /// if the replacement has already been found.
    ///
    /// Late watchers are the normal case rather than a race to lose: a window
    /// registers one when it installs its runtime, which can be after a
    /// subscription that was already superseded has given up.
    pub fn watch(&self) -> async_channel::Receiver<Option<u32>> {
        let (sender, receiver) = async_channel::unbounded();
        let mut state = self.state.lock().recover_poison();
        if state.superseded {
            let _ = sender.try_send(state.protocol_version);
        } else {
            state.watchers.push(sender);
        }
        receiver
    }

    pub fn is_superseded(&self) -> bool {
        self.state.lock().recover_poison().superseded
    }

    pub(super) fn report(&self, protocol_version: Option<u32>) {
        let watchers = {
            let mut state = self.state.lock().recover_poison();
            if state.superseded {
                return;
            }
            state.superseded = true;
            state.protocol_version = protocol_version;
            std::mem::take(&mut state.watchers)
        };
        for watcher in watchers {
            let _ = watcher.try_send(protocol_version);
        }
    }
}

#[derive(Clone, Debug)]
pub enum SharedSessionEvent {
    Updated(SharedSessionState),
    PaneAdded {
        session_id: u64,
        pane_id: u64,
        child_pid: u32,
        state: SharedSessionState,
    },
    PaneRemoved {
        session_id: u64,
        pane_id: u64,
        state: SharedSessionState,
    },
    PanesChanged {
        session_id: u64,
        added: Vec<u64>,
        removed: Vec<u64>,
        state: SharedSessionState,
    },
    StreamFailed {
        session_id: u64,
        pane_id: u64,
    },
}

/// Broadcasts canonical shared-session snapshots to the coordinator that owns
/// a remote tab. Keeping this registry beside the existing exit/revoke
/// registries means subscription reconnects can repair state without making
/// the render path open a socket.
#[derive(Default)]
pub struct SharedSessionReports {
    pub(super) receivers: Mutex<HashMap<u64, Vec<async_channel::Sender<SharedSessionEvent>>>>,
}

impl SharedSessionReports {
    pub fn register(&self, session_id: u64) -> async_channel::Receiver<SharedSessionEvent> {
        let (sender, receiver) = async_channel::unbounded();
        self.receivers
            .lock()
            .recover_poison()
            .entry(session_id)
            .or_default()
            .push(sender);
        receiver
    }

    pub fn forget(&self, session_id: u64) {
        self.receivers.lock().recover_poison().remove(&session_id);
    }

    pub(super) fn registered(&self) -> Vec<u64> {
        self.receivers
            .lock()
            .recover_poison()
            .keys()
            .copied()
            .collect()
    }

    pub(super) fn report(&self, event: SharedSessionEvent) {
        let mut receivers = self.receivers.lock().recover_poison();
        let session_id = match &event {
            SharedSessionEvent::Updated(state)
            | SharedSessionEvent::PaneAdded { state, .. }
            | SharedSessionEvent::PaneRemoved { state, .. }
            | SharedSessionEvent::PanesChanged { state, .. } => state.session_id,
            SharedSessionEvent::StreamFailed { session_id, .. } => *session_id,
        };
        if let Some(subscribers) = receivers.get_mut(&session_id) {
            subscribers.retain(|subscriber| subscriber.try_send(event.clone()).is_ok());
        }
    }
}

/// How long a lost subscription is retried before the panes it served are told
/// that nothing is coming.
///
/// Generous, because the failure it covers is a daemon being replaced: the
/// replacement has to pre-flight a subprocess, exec, re-adopt its sessions and
/// rebind its socket, and a machine under load makes all four slower. Being
/// patient here costs nothing — the terminals keep working throughout, because
/// they read their own descriptors — whereas giving up early is precisely the
/// bug this exists to prevent.
pub(super) const RESUBSCRIBE_GRACE: Duration = Duration::from_secs(60);

/// The first and longest gaps between attempts to find a multiplexer again.
///
/// Backed off rather than flat, because each attempt costs the *daemon* a
/// connection to accept and a thread to serve it — twice, since establishing a
/// client probes for liveness before subscribing. Retrying every few
/// milliseconds for a minute is thousands of those, aimed at a replacement that
/// is starting up and is exactly when it can least afford them; enough of it
/// starves the daemon of threads. The first gap stays short so an upgrade, which
/// is over in well under a second, is not waited on needlessly.
pub(super) const RESUBSCRIBE_FIRST_DELAY: Duration = Duration::from_millis(20);
pub(super) const RESUBSCRIBE_MAX_DELAY: Duration = Duration::from_millis(500);

/// Reads events for as long as any multiplexer is reachable.
///
/// Returns once the grace period has passed with nothing to talk to, at which
/// point the panes really are unreportable and are told so — or as soon as a
/// multiplexer answers that it does not speak this build's protocol, which is
/// unreportable in a different way and must not be confused with it.
pub(super) fn subscription_loop(client: Client, first: Connection, subscription: Subscription) {
    let Subscription {
        exits: reporters,
        revokes,
        grants,
        shared,
        superseded,
    } = subscription;
    let mut connection = first;
    loop {
        let mut announced = false;
        loop {
            match connection.receive::<Event>() {
                Ok((
                    Event::PaneExited {
                        pane_id,
                        raw_status,
                        input_sent,
                        ..
                    },
                    _,
                )) => reporters.report(pane_id, raw_status, input_sent),
                Ok((Event::Revoke { pane_id, .. }, _)) => revokes.report(pane_id),
                Ok((Event::Grant { pane_id, .. }, _)) => grants.report(pane_id),
                // The disconnect that follows is deliberate. Nothing changes in
                // what happens next — the reconnect below is what recovers
                // either way — but knowing it was announced turns a warning
                // into an informational log.
                Ok((Event::Replacing, _)) => announced = true,
                Ok((Event::SharedSessionUpdated { state }, _)) => {
                    shared.report(SharedSessionEvent::Updated(state))
                }
                Ok((
                    Event::SharedPaneAdded {
                        session_id,
                        pane_id,
                        child_pid,
                        state,
                    },
                    _,
                )) => shared.report(SharedSessionEvent::PaneAdded {
                    session_id,
                    pane_id,
                    child_pid,
                    state,
                }),
                Ok((
                    Event::SharedPaneRemoved {
                        session_id,
                        pane_id,
                        state,
                    },
                    _,
                )) => shared.report(SharedSessionEvent::PaneRemoved {
                    session_id,
                    pane_id,
                    state,
                }),
                Ok((
                    Event::SharedPanesChanged {
                        session_id,
                        added,
                        removed,
                        state,
                    },
                    _,
                )) => shared.report(SharedSessionEvent::PanesChanged {
                    session_id,
                    added,
                    removed,
                    state,
                }),
                Ok((
                    Event::SharedStreamFailed {
                        session_id,
                        pane_id,
                    },
                    _,
                )) => shared.report(SharedSessionEvent::StreamFailed {
                    session_id,
                    pane_id,
                }),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if announced {
            log::debug!("the multiplexer is replacing itself; waiting for the replacement");
        } else {
            log::warn!("lost the multiplexer's event stream; trying to re-establish it");
        }
        match resubscribe(&client, &reporters, &revokes, &grants, &shared) {
            Some(Resubscribed::Connection(next)) => connection = next,
            // Nobody is left to tell. Reporting a disconnect here would be
            // writing into registries that no longer have an owner.
            Some(Resubscribed::Abandoned) => return,
            // A multiplexer answered, and refused this build. Every pane keeps
            // its terminal — the descriptors are held here — so the registries
            // are left exactly as they are, and the one thing that has actually
            // been lost is said once, to the window rather than to the panes.
            Some(Resubscribed::Superseded(protocol_version)) => {
                log::error!(
                    "the multiplexer was replaced by one speaking a protocol this build does \
                     not; terminals keep working, but nothing can be started, detached or \
                     reattached until this process restarts"
                );
                superseded.report(protocol_version);
                return;
            }
            None => {
                // Only now, with no multiplexer to ask, is a pane's exit truly
                // unobservable. Saying so is what lets its terminal stop
                // waiting; saying so any earlier reports a live shell as dead.
                log::error!(
                    "no multiplexer answered within {RESUBSCRIBE_GRACE:?}; attached panes can no \
                     longer be told when their processes end"
                );
                reporters.report_all_disconnected();
                return;
            }
        }
    }
}

pub(super) enum Resubscribed {
    Connection(Connection),
    /// The registries' owner is gone, so this subscription has no purpose left.
    Abandoned,
    /// A multiplexer answered and refused this build's protocol, carrying the
    /// version it published if that could be read.
    Superseded(Option<u32>),
}

/// Whether anything outside this thread still holds the registries.
///
/// The thread owns one reference to each; if that is the only one, whoever
/// subscribed has been dropped, nothing can register a pane again, and nothing
/// could observe a report if one arrived. Without this check a subscription whose
/// owner went away kept retrying for the whole grace period — a thread and a
/// steady trickle of connection attempts per closed window, and in a test suite
/// enough of them at once to exhaust the process's threads.
pub(super) fn subscription_is_abandoned(
    reporters: &Arc<ExitReporters>,
    revokes: &Arc<PaneSignals>,
    grants: &Arc<PaneSignals>,
    shared: &Arc<SharedSessionReports>,
) -> bool {
    Arc::strong_count(reporters) == 1
        && Arc::strong_count(revokes) == 1
        && Arc::strong_count(grants) == 1
        && Arc::strong_count(shared) == 1
}

/// Re-establishes the event stream, then reports whatever was missed.
pub(super) fn resubscribe(
    client: &Client,
    reporters: &Arc<ExitReporters>,
    revokes: &Arc<PaneSignals>,
    grants: &Arc<PaneSignals>,
    shared: &Arc<SharedSessionReports>,
) -> Option<Resubscribed> {
    let deadline = Instant::now() + RESUBSCRIBE_GRACE;
    let mut delay = RESUBSCRIBE_FIRST_DELAY;
    loop {
        if subscription_is_abandoned(reporters, revokes, grants, shared) {
            return Some(Resubscribed::Abandoned);
        }
        match client.open_subscription() {
            Ok(connection) => {
                log::info!("re-established the multiplexer's event stream");
                // Subscribe first, then reconcile: an exit that happens between
                // the two arrives on the new subscription, whereas one reported
                // between a reconcile and a subscribe would fall down the same
                // gap this is closing.
                reconcile_missed_exits(client, reporters);
                reconcile_shared_snapshots(client, shared);
                return Some(Resubscribed::Connection(connection));
            }
            // Answered, and refused: waiting out the deadline would achieve
            // nothing but spend it, and what it spends it on is the panes.
            Err(error) if is_protocol_mismatch_error(&error) => {
                return Some(Resubscribed::Superseded(
                    client.published_protocol_version(),
                ));
            }
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(delay);
        delay = (delay * 2).min(RESUBSCRIBE_MAX_DELAY);
    }
}

/// Asks about every pane still waiting on a report, and delivers the exits that
/// happened while there was nobody listening.
pub(super) fn reconcile_missed_exits(client: &Client, reporters: &Arc<ExitReporters>) {
    let pane_ids = reporters.registered();
    if pane_ids.is_empty() {
        return;
    }
    let reports = match client.pane_states(pane_ids) {
        Ok(reports) => reports,
        Err(error) => {
            log::warn!("could not ask the multiplexer what it missed: {error:#}");
            return;
        }
    };
    for report in reports {
        if report.exited || report.unknown {
            reporters.report(report.pane_id, report.raw_status, report.input_sent);
        }
    }
}

pub(super) fn reconcile_shared_snapshots(client: &Client, shared: &Arc<SharedSessionReports>) {
    for session_id in shared.registered() {
        match client.shared_snapshot(session_id) {
            Ok(state) => shared.report(SharedSessionEvent::Updated(state)),
            Err(error) => log::debug!(
                "could not reconcile shared session {session_id} after reconnect: {error:#}"
            ),
        }
    }
}

impl Client {
    /// Starts delivering pane exits and revokes to registered reporters.
    ///
    /// Both registries belong to the one subscription: the same thread that
    /// reports an exit reports a revoke, so they are returned together and
    /// live as long as the subscription does.
    ///
    /// The subscription outlives the *connection* carrying it. That distinction
    /// is the whole point: a client holding a pane's descriptor keeps a working
    /// terminal across a daemon replacement, and the one thing that would ruin
    /// it is treating the lost connection as the pane's process ending.
    pub fn subscribe(&self) -> Result<Subscription> {
        let connection = self.open_subscription()?;
        let exits = Arc::new(ExitReporters::default());
        let revokes = Arc::new(PaneSignals::default());
        let grants = Arc::new(PaneSignals::default());
        let shared = Arc::new(SharedSessionReports::default());
        let superseded = Arc::new(SupersededMultiplexer::default());
        let subscription = Subscription {
            exits,
            revokes,
            grants,
            shared,
            superseded,
        };
        let dispatch = subscription.clone();
        let client = self.reconnect_client();
        thread::spawn(move || {
            subscription_loop(client, connection, dispatch);
        });
        Ok(subscription)
    }

    /// Opens the event stream's connection, and does not return until the
    /// multiplexer has said the subscription is registered.
    ///
    /// The wait is the point. Subscribing used to be one-way, so a caller that
    /// subscribed and then acted — attached a pane, shared a session — could
    /// have the event that followed broadcast before its handle was in the
    /// multiplexer's table, and the event went nowhere. The window is short,
    /// which is why it was reached only on a loaded machine, and why what it
    /// produced was an event that simply never arrived.
    ///
    /// One function because the reconnect path needs both halves of this too,
    /// and had drifted from the first: it left the request read deadline on a
    /// connection that is then read for the lifetime of the subscription, so
    /// every quiet fifteen seconds looked like a lost multiplexer and cost
    /// another reconnect and two reconciling round trips.
    pub(super) fn open_subscription(&self) -> Result<Connection> {
        let mut connection = self.open_ready(Request::Subscribe)?;
        self.expect_ok(&mut connection, "subscribe")?;
        // Subscription connections are intentionally long-lived and may be
        // idle for hours. Keep the write deadline from `open_as`, but remove
        // the request read deadline before handing the connection to the
        // event loop.
        let timeout_cleared = connection.stream().set_read_timeout(None);
        if matches!(
            &timeout_cleared,
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput
        ) {
            // Darwin refuses every SO_RCVTIMEO update with EINVAL after the
            // peer has closed an AF_UNIX socket. The next read will observe
            // EOF immediately, so the leftover deadline cannot affect this
            // connection and the subscription loop still needs to receive it
            // in order to run its reconnect path.
            return Ok(connection);
        }
        timeout_cleared.context("clearing the multiplexer subscription read timeout")?;
        Ok(connection)
    }
}
