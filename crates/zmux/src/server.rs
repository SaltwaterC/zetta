//! The multiplexer daemon.
//!
//! It owns every pane's PTY. While exactly one client is attached to a pane,
//! the daemon hands over the master descriptor and stops reading it, so an
//! attached terminal is precisely as fast as one the application spawned
//! itself. When the client lets go, the daemon resumes reading and retains
//! what it reads according to the configured retention.
//!
//! A second client attaching to a pane flips it to *shared* mode: the holder
//! is told to hand the terminal back ([`Event::Revoke`]), the daemon resumes
//! reading and relays output to every shared client, and each client's input
//! and size go through the daemon. A pane stays shared until its last client
//! leaves; only then does an exclusive attach become possible again.

use std::{
    collections::{HashMap, HashSet},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};
use zetta_sync::RecoverPoison as _;

#[cfg(windows)]
use alacritty_terminal::tty::{ChildEvent, EventedPty as _};
use alacritty_terminal::{
    event::WindowSize,
    tty::{self, ConsolePalette},
};
use anyhow::{Context as _, Result};

#[cfg(all(unix, not(target_os = "macos")))]
use std::os::fd::BorrowedFd;
#[cfg(unix)]
use std::os::fd::{AsFd as _, AsRawFd as _, RawFd};
#[cfg(unix)]
use std::sync::RwLock;

use crate::{
    auth::SessionAuthentication,
    catalog::{SessionCatalogPublisher, create_private_dir},
    messages::{
        ClientId, Envelope, Event, PROTOCOL_VERSION, Request, Response, SharedOperationId,
        SharedSessionOperation, SpawnRequest, TerminalSize,
    },
    paths::session_catalog_dir,
    private_fs::remove_stale_socket,
    process_status::ProcessIdentity,
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
    retention::Retention,
    transport::{Connection, Endpoint, Listener, PeerProcess, Stream, random_hex, token_matches},
};

#[cfg(feature = "session-persistence")]
use crate::persistence::{PersistedSession, PersistedSnapshot, PersistenceStore};

/// One pane's terminal, and what has been retained from it.
struct Pane {
    id: u64,
    pty: tty::Pty,
    #[cfg(windows)]
    /// The console remains owned by the separate pseudoconsole host.
    console_id: u64,
    #[cfg(windows)]
    /// The daemon's PTY reports child exits through this external watcher.
    child_events: tty::AttachedChildEvents,
    /// Who holds this pane's terminal, and what that means for reading it.
    ///
    /// The daemon must not read a pane an exclusive client is reading: a PTY
    /// master has one meaningful reader, and two would split the output
    /// between them. Recording *which* client, rather than just that there is
    /// one, is what lets a pane be reclaimed when that client dies without
    /// detaching — otherwise the pane stays unread forever and its program
    /// blocks as soon as the terminal's buffer fills. The same rule applies
    /// while a revoke is outstanding: the holder still has the descriptor
    /// until its snapshot arrives.
    attachment: Attachment,
    /// The logical request that acquired the current exclusive attachment.
    ///
    /// A process can reattach a pane while an older handover cleanup is still
    /// in flight. Its PID is therefore not an attachment identity: accepting
    /// that stale cleanup makes the daemon read a PTY the newer attachment is
    /// already reading, splitting terminal escape sequences between them.
    attachment_client_id: Option<ClientId>,
    /// The size the daemon last applied. Shared clients report their own sizes
    /// over [`Request::Resize`], which is what size arbitration starts from.
    size: TerminalSize,
    retained: crate::retention::Retained,
    /// The client that handed this pane over to sharing, and how far into the
    /// retained stream the screen it sent reaches.
    ///
    /// That client is still *showing* that screen — it stopped reading the
    /// terminal, it did not clear its grid — so replaying it would draw it a
    /// second time, wherever the program happens to have left the cursor. Only
    /// what the daemon read after the handover is new to it. Cleared by the
    /// first shared attach, which is that client's.
    handed_over: Option<RevokeHandover>,
    /// Attach requests waiting for the revoke that produced `handed_over`.
    /// Keep an empty shared attachment alive until these waiters have had a
    /// chance to join; otherwise a holder that briefly re-attaches and closes
    /// can collapse the handover to exclusive before the waiting client wakes.
    handover_waiters: usize,
    exited: bool,
    /// The raw status observed when the process ended, kept so a client that
    /// missed the broadcast can still be told the *real* exit rather than
    /// "status unavailable". Only the reaper can observe it, and it observes it
    /// exactly once, so nothing else can reconstruct it later.
    exit_status: Option<i32>,
    /// Input from shared clients that the terminal could not take yet.
    ///
    /// The master is non-blocking, so a full terminal buffer makes a write
    /// return `WouldBlock` after a partial write. Dropping the remainder loses a
    /// shared client's keystrokes silently, which is why it is queued here for
    /// the drain thread to finish.
    pending_input: Vec<u8>,
    clipboard_scanner: zclip::protocol::Scanner,
}

/// A revoke handover that has not been joined yet.
struct RevokeHandover {
    client_process_id: u32,
    /// What the pane has printed since that client handed its screen over.
    ///
    /// The one thing a screen cannot answer. Everybody else joining a pane is
    /// sent the screen; this client is *already showing* it, so sending it again
    /// would draw it a second time over whatever the program has drawn since —
    /// wherever the cursor happens to be. What it needs is the difference, which
    /// exists only as the bytes that arrived while it was rejoining.
    ///
    /// A handover takes milliseconds, so this is short-lived and small; the cap
    /// is there because "short-lived" is not something the daemon can insist on.
    output: Vec<u8>,
}

/// How much a handing-over client's missed output may accumulate before the
/// oldest of it is dropped. Reached only if that client never comes back to join,
/// by which point the pane's own screen is what the next attach shows anyway.
const HANDOVER_OUTPUT_LIMIT: usize = 1024 * 1024;

/// What a pane's terminal currently is, and who holds it.
enum Attachment {
    /// No client holds the terminal; the drain loop reads it.
    None,
    /// One client holds the descriptor and reads it directly.
    Exclusive(u32),
    /// A revoke was sent and the holder's snapshot has not arrived yet.
    /// Nothing may read the pane: the holder still has the descriptor, and
    /// two readers would split the output between them.
    Revoking { holder: u32 },
    /// Shared mode: the daemon reads the pane and relays output to every
    /// shared client, whose input and size come back through the daemon.
    Shared(Vec<SharedClient>),
    /// A grant was accepted and the descriptor has not gone out yet. Nothing may
    /// read the pane, and nothing more may be queued to the client: what is still
    /// queued has to reach it *before* it starts reading the terminal itself, or
    /// the two sources arrive out of order.
    Granting { holder: u32 },
}

/// One client attached in shared mode.
struct SharedClient {
    process_id: u32,
    /// The process the transport vouched for when this client attached, which
    /// is what a later request has to match to act as this viewer — taking the
    /// pane exclusively above all. `process_id` is only what the client said.
    /// `None` where nothing vouched for it: an unattested Windows peer, or a
    /// viewer carried across an upgrade, which re-attaches before it matters.
    peer_process_id: Option<u32>,
    client_id: ClientId,
    /// This one attachment, among any others the same client has. A client
    /// that recovers a failed stream attaches again under the same
    /// [`ClientId`] before its old stream has necessarily been torn down, so
    /// ending a stream must remove the attachment that stream served — by
    /// client ID, it also removed the replacement.
    attachment: u64,
    stream_only: bool,
    /// The client this one is relaying the pane to, when it is a relay rather
    /// than the viewer itself. Set by `zmux relay-pane`, which reads a pane on
    /// this host and copies it onto a Mosh link; the window at the far end of
    /// that link is the viewer named here and is in no pane's shared set.
    ///
    /// Its whole purpose is to let that window's own control requests — image
    /// paste — be recognized as coming from somebody looking at the pane. It
    /// lives and dies with this attachment, so a window whose relay has gone
    /// is no longer treated as present.
    relaying_for: Option<ClientId>,
    /// Frames queued for this client, written by its own relay thread.
    ///
    /// Deliberately not the connection itself. Writing to a viewer's socket from
    /// here meant doing it while holding [`Daemon::sessions`], so a viewer that
    /// stopped reading stalled *everything*: its socket buffer filled, the write
    /// blocked for up to [`RELAY_WRITE_TIMEOUT`], and every other pane's drain,
    /// every shared client's input and every attach queued up behind it.
    ///
    /// Bounded, because the alternative to blocking is not "buffer for ever": a
    /// viewer that falls this far behind is wedged, and dropping it from the
    /// relay is better than growing the daemon until it is killed.
    relay: Relay,
    /// How much this viewer's relay had written when it was last looked at, and
    /// since when output has waited for it without any being written — reset by
    /// every write and by an empty queue, so an idle viewer is never behind. A
    /// viewer that is merely slow keeps writing; one that has stopped reading
    /// stops writing, and only that one is dropped. See
    /// `workers::viewer_has_stalled`.
    written_seen: usize,
    wrote_at: Instant,
    /// A client does not constrain a shared pane until its first initialized
    /// layout report arrives. The response's dimensions are advisory only:
    /// they tell the client what the pane is currently running at, not what
    /// this unmeasured client is showing.
    size: Option<(u16, u16)>,
    input_sent: bool,
}

impl Attachment {
    /// Whether no client holds this pane's terminal.
    fn is_none(&self) -> bool {
        matches!(self, Attachment::None)
    }
}

impl Session {
    /// Whether the user can pick this session up right now, which is also
    /// whether it should appear in the catalog.
    ///
    /// Two ways to ask for that, and they are not the same request. Detaching
    /// sets `keep`: the session is to outlive its window, so of course it can be
    /// picked up. Sharing sets `offered`: the session stays exactly where it is,
    /// on screen, and is merely made joinable. Requiring `keep` for both meant
    /// the only route to a shared session was to dismiss it first and take it
    /// straight back.
    ///
    /// A pane whose process has ended has nothing left to attach to, so a
    /// session needs at least one live one either way.
    ///
    /// A session a window is *currently showing* is not something to pick up: it
    /// is on screen. `keep` is sticky, so reattaching a session leaves it set —
    /// deliberately, because a window that then crashes must still find the
    /// session — and listing it on that basis put a tab the user was looking at
    /// in their own reconnect picker, with the button beside it permanently lit.
    /// Offering it is the one reason to list a held session: then another window
    /// may join it, which is what the "in use" marker says.
    fn is_available(&self) -> bool {
        (self.keep || self.offered)
            && self.panes.iter().any(|pane| !pane.exited)
            && (self.offered || !self.is_held())
    }

    /// Whether a window is showing this session right now.
    ///
    /// The same question [`catalog_summary`] answers with `held`, so the two
    /// cannot disagree about what "in use" means.
    fn is_held(&self) -> bool {
        self.panes
            .iter()
            .any(|pane| matches!(pane.attachment, Attachment::Exclusive(_)))
    }

    /// Whether `client` may see and attach this session.
    ///
    /// Availability says the session has something left to attach to; this says
    /// whose it is. An offered session is everyone's, and one nobody has held yet
    /// is nobody's in particular. Being owned by a process that has since exited
    /// is *not* the same as being unowned: that session stays out of every other
    /// window's reach until somebody shares it.
    ///
    /// `client` is the verified peer, not the envelope's claim — see
    /// `Attacher::verified_process_id`. A peer nobody could vouch for is in
    /// scope only where everyone is.
    fn is_in_scope_for(&self, client: Option<u32>) -> bool {
        self.offered
            || self
                .owner
                .is_none_or(|owner| client.is_some_and(|client| owner.is(client)))
    }
}

struct Session {
    id: u64,
    summary: BackgroundSessionSummary,
    state: serde_json::Value,
    /// Canonical collaboration state when this session is offered for live
    /// multi-client editing. Kept separate from the application's opaque tab
    /// state so a revision and operation id survive without changing that
    /// payload's schema.
    shared_state: Option<crate::messages::SharedSessionState>,
    authentication: Option<SessionAuthentication>,
    /// The sealed session key that goes with `authentication`, when the secret
    /// was generated rather than typed. Held beside the verifier and published
    /// with the summary, never read here: opening it needs an age identity, and
    /// the daemon deliberately has none.
    key_envelope: Option<String>,
    failed_authentications: u32,
    refuse_until: Option<std::time::Instant>,
    panes: Vec<Pane>,
    /// Whether the user asked for this session to outlive the window that
    /// created it, by detaching it.
    ///
    /// Sticky: attaching shows a session again but does not withdraw the
    /// request to keep it. Conflating the two meant a session that was
    /// detached and then attached looked exactly like one nobody had ever
    /// asked to keep, and was destroyed when its window died.
    keep: bool,
    /// Whether the user asked for this session to be joinable while a window is
    /// still showing it.
    ///
    /// Independent of `keep` as a property: offering a session does not by
    /// itself make it outlive its windows, while a kept session is attachable
    /// whether or not it was ever offered. A caller may request both, which is
    /// what Zetta's keep-running action does by default.
    offered: bool,
    /// The process this session belongs to while it is not offered.
    ///
    /// Backgrounding a tab is private unless it was already offered. Before the
    /// multiplexer held these sessions they lived in the process that made them
    /// and no other Zetta could see them, and that is still what plain
    /// `Ctrl-Shift-D` means; `offered` is what makes a session everyone's.
    /// Recorded rather than inferred from
    /// the panes' attachments, because a detached session has none.
    ///
    /// Kept when that process goes away, rather than released: a session
    /// backgrounded in one window must not become another window's because the
    /// first one exited, which is the whole difference between backgrounding and
    /// sharing. Widening a private session stays an explicit request — `Share`
    /// or `SetSessionScope` — so a session whose window is gone is still listed,
    /// killable and shareable, but not silently attachable.
    ///
    /// `None` only for a session nobody has held yet. An identity rather than a
    /// PID, because it outlives the process it names: see
    /// [`ProcessIdentity`].
    owner: Option<ProcessIdentity>,
    /// The recipients this session's disk records are encrypted to while it is
    /// protected, chosen by its own clients rather than whoever configured the
    /// store last. `None` withholds a protected session from disk; see
    /// [`sealing`].
    sealed_to: Option<Vec<String>>,
}

#[cfg(feature = "session-persistence")]
struct RestoredPane {
    bytes: Vec<u8>,
}

#[cfg(feature = "session-persistence")]
struct RestoredSession {
    request: crate::messages::ResumeRequest,
    restored_at: u64,
    /// The peer identity that completed Resume. A protected lease must be
    /// consumed by that same verified peer, not merely by a process naming the
    /// same client id in its envelope.
    authorized_peer: Option<u32>,
    snapshots: Vec<RestoredPane>,
    /// Number of successful base-pane handoffs made from this lease. Keep the
    /// lease while later panes are still being materialized so they can consume
    /// their saved screen and, for protected records, the same peer check.
    spawned_panes: usize,
}

pub struct Daemon {
    sessions: Mutex<Vec<Session>>,
    /// Wakes an attach waiting for a revoke handover to complete.
    ///
    /// Paired with [`Daemon::sessions`]: the snapshot handler changes the
    /// pane's attachment under that lock and notifies here, and the attach
    /// waits on it while the lock is released.
    sessions_condvar: Condvar,
    /// Admits session-secret checks, which run with [`Daemon::sessions`]
    /// released — see [`secret_check`].
    verification_gate: VerificationGate,
    next_session_id: AtomicU64,
    next_pane_id: AtomicU64,
    /// The clients' long-lived event connections, keyed by client process.
    ///
    /// Keyed so a revoke can reach the one client holding a pane, instead of
    /// being broadcast to every subscriber. A client reconnecting subscribes
    /// again, replacing its earlier entry.
    subscribers: Mutex<HashMap<ClientId, Subscriber>>,
    next_subscriber_relay_id: AtomicU64,
    catalog: Mutex<SessionCatalogPublisher>,
    retention: Mutex<Retention>,
    running: AtomicBool,
    /// Private per-session storage for normalized clipboard images.
    image_directory: PathBuf,
    #[cfg(feature = "session-persistence")]
    /// The private directory this daemon serves.  Keeping it here lets a
    /// memory-fallback daemon recover or discard records left by an earlier
    /// disk daemon without consulting process-global configuration again.
    directory: PathBuf,
    /// This multiplexer's own executable, resolved once at startup.
    ///
    /// Resolving it at upgrade time is wrong: on Linux `/proc/self/exe` reads
    /// as `"<path> (deleted)"` once the file has been replaced, which is
    /// exactly what rebuilding does — so the upgrade would try to execute a
    /// path that cannot exist.
    executable: Option<PathBuf>,
    #[cfg(windows)]
    /// The process that owns every Windows pseudoconsole across daemon
    /// replacements.
    pty_host: crate::pty_host::HostClient,
    #[cfg(all(unix, not(target_os = "macos")))]
    /// The listener is inherited during an upgrade where the platform preserves
    /// local-socket peer credentials across `execv`; macOS rebinds it instead.
    listener_fd: RawFd,
    /// Wakes the drain thread when a pane is attached or detached.
    drain_wake: Mutex<Option<Stream>>,
    #[cfg(feature = "session-persistence")]
    persistence: Arc<PersistenceQueue>,
    #[cfg(feature = "session-persistence")]
    restored: Mutex<Vec<RestoredSession>>,
    recipient_grants: Mutex<RecipientGrants>,
    /// Connections being served, against [`ipc_limits::MAX_CONNECTIONS`].
    connections: Arc<AtomicUsize>,
    /// Raw snapshot bytes being read, across every request.
    snapshot_budget: SnapshotBudget,
    /// What makes this the directory's only daemon; see `lifetime_lock`.
    /// `None` only for a replacement that could neither take over nor take
    /// the lock, which carries on rather than drop its sessions.
    lifetime_lock: Mutex<Option<LifetimeLock>>,
    /// Held for reading while a pane's process starts, and for writing while
    /// an upgrade has the descriptors it carries inheritable; see
    /// `exec_carrying`.
    #[cfg(unix)]
    spawn_gate: RwLock<()>,
    /// Set once a Windows replacement has taken over the socket and endpoint.
    handed_over: AtomicBool,
}

/// Reads one request and checks that it may be served at all.
///
/// Every refusal answers before it returns. A connection that simply closes
/// tells the client nothing, and the client has to decide whether to fall back
/// — which it cannot do sensibly without a reason.
fn read_request(connection: &mut Connection, token: &str, deadline: Instant) -> Result<Envelope> {
    let envelope: Envelope = match connection.receive_by(deadline) {
        Ok((envelope, _)) => envelope,
        Err(error) => {
            let _ = connection.send(&Response::Error {
                message: format!("unreadable request: {error:#}"),
            });
            return Err(error);
        }
    };
    if !token_matches(&envelope.token, token) {
        connection.send(&Response::Error {
            message: "invalid multiplexer token".to_owned(),
        })?;
        anyhow::bail!("rejected a connection presenting the wrong token");
    }
    // Every request but one has to agree on the protocol, and the exception is
    // not optional: `Request::Upgrade` is the mechanism by which a version
    // boundary is crossed, so refusing it for disagreeing about the version made
    // every protocol bump a choice between running the newly built client and
    // keeping the sessions the old daemon is holding. Rebuilding leaves exactly
    // that situation — a new client, an old daemon — and it is the one moment
    // `--upgrade` exists for.
    //
    // Nothing is relaxed by allowing it. The endpoint token has already been
    // checked; the image is the one this daemon resolved at startup and never one
    // a client names; and the request carries no payload, so there is no shape
    // for the two sides to disagree about. Its only answers are `Response::Ok`
    // and `Response::Error`, whose shapes are fixed.
    if envelope.version != PROTOCOL_VERSION && !matches!(envelope.request, Request::Upgrade) {
        connection.send(&Response::Error {
            message: crate::messages::protocol_mismatch_message(PROTOCOL_VERSION, envelope.version),
        })?;
        anyhow::bail!("refused a client speaking protocol {}", envelope.version);
    }
    Ok(envelope)
}

/// A session as it is offered, which is not quite as its client described it.
///
/// `held` is the daemon's to decide, not the client's: it says a window is
/// exclusively showing the session right now, so attaching means a revoke
/// handover rather than an ordinary reconnect. Computed in one place because
/// both the published catalog and [`Request::List`] answer for it — `List`
/// reported the client's stored value, which is always `false`, so anything
/// asking directly was told a live session was free.
fn catalog_summary(session: &Session) -> BackgroundSessionSummary {
    let previous_state = session.shared_state.clone().unwrap_or_else(|| {
        crate::messages::SharedSessionState::new(
            session.id,
            session.summary.clone(),
            session.state.clone(),
        )
    });
    let mut summary = summary_from_daemon_panes(session, &previous_state).unwrap_or_else(|error| {
        log::warn!(
            "could not reconcile session {} with its live panes: {error:#}",
            session.id
        );
        session.summary.clone()
    });
    summary.held = session.is_held();
    // Taken from the session rather than trusted from the stored summary, so it
    // follows the verifier: a share or scope request that republishes a summary
    // without it does not silently drop the way in to a protected session.
    summary.key_envelope = session.key_envelope.clone();
    // Whose it is, so a process reading the one shared catalog can tell its own
    // sessions from another window's. An offered session is nobody's in
    // particular and carries no scope at all.
    summary.scoped_to = (!session.offered)
        .then_some(session.owner)
        .flatten()
        .map(|owner| owner.pid);
    summary
}

fn publish(daemon: &Arc<Daemon>) {
    let sessions = daemon
        .sessions()
        .iter()
        .filter(|session| session.is_available())
        .map(catalog_summary)
        .collect::<Vec<_>>();
    let mut catalog = daemon.catalog.lock().recover_poison();
    if let Err(error) = catalog.publish_sessions(sessions) {
        log::warn!("could not publish the session catalog: {error:#}");
    }
}

#[cfg(all(test, unix, not(target_os = "macos")))]
#[path = "tests/server/subscriber_relay.rs"]
mod subscriber_relay_tests;

fn window_size(size: TerminalSize) -> WindowSize {
    WindowSize {
        num_lines: size.lines.max(1),
        num_cols: size.columns.max(1),
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

/// Spawns a daemon worker, retrying while the system is momentarily out of
/// thread capacity.
///
/// A fresh process starting up — a replacement after an upgrade most often —
/// can hit a transient `EAGAIN` when the host is under load, and dying there
/// would discard exactly the sessions the upgrade was meant to preserve.
/// Nothing else can be done with a spawn that failed this way, so the retry
/// only makes the daemon wait out the shortage. The factory builds a fresh
/// worker for every attempt, because a failed `Builder::spawn` consumes the
/// closure it was given.
fn spawn_worker(name: &str, make_worker: impl Fn() -> Box<dyn FnOnce() + Send>) {
    loop {
        match std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(make_worker())
        {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                log::warn!("spawning the {name} thread was momentarily refused; retrying");
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(error) => panic!("spawning the {name} thread failed: {error}"),
        }
    }
}

/// When a connection was accepted, and its claim on one of the daemon's
/// connection places, which it holds until its thread ends.
struct Accepted {
    accepted: Instant,
    permit: ConnectionPermit,
}

/// Serves one accepted connection, waiting out a transient shortage of threads.
///
/// `thread::Builder::spawn` consumes its closure even when the OS refuses to
/// create the thread, so the stream lives in a slot shared by each retry. An
/// accepted request must not be discarded just because a parallel workload
/// briefly exhausted the process's thread capacity: the peer cannot tell that
/// apart from a daemon that died, and idempotent requests are not all retried by
/// their callers.
fn spawn_connection(daemon: Arc<Daemon>, stream: Stream, token: String, accepted: Accepted) {
    let stream = Arc::new(Mutex::new(Some((stream, accepted))));
    loop {
        let stream_slot = Arc::clone(&stream);
        let daemon = daemon.clone();
        let token = token.clone();
        match thread::Builder::new()
            .name("zmux connection".to_owned())
            .spawn(move || {
                let (stream, accepted) = stream_slot
                    .lock()
                    .recover_poison()
                    .take()
                    .expect("the accepted connection was already claimed");
                let Accepted { accepted, permit } = accepted;
                let _permit = permit;
                let deadline = accepted + REQUEST_DEADLINE;
                if let Err(error) = serve(&daemon, stream, &token, deadline) {
                    log::debug!("multiplexer connection ended: {error:#}");
                }
            }) {
            Ok(_) => return,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                log::warn!(
                    "spawning a multiplexer connection thread was momentarily refused; retrying"
                );
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                log::warn!("could not serve a multiplexer connection: {error}");
                return;
            }
        }
    }
}

#[cfg(any(windows, test))]
mod agent_pipe;
mod attachment;
mod dispatch;
mod exclusive_panes;
mod image_store;
mod ipc_limits;
mod lifecycle;
mod lifetime_lock;
#[cfg(feature = "session-persistence")]
mod persistence_queue;
mod sealing;
mod secret_check;
mod sizing;
mod upgrade;
mod workers;

mod shared_layout;
use shared_layout::*;
mod subscribers;
use subscribers::*;
#[cfg(feature = "session-persistence")]
mod persist;
#[cfg(feature = "session-persistence")]
use persist::*;
mod startup;
pub use startup::*;

// The daemon was one module before it was split by responsibility, and its
// halves still call each other freely: a request handled by `dispatch` resizes
// through `sizing`, and an attach converts a pane through `lifecycle`. The
// globs keep that one namespace rather than spelling out a hundred imports.
use attachment::*;
use dispatch::*;
use exclusive_panes::*;
use image_store::*;
use ipc_limits::*;
use lifecycle::*;
use lifetime_lock::*;
#[cfg(feature = "session-persistence")]
use persistence_queue::PersistenceQueue;
pub use sealing::RecipientGrants;
use sealing::*;
use secret_check::*;
use sizing::*;
use upgrade::*;
use workers::*;
