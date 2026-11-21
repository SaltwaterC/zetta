//! Catalog subscribers: the bounded relay that writes events to each one off
//! the daemon's lock, and broadcasting an event to all, all but one, or one.

use super::*;

pub(super) struct Subscriber {
    pub(super) process_id: u32,
    pub(super) relay: Arc<SubscriberRelay>,
}

/// The daemon never writes directly to a subscriber: an SSH forward can stop
/// accepting bytes for minutes while Wi-Fi recovers.  Keeping that write on its
/// own thread means the control plane continues to make progress, while the
/// bounded queue still disconnects a subscriber that is permanently stuck.
pub(super) struct SubscriberRelay {
    pub(super) id: u64,
    pub(super) client_id: ClientId,
    pub(super) sender: SyncSender<Event>,
    pub(super) closed: Arc<AtomicBool>,
    pub(super) receiver: Arc<Mutex<Option<mpsc::Receiver<Event>>>>,
    pub(super) connection: Arc<Mutex<Option<Connection>>>,
}

pub(super) const SUBSCRIBER_RELAY_QUEUE_CAPACITY: usize = 16;
pub(super) const SUBSCRIBER_RELAY_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

impl SubscriberRelay {
    pub(super) fn new(
        daemon: &Arc<Daemon>,
        client_id: ClientId,
        connection: Connection,
    ) -> Arc<Self> {
        let id = daemon
            .next_subscriber_relay_id
            .fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_RELAY_QUEUE_CAPACITY);
        let receiver = Arc::new(Mutex::new(Some(receiver)));
        let connection = Arc::new(Mutex::new(Some(connection)));
        let closed = Arc::new(AtomicBool::new(false));
        Arc::new(Self {
            id,
            client_id: client_id.clone(),
            sender,
            closed: Arc::clone(&closed),
            receiver,
            connection,
        })
    }

    /// Starts delivery only after the relay has entered `Daemon::subscribers`.
    /// A client can attach immediately after sending Subscribe, so publishing
    /// the handle first ensures its first revoke is queued rather than missed.
    pub(super) fn start(&self, daemon: &Arc<Daemon>) {
        let daemon = Arc::downgrade(daemon);
        let client_id = self.client_id.clone();
        let id = self.id;
        let closed = Arc::clone(&self.closed);
        let receiver = Arc::clone(&self.receiver);
        let connection = Arc::clone(&self.connection);
        spawn_worker("zmux subscription relay", move || {
            let daemon = daemon.clone();
            let client_id = client_id.clone();
            let closed = Arc::clone(&closed);
            let receiver = Arc::clone(&receiver);
            let connection = Arc::clone(&connection);
            Box::new(move || {
                let receiver = receiver
                    .lock()
                    .recover_poison()
                    .take()
                    .expect("the subscription relay receiver was already claimed");
                let connection = connection
                    .lock()
                    .recover_poison()
                    .take()
                    .expect("the subscription relay connection was already claimed");
                run_subscriber_relay(daemon, client_id, id, closed, connection, receiver);
            })
        });
    }

    /// Tells the client that its subscription is live, on the connection the
    /// relay has taken over but not yet started delivering on.
    ///
    /// Sent from here rather than before the relay is built because *when* it
    /// goes out is the whole point: a client that has this reply is one whose
    /// handle is already in [`Daemon::subscribers`], so anything broadcast from
    /// the moment it acts is queued for it. Sending it any earlier would leave
    /// the gap it closes — small, and therefore reached only under load, which
    /// is where a missed first event is hardest to explain.
    pub(super) fn acknowledge(&self) -> Result<()> {
        let mut connection = self.connection.lock().recover_poison();
        connection
            .as_mut()
            .context("the subscription connection was already claimed")?
            .send(&Response::Ok)
    }

    /// Queues an event without waiting for the remote socket.  A full queue is
    /// a permanently stalled subscription for the daemon's purposes.
    pub(super) fn enqueue(&self, event: Event) -> bool {
        if self.closed.load(Ordering::Acquire) {
            return false;
        }
        match self.sender.try_send(event) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.closed.store(true, Ordering::Release);
                false
            }
        }
    }
}

pub(super) fn run_subscriber_relay(
    daemon: std::sync::Weak<Daemon>,
    client_id: ClientId,
    relay_id: u64,
    closed: Arc<AtomicBool>,
    mut connection: Connection,
    receiver: mpsc::Receiver<Event>,
) {
    if let Err(error) = connection.set_write_timeout(Some(SUBSCRIBER_RELAY_WRITE_TIMEOUT)) {
        log::debug!("could not set subscription write timeout: {error:#}");
        closed.store(true, Ordering::Release);
        if let Some(daemon) = daemon.upgrade() {
            remove_subscriber_if_matches(&daemon, &client_id, relay_id);
        }
        return;
    }
    while let Ok(event) = receiver.recv() {
        if closed.load(Ordering::Acquire) {
            break;
        }
        if let Err(error) = connection.send(&event) {
            log::debug!(
                "subscription delivery to client {} failed: {error:#}",
                client_id.as_str()
            );
            closed.store(true, Ordering::Release);
            if let Some(daemon) = daemon.upgrade() {
                remove_subscriber_if_matches(&daemon, &client_id, relay_id);
            }
            break;
        }
    }
}
impl Daemon {
    /// The session list. `unwrap`, not recovery: a session's panes, their
    /// attachments and holders, and what the catalog says about it are changed
    /// together under this lock, so a panic part-way through leaves them
    /// disagreeing — and serving that would hand clients panes that no longer
    /// match what the daemon believes they hold.
    pub(super) fn sessions(&self) -> MutexGuard<'_, Vec<Session>> {
        self.sessions.lock().unwrap()
    }

    pub(super) fn new(
        directory: &Path,
        retention: Retention,
        image_directory: PathBuf,
        #[cfg(feature = "session-persistence")] persistence: Option<PersistenceStore>,
        next_session_id: u64,
        generation: u64,
        #[cfg(all(unix, not(target_os = "macos")))] listener_fd: RawFd,
        #[cfg(windows)] pty_host: crate::pty_host::HostClient,
    ) -> Self {
        #[cfg(feature = "session-persistence")]
        let persistence_enabled = matches!(retention, Retention::Disk) && persistence.is_some();
        Self {
            sessions: Mutex::new(Vec::new()),
            sessions_condvar: Condvar::new(),
            verification_gate: VerificationGate::default(),
            next_session_id: AtomicU64::new(next_session_id),
            next_pane_id: AtomicU64::new(1),
            subscribers: Mutex::new(HashMap::new()),
            next_subscriber_relay_id: AtomicU64::new(1),
            catalog: Mutex::new(SessionCatalogPublisher::with_generation(
                directory, generation,
            )),
            retention: Mutex::new(retention),
            running: AtomicBool::new(true),
            image_directory,
            #[cfg(feature = "session-persistence")]
            directory: directory.to_owned(),
            executable: resolve_own_executable(),
            #[cfg(windows)]
            pty_host,
            #[cfg(all(unix, not(target_os = "macos")))]
            listener_fd,
            drain_wake: Mutex::new(None),
            #[cfg(feature = "session-persistence")]
            persistence: Arc::new(PersistenceQueue::new(persistence, persistence_enabled)),
            #[cfg(feature = "session-persistence")]
            restored: Mutex::new(Vec::new()),
            recipient_grants: Mutex::new(RecipientGrants::default()),
            connections: Arc::new(AtomicUsize::new(0)),
            snapshot_budget: SnapshotBudget::default(),
            lifetime_lock: Mutex::new(None),
            #[cfg(unix)]
            spawn_gate: RwLock::new(()),
            handed_over: AtomicBool::new(false),
        }
    }
}

pub(super) fn broadcast(daemon: &Arc<Daemon>, event: &Event) {
    let relays = daemon
        .subscribers
        .lock()
        .recover_poison()
        .values()
        .map(|subscriber| Arc::clone(&subscriber.relay))
        .collect::<Vec<_>>();
    for relay in relays {
        enqueue_subscriber_event(daemon, &relay, event);
    }
}

/// Broadcasts an event to every subscribed client except the one whose request
/// already carries the result. Excluding the requester is important for shared
/// pane creation: its request connection is already the data plane, so handling
/// the pane-added event as well would attach a duplicate local pane.
pub(crate) fn broadcast_except(daemon: &Arc<Daemon>, event: &Event, excluded: &ClientId) {
    let relays = daemon
        .subscribers
        .lock()
        .recover_poison()
        .iter()
        .filter(|(client_id, _)| *client_id != excluded)
        .map(|(_, subscriber)| Arc::clone(&subscriber.relay))
        .collect::<Vec<_>>();
    for relay in relays {
        enqueue_subscriber_event(daemon, &relay, event);
    }
}

/// Sends a shared-stream failure only to the viewer whose data connection
/// failed. Other viewers keep their relays and must not reconnect or lose their
/// byte ordering merely because one socket disappeared.
pub(crate) fn broadcast_to(daemon: &Arc<Daemon>, event: &Event, target: &ClientId) {
    let relay = daemon
        .subscribers
        .lock()
        .recover_poison()
        .get(target)
        .map(|subscriber| Arc::clone(&subscriber.relay));
    if let Some(relay) = relay {
        enqueue_subscriber_event(daemon, &relay, event);
    }
}

/// Delivers one event without allowing a subscriber's socket to block the
/// caller. The identity check prevents an old writer's failure from removing a
/// replacement subscription for the same logical client.
pub(super) fn enqueue_subscriber_event(
    daemon: &Arc<Daemon>,
    relay: &Arc<SubscriberRelay>,
    event: &Event,
) {
    if !relay.enqueue(event.clone()) {
        remove_subscriber_if_matches(daemon, &relay.client_id, relay.id);
    }
}

pub(super) fn remove_subscriber_if_matches(daemon: &Daemon, client_id: &ClientId, relay_id: u64) {
    let mut subscribers = daemon.subscribers.lock().recover_poison();
    if subscribers
        .get(client_id)
        .is_some_and(|subscriber| subscriber.relay.id == relay_id)
    {
        subscribers.remove(client_id);
    }
}
