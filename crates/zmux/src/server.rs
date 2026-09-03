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
    collections::HashMap,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::WindowSize,
    tty::{self, ChildEvent, ConsolePalette, EventedPty as _},
};
use anyhow::{Context as _, Result};

#[cfg(all(unix, not(target_os = "macos")))]
use std::os::fd::BorrowedFd;
#[cfg(all(unix, not(target_os = "macos")))]
use std::os::fd::RawFd;
#[cfg(unix)]
use std::os::fd::{AsRawFd as _, FromRawFd as _};

use crate::{
    auth::SessionAuthentication,
    catalog::{SessionCatalogPublisher, create_private_dir},
    messages::{
        ClientId, Envelope, Event, PROTOCOL_VERSION, Request, Response, SpawnRequest, TerminalSize,
    },
    paths::session_catalog_dir,
    protocol::{BackgroundPaneLayout, BackgroundSessionSummary},
    retention::Retention,
    transport::{
        Connection, Endpoint, Listener, Stream, peer_is_this_user, random_hex, token_matches,
    },
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
    /// The size the daemon last applied, or the size the client reported when
    /// it handed the pane over. Shared clients join at this size and report
    /// their own over [`Request::Resize`], which is what size arbitration
    /// starts from.
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
    client_id: ClientId,
    stream_only: bool,
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
    /// when that changed. A viewer that is merely slow keeps writing; one that has
    /// stopped reading stops writing, and only that one is dropped.
    written_seen: usize,
    wrote_at: Instant,
    columns: u16,
    lines: u16,
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
    fn is_in_scope_for(&self, client: u32) -> bool {
        self.offered || self.owner.is_none_or(|owner| owner == client)
    }
}

/// Removes panes whose process has ended and no client is reading, and ends
/// sessions that no longer hold anything to attach to.
///
/// A pane that exits while detached leaves nothing a reattach could show: the
/// process is gone, so handing its terminal to a new client produces an empty,
/// blocked tab. A session whose panes have all gone is dead and must not be
/// offered. A pane a client is still reading is kept until that client lets
/// go — the terminal it holds shows the exit — at which point the reclaim and
/// detach paths prune it here.
///
/// Returns whether anything was pruned, so the caller republishes only then.
fn prune_exited_panes(daemon: &Arc<Daemon>) -> bool {
    let mut sessions = daemon.sessions.lock().unwrap();
    let mut changed = false;
    #[cfg(feature = "session-persistence")]
    let mut removed_session_ids = Vec::new();
    #[cfg(windows)]
    let mut closed_consoles = Vec::new();
    for session in sessions.iter_mut() {
        let before = session.panes.len();
        session.panes.retain(|pane| {
            let remove = pane.exited && matches!(pane.attachment, Attachment::None);
            #[cfg(windows)]
            if remove {
                closed_consoles.push(pane.console_id);
            }
            !remove
        });
        changed |= session.panes.len() != before;
        #[cfg(feature = "session-persistence")]
        if session.panes.is_empty() {
            removed_session_ids.push(session.id);
        }
    }
    let before = sessions.len();
    sessions.retain(|session| !session.panes.is_empty());
    changed |= sessions.len() != before;
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    {
        let mut persistence = daemon.persistence.lock().unwrap();
        if let Some(persistence) = persistence.as_mut() {
            for session_id in removed_session_ids {
                if let Err(error) = persistence.forget(session_id) {
                    log::warn!("could not remove persisted session {session_id}: {error:#}");
                }
            }
        }
    }
    #[cfg(windows)]
    for console_id in closed_consoles {
        close_host_console(daemon, console_id);
    }
    changed
}

struct Session {
    id: u64,
    summary: BackgroundSessionSummary,
    state: serde_json::Value,
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
    /// `None` only for a session nobody has held yet.
    owner: Option<u32>,
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
    next_session_id: AtomicU64,
    next_pane_id: AtomicU64,
    /// The clients' long-lived event connections, keyed by client process.
    ///
    /// Keyed so a revoke can reach the one client holding a pane, instead of
    /// being broadcast to every subscriber. A client reconnecting subscribes
    /// again, replacing its earlier entry.
    subscribers: Mutex<HashMap<ClientId, Subscriber>>,
    catalog: Mutex<SessionCatalogPublisher>,
    retention: Mutex<Retention>,
    running: AtomicBool,
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
    persistence: Mutex<Option<PersistenceStore>>,
    #[cfg(feature = "session-persistence")]
    persistence_enabled: AtomicBool,
    #[cfg(feature = "session-persistence")]
    restored: Mutex<Vec<RestoredSession>>,
}

struct Subscriber {
    process_id: u32,
    connection: Connection,
}

impl Daemon {
    fn new(
        directory: &Path,
        retention: Retention,
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
            next_session_id: AtomicU64::new(next_session_id),
            next_pane_id: AtomicU64::new(1),
            subscribers: Mutex::new(HashMap::new()),
            catalog: Mutex::new(SessionCatalogPublisher::with_generation(
                directory, generation,
            )),
            retention: Mutex::new(retention),
            running: AtomicBool::new(true),
            #[cfg(feature = "session-persistence")]
            directory: directory.to_owned(),
            executable: resolve_own_executable(),
            #[cfg(windows)]
            pty_host,
            #[cfg(all(unix, not(target_os = "macos")))]
            listener_fd,
            drain_wake: Mutex::new(None),
            #[cfg(feature = "session-persistence")]
            persistence: Mutex::new(persistence),
            #[cfg(feature = "session-persistence")]
            persistence_enabled: AtomicBool::new(persistence_enabled),
            #[cfg(feature = "session-persistence")]
            restored: Mutex::new(Vec::new()),
        }
    }
}

pub fn endpoint_path(directory: &Path) -> PathBuf {
    directory.join("zmux.json")
}

fn socket_path(directory: &Path) -> PathBuf {
    directory.join("zmux.sock")
}

/// Runs the daemon until it is holding nothing and every client has gone.
///
/// `resume_from` is the handover descriptor left by a previous image that
/// replaced itself; the sessions it describes are adopted rather than started.
pub fn run(
    retention: Retention,
    persistence_recipients: Option<Vec<String>>,
    #[cfg(unix)] resume_from: Option<i32>,
    #[cfg(windows)] resume_from: Option<PathBuf>,
    #[cfg(unix)] resume_listener: Option<i32>,
    #[cfg(windows)] resume_ready: Option<PathBuf>,
) -> Result<()> {
    retention.validate()?;
    #[cfg(not(feature = "session-persistence"))]
    if persistence_recipients.is_some() {
        anyhow::bail!(
            "disk persistence needs the session-persistence feature, which this multiplexer \
             was built without"
        );
    }
    #[cfg(unix)]
    let resumed_handover = resume_from
        .map(crate::upgrade::read_handover)
        .transpose()
        .context("reading the multiplexer upgrade handover")?;
    #[cfg(windows)]
    let resumed_handover = resume_from
        .as_deref()
        .map(crate::upgrade::read_handover)
        .transpose()
        .context("reading the multiplexer upgrade handover")?;
    #[cfg(unix)]
    let generation = resumed_handover
        .as_ref()
        .map(|handover| handover.generation)
        .unwrap_or(new_generation()?);
    #[cfg(windows)]
    let generation = resumed_handover
        .as_ref()
        .map(|handover| handover.generation)
        .unwrap_or(new_generation()?);
    #[cfg(windows)]
    let retention = resumed_handover
        .as_ref()
        .map(|handover| handover.retention)
        .unwrap_or(retention);
    let directory = session_catalog_dir();
    create_private_dir(&directory)?;
    let socket = socket_path(&directory);
    let endpoint = endpoint_path(&directory);

    // Replacing itself keeps the endpoint: a client holding the old token has
    // to keep working, and reissuing one would lock out every window that is
    // attached to a session right now. A first start gets a fresh token.
    #[cfg(unix)]
    let inherited = resume_from
        .is_some()
        .then(|| Endpoint::read(&endpoint).ok())
        .flatten();
    #[cfg(windows)]
    let inherited = if resumed_handover.is_some() {
        Some(Endpoint::read(&endpoint).context("reading the daemon endpoint for replacement")?)
    } else {
        None
    };

    #[cfg(windows)]
    let pty_host = crate::pty_host::ensure_running(&directory)?;

    #[cfg(windows)]
    if let Some(handover) = resumed_handover.as_ref() {
        let ready = resume_ready
            .as_deref()
            .context("a Windows handover requires --resume-ready")?;
        validate_handover_with_host(&pty_host, handover)?;
        crate::upgrade::mark_ready(ready)?;
        wait_for_old_daemon(
            inherited
                .as_ref()
                .context("the Windows replacement has no old endpoint")?,
        )?;
    }

    // A stale socket from a daemon that died is not a reason to refuse to
    // start, but a live one is: two daemons would each own half the sessions.
    if inherited.is_none()
        && let Ok(existing) = Endpoint::read(&endpoint)
        && Stream::connect(&existing.socket_path).is_ok()
    {
        anyhow::bail!(
            "another multiplexer is already running on {}",
            existing.socket_path.display()
        );
    }
    #[cfg(target_os = "macos")]
    let resume_listener = {
        if let Some(descriptor) = resume_listener {
            // An older image may still have passed its listener through. It is
            // unusable after a Darwin exec, so close it before binding the
            // replacement's fresh listener.
            // SAFETY: the descriptor was inherited from the previous image.
            unsafe { libc::close(descriptor) };
        }
        None::<i32>
    };
    #[cfg(unix)]
    let listener = if let Some(descriptor) = resume_listener {
        anyhow::ensure!(
            resumed_handover.is_some(),
            "--resume-listener requires --resume-from"
        );
        anyhow::ensure!(
            crate::upgrade::descriptor_is_open(descriptor),
            "the inherited listener descriptor {descriptor} was not inherited"
        );
        // SAFETY: the descriptor was inherited from the previous image and is
        // claimed by this listener exactly once.
        unsafe { Listener::from_raw_fd(descriptor) }
    } else {
        #[cfg(not(target_os = "macos"))]
        anyhow::ensure!(
            resumed_handover.is_none(),
            "an upgrade did not carry its listening socket"
        );
        let _ = std::fs::remove_file(&socket);
        let listener = Listener::bind(&socket)
            .with_context(|| format!("binding the multiplexer socket {}", socket.display()))?;
        restrict_socket(&socket)?;
        listener
    };
    #[cfg(windows)]
    let listener = {
        let _ = std::fs::remove_file(&socket);
        let listener = Listener::bind(&socket)
            .with_context(|| format!("binding the multiplexer socket {}", socket.display()))?;
        restrict_socket(&socket)?;
        listener
    };
    let token = match inherited {
        Some(endpoint) => endpoint.token,
        None => random_hex(32)?,
    };
    #[cfg(feature = "session-persistence")]
    let persistence = if matches!(retention, Retention::Disk) {
        PersistenceStore::open_with_recovery_state(
            &directory,
            persistence_recipients.as_deref(),
            resume_from.is_some(),
        )?
    } else {
        // A temporary memory fallback must not make records from an earlier
        // disk daemon disappear.  Reopen the store for listing, resume, and
        // explicit cleanup, while `persistence_enabled` below keeps this
        // recovery handle from persisting new memory-mode sessions.
        PersistenceStore::open_with_recovery_state(&directory, None, false)?
    };
    #[cfg(feature = "session-persistence")]
    let next_session_id = crate::persistence::next_record_id(&directory)?;
    #[cfg(not(feature = "session-persistence"))]
    let next_session_id = 1;
    let daemon = Arc::new(Daemon::new(
        &directory,
        retention,
        #[cfg(feature = "session-persistence")]
        persistence,
        next_session_id,
        generation,
        #[cfg(all(unix, not(target_os = "macos")))]
        listener.as_raw_fd(),
        #[cfg(windows)]
        pty_host,
    ));
    #[cfg(unix)]
    if let Some(handover) = resumed_handover {
        // The previous image left its sessions behind in this same process:
        // the descriptors are still open and the shells are still this
        // process's children, so they are adopted rather than restarted.
        match adopt_handover(&daemon, handover) {
            Ok(count) => log::info!("zmux resumed {count} session(s) across an upgrade"),
            Err(error) => log::error!("could not resume sessions across the upgrade: {error:#}"),
        }
        // An older image could hand over panes whose process ended before the
        // exec: the reaper never re-examines a pane already marked exited, so
        // prune what an upgraded daemon should not offer.
        prune_exited_panes(&daemon);
        publish(&daemon);
    }
    #[cfg(windows)]
    if let Some(handover) = resumed_handover {
        let count = adopt_handover(&daemon, handover)
            .context("adopting sessions across the Windows upgrade")?;
        log::info!("zmux resumed {count} session(s) across an upgrade");
        prune_exited_panes(&daemon);
        publish(&daemon);
        if let Some(path) = resume_from.as_deref() {
            crate::upgrade::remove_handover(
                path,
                resume_ready
                    .as_deref()
                    .expect("validated Windows replacement readiness path"),
            );
        }
    }
    start_reaper(daemon.clone())?;
    start_drain(daemon.clone())?;

    // Publish only after recovery, daemon construction, handover adoption, and
    // worker setup have completed. A bound socket can accept a connection
    // before any of those steps are done, so the endpoint is not itself a
    // readiness signal; clients use `Request::Ping` as the final probe.
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: std::process::id(),
        socket_path: socket.clone(),
        token: token.clone(),
    }
    .write(&endpoint)?;

    log::info!("zmux listening on {}", socket.display());
    for stream in listener.incoming() {
        if !daemon.running.load(Ordering::SeqCst) {
            break;
        }
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("multiplexer accept failed: {error:#}");
                continue;
            }
        };
        let daemon = daemon.clone();
        let token = token.clone();
        // Never `thread::spawn`, which panics when the process is briefly out of
        // threads — and a panic here is on the accept loop, so it ends the
        // daemon and every session it holds. A connection that cannot be served
        // is one refused request the client will retry; the sessions are worth
        // more than it is.
        if let Err(error) = thread::Builder::new()
            .name("zmux connection".to_owned())
            .spawn(move || {
                if let Err(error) = serve(&daemon, stream, &token) {
                    log::debug!("multiplexer connection ended: {error:#}");
                }
            })
        {
            log::warn!("could not serve a multiplexer connection: {error}");
        }
    }

    #[cfg(feature = "session-persistence")]
    {
        let mut persistence = daemon.persistence.lock().unwrap();
        if let Some(persistence) = persistence.as_mut()
            && let Err(error) = persistence.flush_segments()
        {
            log::warn!("could not flush encrypted scrollback: {error:#}");
        }
    }
    let _ = std::fs::remove_file(&socket);
    let _ = std::fs::remove_file(&endpoint);
    Ok(())
}

fn new_generation() -> Result<u64> {
    u64::from_str_radix(&random_hex(8)?, 16).context("creating a daemon generation")
}

#[cfg(windows)]
fn validate_handover_with_host(
    host: &crate::pty_host::HostClient,
    handover: &crate::upgrade::Handover,
) -> Result<()> {
    let consoles = host.list()?;
    let expected = handover
        .sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.console_id)
        .collect::<std::collections::HashSet<_>>();
    anyhow::ensure!(
        expected.len()
            == handover
                .sessions
                .iter()
                .map(|session| session.panes.len())
                .sum::<usize>(),
        "the Windows handover contains a duplicate pseudoconsole"
    );
    let known = consoles
        .iter()
        .map(|console| console.console_id)
        .collect::<std::collections::HashSet<_>>();
    for console_id in &expected {
        anyhow::ensure!(
            known.contains(console_id),
            "the pseudoconsole host no longer holds console {console_id}"
        );
        let (_, handles) = host.handles(*console_id, std::process::id())?;
        let handles = crate::transport::claim_duplicated(&handles);
        anyhow::ensure!(
            handles.len() == 2,
            "pseudoconsole {console_id} did not return two pipe handles"
        );
    }
    // A failed daemon start must not strand a console in the host forever.
    // This is safe because the host directory identifies one daemon instance.
    for console in consoles {
        if !expected.contains(&console.console_id) {
            let _ = host.close(console.console_id);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn wait_for_old_daemon(endpoint: &Endpoint) -> Result<()> {
    const TIMEOUT: Duration = Duration::from_secs(10);
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "the previous multiplexer did not release its endpoint within {TIMEOUT:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn restrict_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting the multiplexer socket {}", path.display()))
}

/// Windows carries no mode bits to set here. The socket inherits the access
/// control of the per-user configuration directory it lives in, which is where
/// the endpoint token already relies on being private.
#[cfg(windows)]
fn restrict_socket(_path: &Path) -> Result<()> {
    Ok(())
}

/// Reads one request and checks that it may be served at all.
///
/// Every refusal answers before it returns. A connection that simply closes
/// tells the client nothing, and the client has to decide whether to fall back
/// — which it cannot do sensibly without a reason.
fn read_request(connection: &mut Connection, token: &str) -> Result<Envelope> {
    let envelope: Envelope = match connection.receive() {
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
            message: format!(
                "this multiplexer speaks protocol version {PROTOCOL_VERSION}, not {}",
                envelope.version
            ),
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
    let mut summary = session.summary.clone();
    summary.held = session.is_held();
    // Taken from the session rather than trusted from the stored summary, so it
    // follows the verifier: a share or scope request that republishes a summary
    // without it does not silently drop the way in to a protected session.
    summary.key_envelope = session.key_envelope.clone();
    // Whose it is, so a process reading the one shared catalog can tell its own
    // sessions from another window's. An offered session is nobody's in
    // particular and carries no scope at all.
    summary.scoped_to = (!session.offered).then_some(session.owner).flatten();
    summary
}

fn publish(daemon: &Arc<Daemon>) {
    let sessions = daemon
        .sessions
        .lock()
        .unwrap()
        .iter()
        .filter(|session| session.is_available())
        .map(catalog_summary)
        .collect::<Vec<_>>();
    let mut catalog = daemon.catalog.lock().unwrap();
    if let Err(error) = catalog.publish_sessions(sessions) {
        log::warn!("could not publish the session catalog: {error:#}");
    }
}

#[cfg(feature = "session-persistence")]
fn persist_session(daemon: &Arc<Daemon>, session: &PersistedSession) -> Result<()> {
    if !daemon.persistence_enabled.load(Ordering::Acquire) {
        return Ok(());
    }
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence.save_session(session)?;
    }
    Ok(())
}

#[cfg(feature = "session-persistence")]
fn persisted_live_session(session: &Session) -> PersistedSession {
    PersistedSession {
        id: session.id,
        created_at: unix_now(),
        updated_at: unix_now(),
        summary: session.summary.clone(),
        state: session.state.clone(),
        verifier: session
            .authentication
            .as_ref()
            .map(|authentication| authentication.verifier().to_owned()),
        key_envelope: session.key_envelope.clone(),
        failed_authentications: session.failed_authentications,
        backoff_seconds: session
            .refuse_until
            .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or_default(),
        snapshots: session
            .panes
            .iter()
            .map(|pane| {
                let (columns, lines) =
                    terminal_size(pane).unwrap_or((pane.size.columns, pane.size.lines));
                PersistedSnapshot {
                    pane_id: pane.id,
                    bytes: pane.retained.snapshot(),
                    columns: Some(columns),
                    lines: Some(lines),
                }
            })
            .collect(),
    }
}

#[cfg(feature = "session-persistence")]
fn forget_persisted_session(daemon: &Arc<Daemon>, session_id: u64) -> Result<()> {
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut()
        && persistence
            .records()
            .iter()
            .any(|record| record.id == session_id)
    {
        persistence.forget(session_id)?;
    }
    Ok(())
}

#[cfg(feature = "session-persistence")]
fn record_persistence_output(daemon: &Arc<Daemon>, session_id: u64, pane_id: u64, bytes: &[u8]) {
    if !daemon.persistence_enabled.load(Ordering::Acquire) {
        return;
    }
    let result = daemon
        .persistence
        .lock()
        .unwrap()
        .as_mut()
        .map(|persistence| persistence.append_scrollback(session_id, pane_id, bytes))
        .transpose();
    if let Err(error) = result {
        log::warn!("could not persist pane {pane_id} output: {error:#}");
    }
}

#[cfg(feature = "session-persistence")]
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn broadcast(daemon: &Arc<Daemon>, event: &Event) {
    let mut subscribers = daemon.subscribers.lock().unwrap();
    subscribers.retain(|_, subscriber| subscriber.connection.send(event).is_ok());
}

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

mod attachment;
mod dispatch;
mod lifecycle;
mod sizing;
mod upgrade;
mod workers;

// The daemon was one module before it was split by responsibility, and its
// halves still call each other freely: a request handled by `dispatch` resizes
// through `sizing`, and an attach converts a pane through `lifecycle`. The
// globs keep that one namespace rather than spelling out a hundred imports.
use attachment::*;
use dispatch::*;
use lifecycle::*;
use sizing::*;
use upgrade::*;
use workers::*;
