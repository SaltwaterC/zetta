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

#[cfg(unix)]
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, RawFd};

use crate::{
    auth::SessionAuthentication,
    catalog::{SessionCatalogPublisher, create_private_dir},
    messages::{Envelope, Event, PROTOCOL_VERSION, Request, Response, SpawnRequest, TerminalSize},
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
struct RestoredSession {
    request: crate::messages::ResumeRequest,
    restored_at: u64,
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
    subscribers: Mutex<HashMap<u32, Connection>>,
    catalog: Mutex<SessionCatalogPublisher>,
    retention: Mutex<Retention>,
    running: AtomicBool,
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
    #[cfg(unix)]
    /// The listener is inherited during an upgrade so clients never observe a
    /// rebind gap.
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

impl Daemon {
    fn new(
        directory: &Path,
        retention: Retention,
        #[cfg(feature = "session-persistence")] persistence: Option<PersistenceStore>,
        next_session_id: u64,
        generation: u64,
        #[cfg(unix)] listener_fd: RawFd,
        #[cfg(windows)] pty_host: crate::pty_host::HostClient,
    ) -> Self {
        #[cfg(feature = "session-persistence")]
        let persistence_enabled = persistence.is_some();
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
            executable: resolve_own_executable(),
            #[cfg(windows)]
            pty_host,
            #[cfg(unix)]
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
    Endpoint {
        version: crate::transport::ENDPOINT_VERSION,
        protocol_version: PROTOCOL_VERSION,
        process_id: std::process::id(),
        socket_path: socket.clone(),
        token: token.clone(),
    }
    .write(&endpoint)?;

    #[cfg(feature = "session-persistence")]
    let persistence = if matches!(retention, Retention::Disk) {
        PersistenceStore::open_with_recovery_state(
            &directory,
            persistence_recipients.as_deref(),
            resume_from.is_some(),
        )?
    } else {
        None
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
        #[cfg(unix)]
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

fn serve(daemon: &Arc<Daemon>, stream: Stream, token: &str) -> Result<()> {
    // A terminal must never cross a user boundary. The socket's permissions
    // already restrict this; checking the peer as well means a mistake in the
    // directory's access control is not on its own enough.
    anyhow::ensure!(
        peer_is_this_user(&stream)?,
        "refusing a connection from another user"
    );
    let peer_process_id = crate::transport::peer_process_id(&stream)?;

    let mut connection = Connection::new(stream);
    let mut envelope = read_request(&mut connection, token)?;

    // A client may ask to be identified before it sends the request that needs
    // it, which is how a request that streams raw bytes after its message gets
    // an identity: the daemon cannot interject a challenge there, because the
    // bytes would arrive where it was expecting the answer.
    #[cfg(windows)]
    let peer_process_id = if matches!(envelope.request, Request::Attest) {
        let attested = match peer_process_id {
            attested @ Some(_) => attested,
            None => {
                attest_peer(&mut connection, envelope.client_process_id).unwrap_or_else(|error| {
                    log::debug!("peer attestation failed: {error:#}");
                    None
                })
            }
        };
        connection.send(&Response::Ok)?;
        envelope = read_request(&mut connection, token)?;
        attested
    } else {
        peer_process_id
    };
    // Nothing to establish where the kernel already answered the question, but
    // the exchange still has to be answered so a portable client can ask.
    #[cfg(unix)]
    if matches!(envelope.request, Request::Attest) {
        connection.send(&Response::Ok)?;
        envelope = read_request(&mut connection, token)?;
    }

    // Where the platform reports no peer credentials, a request whose answer
    // depends on who is asking has to establish that first — see
    // `transport::PeerChallenge`. A peer that will not or cannot answer is not
    // refused here: it simply proceeds without an identity, and the checks that
    // needed one refuse it with their own message.
    #[cfg(windows)]
    let peer_process_id = match peer_process_id {
        attested @ Some(_) => attested,
        None if attestation_needed(daemon, &envelope.request) => {
            match attest_peer(&mut connection, envelope.client_process_id) {
                Ok(attested) => attested,
                Err(error) => {
                    log::debug!("peer attestation failed: {error:#}");
                    None
                }
            }
        }
        None => None,
    };

    match envelope.request {
        Request::Subscribe => {
            // Keyed by process so a revoke can be sent to the one client that
            // holds a pane rather than broadcast to every subscriber. A client
            // that subscribes twice is the same process, so the later
            // connection replaces the earlier one.
            daemon
                .subscribers
                .lock()
                .unwrap()
                .insert(envelope.client_process_id, connection);
            Ok(())
        }
        Request::Spawn(request) => {
            let response = spawn(daemon, request, &mut connection);
            match response {
                Ok(()) => Ok(()),
                Err(error) => {
                    let _ = connection.send(&Response::Error {
                        message: format!("{error:#}"),
                    });
                    Err(error)
                }
            }
        }
        Request::Attach {
            session_id,
            pane_id,
            secret,
        } => attach(
            daemon,
            session_id,
            pane_id,
            secret,
            envelope.client_process_id,
            &mut connection,
        ),
        // The revoke handover's answer, from the client that was holding the
        // pane. Its own connection, distinct from the one the attach that
        // started the revoke is waiting on.
        Request::Snapshot {
            session_id,
            pane_id,
            length,
            columns,
            lines,
        } => snapshot(
            daemon,
            session_id,
            pane_id,
            length,
            columns,
            lines,
            envelope.client_process_id,
            &mut connection,
        ),
        Request::Input { .. } => connection.send(&Response::Error {
            message: "input belongs on a shared connection".to_owned(),
        }),
        Request::Attested { .. } | Request::Attest => connection.send(&Response::Error {
            message: "an attestation precedes a request, it does not replace one".to_owned(),
        }),
        Request::Detach(request) => match detach(
            daemon,
            request,
            envelope.client_process_id,
            peer_process_id,
            &mut connection,
        ) {
            Ok(()) => Ok(()),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Resume(request) => match resume(
            daemon,
            request,
            envelope.client_process_id,
            peer_process_id,
            &mut connection,
        ) {
            Ok(()) => Ok(()),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::TakeExclusive {
            session_id,
            pane_id,
        } => take_exclusive(
            daemon,
            session_id,
            pane_id,
            envelope.client_process_id,
            &mut connection,
        ),
        Request::Share(request) => share(
            daemon,
            request,
            envelope.client_process_id,
            peer_process_id,
            &mut connection,
        ),
        Request::Resize {
            session_id,
            pane_id,
            columns,
            lines,
        } => match resize_pane(daemon, session_id, pane_id, columns, lines, peer_process_id) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::SetConsolePalette {
            session_id,
            pane_id,
            palette,
        } => match set_console_palette(
            daemon,
            session_id,
            pane_id,
            palette,
            envelope.client_process_id,
            peer_process_id,
        ) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::List => {
            // Sanitized exactly as the published catalog is. The endpoint token
            // authenticates the channel, not a session: listing must not reveal
            // the commands, titles or directories of a session whose whole
            // point is that they stay private until its secret is presented.
            let sessions = daemon
                .sessions
                .lock()
                .unwrap()
                .iter()
                .filter(|session| session.is_available())
                .map(|session| catalog_summary(session).for_public_catalog())
                .collect();
            #[cfg(feature = "session-persistence")]
            let mut restorable: Vec<crate::protocol::RestorableSessionRecord> = daemon
                .persistence
                .lock()
                .unwrap()
                .as_ref()
                .map(|persistence| {
                    persistence
                        .records()
                        .iter()
                        .filter(|record| record.restorable)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            #[cfg(feature = "session-persistence")]
            for restored in daemon.restored.lock().unwrap().iter() {
                restorable.push(crate::protocol::RestorableSessionRecord {
                    id: restored.request.record_id,
                    created_at: restored.restored_at,
                    updated_at: restored.restored_at,
                    metadata_bytes: 0,
                    snapshot_bytes: 0,
                    scrollback_bytes: 0,
                    protected: restored.request.verifier.is_some(),
                    restorable: false,
                });
            }
            #[cfg(not(feature = "session-persistence"))]
            let restorable = Vec::new();
            connection.send(&Response::Sessions {
                sessions,
                restorable,
            })
        }
        Request::PaneStates { pane_ids } => {
            let panes = pane_states(daemon, &pane_ids, peer_process_id);
            connection.send(&Response::PaneStates { panes })
        }
        Request::ClosePane {
            session_id,
            pane_id,
        } => {
            match close_pane(
                daemon,
                session_id,
                pane_id,
                envelope.client_process_id,
                peer_process_id,
            ) {
                Ok(()) => connection.send(&Response::Ok),
                Err(error) => connection.send(&Response::Error {
                    message: format!("{error:#}"),
                }),
            }
        }
        Request::Kill { session_id } => match kill(daemon, session_id, peer_process_id) {
            Ok(()) => {
                publish(daemon);
                connection.send(&Response::Ok)
            }
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::SetSessionScope {
            session_id,
            shared,
            verifier,
        } => match set_session_scope(
            daemon,
            session_id,
            shared,
            verifier,
            peer_process_id,
            &mut connection,
        ) {
            Ok(()) => Ok(()),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Forget { session_id } => match forget(daemon, session_id, peer_process_id) {
            Ok(()) => {
                publish(daemon);
                connection.send(&Response::Ok)
            }
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Configure {
            retention,
            persistence_recipients,
        } => match configure_daemon(daemon, retention, persistence_recipients) {
            Ok(()) => connection.send(&Response::Ok),
            Err(error) => connection.send(&Response::Error {
                message: format!("{error:#}"),
            }),
        },
        Request::Upgrade => {
            #[cfg(any(unix, windows))]
            {
                // Only returns when the replacement was refused: a successful
                // upgrade never comes back, because this process becomes it.
                match upgrade_daemon(daemon, &mut connection) {
                    Ok(()) => Ok(()),
                    // The reply is only reported here while the upgrade could
                    // still be refused. Past the point of no return the request
                    // has already been answered, and writing a second reply onto
                    // the same connection would leave the client parsing one
                    // message as the tail of another.
                    Err(UpgradeRefused::Before(error)) => connection.send(&Response::Error {
                        message: format!("{error:#}"),
                    }),
                    Err(UpgradeRefused::AfterAnswering(error)) => {
                        log::error!("the multiplexer could not replace itself: {error:#}");
                        Ok(())
                    }
                }
            }
            #[cfg(not(any(unix, windows)))]
            connection.send(&Response::Error {
                message: "replacing the multiplexer in place is not supported on this platform; \
                          a pseudoconsole cannot be moved between processes"
                    .to_owned(),
            })
        }
        Request::Shutdown => {
            let held = daemon.sessions.lock().unwrap().len();
            if held > 0 {
                // Answered rather than ignored. Replying `Ok` to a request that
                // was not honoured left the caller to guess, and what it guessed
                // was that the multiplexer had stopped.
                return connection.send(&Response::Error {
                    message: format!(
                        "the multiplexer is holding {held} session{}",
                        if held == 1 { "" } else { "s" }
                    ),
                });
            }
            // Ask the separate Windows host to exit before acknowledging the
            // request. The reply must be sent before `running` is cleared:
            // once the accept loop is woken, this process can finish while
            // this connection worker is still running, which would close the
            // socket before the caller received the successful stop.
            #[cfg(windows)]
            if let Err(error) = daemon.pty_host.shutdown() {
                return connection.send(&Response::Error {
                    message: format!("could not stop the pseudoconsole host: {error:#}"),
                });
            }
            let response = connection.send(&Response::Ok);
            daemon.running.store(false, Ordering::SeqCst);
            // Unblock the accept loop so it observes the flag.
            let _ = Stream::connect(socket_path(&session_catalog_dir()));
            response
        }
    }
}

/// Applies settings from a client that may have been started after this
/// daemon. A daemon deliberately outlives its clients, so treating its startup
/// arguments as permanent makes editing the configuration appear to work while
/// leaving all subsequent sessions under the old retention policy.
fn configure_daemon(
    daemon: &Arc<Daemon>,
    retention: Retention,
    persistence_recipients: Vec<String>,
) -> Result<()> {
    retention.validate()?;
    #[cfg(not(feature = "session-persistence"))]
    let _ = persistence_recipients;

    let old_retention = *daemon.retention.lock().unwrap();
    let mut sessions = daemon.sessions.lock().unwrap();
    if old_retention != retention {
        for session in sessions.iter_mut() {
            for pane in &mut session.panes {
                let snapshot = pane.retained.snapshot();
                let mut retained = retention.new_retained(pane.size.columns, pane.size.lines);
                retained.seed(snapshot);
                pane.retained = retained;
            }
        }
    }
    #[cfg(feature = "session-persistence")]
    let persisted_sessions = sessions
        .iter()
        .filter(|session| session.keep || session.offered)
        .map(persisted_live_session)
        .collect::<Vec<_>>();
    drop(sessions);

    #[cfg(feature = "session-persistence")]
    let mut next_persistence = {
        let mut persistence = daemon.persistence.lock().unwrap();
        if let Some(persistence) = persistence.as_mut() {
            persistence
                .flush_segments()
                .context("flushing encrypted scrollback before changing retention")?;
        }
        if matches!(retention, Retention::Disk) && !persistence_recipients.is_empty() {
            PersistenceStore::open_with_recovery_state(
                &session_catalog_dir(),
                Some(&persistence_recipients),
                true,
            )?
        } else {
            None
        }
    };

    #[cfg(feature = "session-persistence")]
    {
        if let Some(persistence) = next_persistence.as_mut() {
            for session in &persisted_sessions {
                persistence.save_session(session)?;
            }
        }
        let mut persistence = daemon.persistence.lock().unwrap();
        let persistence_enabled = next_persistence.is_some();
        *persistence = next_persistence;
        daemon
            .persistence_enabled
            .store(persistence_enabled, Ordering::Release);
    }
    *daemon.retention.lock().unwrap() = retention;
    wake_drain(daemon);
    publish(daemon);
    Ok(())
}

fn spawn(daemon: &Arc<Daemon>, request: SpawnRequest, connection: &mut Connection) -> Result<()> {
    #[cfg(unix)]
    let options = tty::Options {
        shell: request
            .program
            .map(|program| tty::Shell::new(program, request.args)),
        working_directory: request.working_directory,
        drain_on_exit: true,
        env: request.env,
        #[cfg(not(windows))]
        child_signal_mask: None,
        // Arguments are passed through as the client resolved them, so the
        // multiplexer must not re-quote what has already been quoted.
        #[cfg(windows)]
        escape_args: false,
    };
    #[cfg(unix)]
    let pty = tty::new(&options, window_size(request.size), 0)
        .context("starting the terminal process")?;
    #[cfg(unix)]
    let child_pid = pty.child_pid();
    #[cfg(windows)]
    let (console_id, child_pid, pty, child_events) = {
        let (console_id, child_pid, handles) = daemon.pty_host.open(
            request.program,
            request.args,
            request.env,
            request.working_directory,
            request.size,
            request.console_palette,
            std::process::id(),
        )?;
        let mut handles = crate::transport::claim_duplicated(&handles);
        if handles.len() != 2 {
            let _ = daemon.pty_host.close(console_id);
            anyhow::bail!("the pseudoconsole host did not return two pipe handles");
        }
        let conin = handles.remove(1);
        let conout = handles.remove(0);
        match tty::attach(conout, conin, child_pid) {
            Ok((pty, child_events)) => (console_id, child_pid, pty, child_events),
            Err(error) => {
                let _ = daemon.pty_host.close(console_id);
                return Err(error).context("attaching the pseudoconsole to the daemon");
            }
        }
    };
    let pane_id = daemon.next_pane_id.fetch_add(1, Ordering::Relaxed);
    let retention = *daemon.retention.lock().unwrap();

    let mut sessions = daemon.sessions.lock().unwrap();
    let session_id = match request.session_id {
        Some(id) if sessions.iter().any(|session| session.id == id) => id,
        _ => {
            let id = daemon.next_session_id.fetch_add(1, Ordering::Relaxed);
            sessions.push(Session {
                id,
                summary: BackgroundSessionSummary {
                    id,
                    title: String::new(),
                    authentication_required: false,
                    active_pane: pane_id,
                    layout: BackgroundPaneLayout::Pane { pane_id },
                    panes: Vec::new(),
                    held: false,
                    scoped_to: None,
                },
                state: serde_json::Value::Null,
                authentication: None,
                failed_authentications: 0,
                refuse_until: None,
                panes: Vec::new(),
                keep: false,
                offered: false,
                // The window that spawned it owns it from the start, so a tab
                // backgrounded later is already this process's and needs no
                // separate claim.
                owner: Some(request.client_process_id),
            });
            id
        }
    };
    let session = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .expect("the session was just located or created");

    // Record the pane before answering. A handover that fails after the
    // session exists but before the pane does would otherwise leave a session
    // holding nothing: unattachable, and — because "every pane is unattached"
    // is vacuously true of no panes — liable to be published as though the
    // user could pick it up.
    session.panes.push(Pane {
        id: pane_id,
        pty,
        #[cfg(windows)]
        console_id,
        #[cfg(windows)]
        child_events,
        attachment: exclusive_attachment(request.client_process_id),
        size: request.size,
        retained: retention.new_retained(request.size.columns, request.size.lines),
        handed_over: None,
        handover_waiters: 0,
        exited: false,
        exit_status: None,
        pending_input: Vec::new(),
    });
    let pane = session.panes.last().expect("the pane was just pushed");
    let handover = handover_handles(daemon, pane, request.client_process_id).and_then(|handles| {
        connection.send_with(
            &Response::Spawned {
                session_id,
                pane_id,
                child_pid,
                handles: handles.values,
            },
            &handles.attachments,
        )
    });
    if let Err(error) = handover {
        // Nobody received this terminal, so nothing can ever read it.
        session.panes.retain(|pane| pane.id != pane_id);
        sessions.retain(|session| !session.panes.is_empty());
        #[cfg(windows)]
        let _ = daemon.pty_host.close(console_id);
        return Err(error);
    }
    Ok(())
}

fn attach(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: Option<u64>,
    secret: Option<String>,
    client_process_id: u32,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };

    // Whose session it is, before anything is revealed about it. A backgrounded
    // session belongs to the window that put it away; another process may only
    // have it once somebody has said so.
    if !session.is_in_scope_for(client_process_id) {
        let owner = session.owner.unwrap_or_default();
        // Two different situations, and the way out differs: that window can
        // share the session itself, but a window that has exited cannot, so say
        // which one this is rather than suggesting something impossible.
        let route = if process_is_running(owner) {
            format!("share it from that window, or with `zmux share {session_id}`")
        } else {
            format!(
                "that window has exited, and a backgrounded session does not become another \
                 window's on its own; `zmux share {session_id}` opens it up"
            )
        };
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} is scoped to the Zetta process that backgrounded it \
                 (process {owner}): {route}"
            ),
        });
    }
    // Held here from now on: whoever is showing a session is who it goes back to
    // when it is backgrounded again, and who may scope it back after sharing.
    if !session.offered {
        session.owner = Some(client_process_id);
    }

    // The holder answering a handover the daemon itself asked for is not a new
    // grant of access: that client is displaying the pane right now, and was
    // sent `Event::Revoke` a moment ago. Challenging it would make a protected
    // session impossible to share — the handover carries no secret, because the
    // user typed it into whichever *other* window is joining.
    let mid_handover = session.panes.iter().any(|pane| {
        // Either half of the handshake: the revoke has been sent and not
        // answered yet, or it has been answered — which is what
        // `handed_over` records — and this is the answering client coming
        // back to join. Any pane of the session counts, because the client
        // that is mid-handover on one is the client displaying them all.
        matches!(pane.attachment, Attachment::Revoking { holder } | Attachment::Granting { holder } if holder == client_process_id)
            || pane
                .handed_over
                .as_ref()
                .is_some_and(|handover| handover.client_process_id == client_process_id)
    });
    if let Some(authentication) = session.authentication.clone().filter(|_| !mid_handover) {
        let Some(secret) = secret else {
            return connection.send(&Response::AuthenticationRequired);
        };
        // An attempt inside the backoff window is refused without being
        // evaluated, and reports the same failure as a wrong secret so the
        // window cannot be probed.
        let refused = session
            .refuse_until
            .is_some_and(|until| std::time::Instant::now() < until);
        if refused || authentication.verify(&secret).is_none() {
            if !refused {
                session.failed_authentications = session.failed_authentications.saturating_add(1);
                session.refuse_until = std::time::Instant::now().checked_add(
                    crate::auth::failed_authentication_delay(session.failed_authentications),
                );
            }
            return connection.send(&Response::AuthenticationFailed);
        }
        session.failed_authentications = 0;
        session.refuse_until = None;
    }

    let state = session.state.clone();
    let summary = Box::new(session.summary.clone());
    // Resolved only now, after the secret has been checked: which panes a
    // protected session has is part of what its secret protects.
    let pane_id = match pane_id {
        Some(pane_id) => pane_id,
        None => match session.panes.first() {
            Some(pane) => pane.id,
            None => {
                return connection.send(&Response::Error {
                    message: format!("session {session_id} has no panes"),
                });
            }
        },
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if pane.exited {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} has ended"),
        });
    }

    // A pane nobody is reading, or that this same process already holds, can
    // be handed over directly. The latter preserves the original behaviour:
    // a client re-attaching — after a crash of its window, say — takes its
    // pane back rather than starting a revoke against itself.
    if matches!(pane.attachment, Attachment::None)
        || matches!(pane.attachment, Attachment::Exclusive(holder) if holder == client_process_id)
    {
        return attach_exclusive(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            state,
            summary,
            connection,
        );
    }
    if matches!(pane.attachment, Attachment::Shared(_)) {
        return attach_shared(
            daemon,
            sessions,
            session_id,
            pane_id,
            client_process_id,
            state,
            summary,
            connection,
        );
    }

    // Another client holds the pane, or is already handing it over. From here
    // on the pane is committed to shared mode, whether or not this client ends
    // up attaching: the holder's own re-attach joins the same shared set.
    match pane.attachment {
        Attachment::Exclusive(holder) => {
            // Ask the holder to hand the terminal over, and wait for its
            // snapshot. The wait releases the sessions lock, so the snapshot
            // handler can change the pane's attachment and notify us.
            pane.attachment = Attachment::Revoking { holder };
            let revoke = Event::Revoke {
                session_id,
                pane_id,
            };
            let subscribers = daemon.subscribers.lock().unwrap();
            let subscriber = subscribers
                .get(&holder)
                .map(|connection| connection.try_clone());
            drop(subscribers);
            if let Some(Ok(mut subscriber)) = subscriber
                && let Err(error) = subscriber.send(&revoke)
            {
                log::debug!("revoke delivery to client {holder} failed: {error:#}");
            }
        }
        // Mid-handover in either direction. Wait: a revoke resolves to shared,
        // and a grant resolves to exclusive, which the wait loop then revokes.
        Attachment::Revoking { .. } | Attachment::Granting { .. } => {}
        Attachment::None | Attachment::Shared(_) => {
            unreachable!("the pane's attachment was decided above");
        }
    }
    // Keep the shared-empty state alive until this request has joined. A
    // holder can re-attach concurrently with this waiter and its connection
    // may close before the waiter gets the condvar wake-up.
    pane.handover_waiters = pane.handover_waiters.saturating_add(1);
    let _ = pane;

    let deadline = Instant::now() + REVOKE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if let Some(session) = sessions.iter_mut().find(|session| session.id == session_id)
                && let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id)
            {
                finish_handover_waiter(pane, true);
            }
            return connection.send(&Response::Error {
                message: format!(
                    "the session's current viewer did not hand over pane {pane_id} within \
                     {REVOKE_TIMEOUT:?}"
                ),
            });
        }
        let (guard, _) = match daemon.sessions_condvar.wait_timeout(sessions, remaining) {
            Ok(result) => result,
            // A poisoned lock still hands the guard back; nothing panicked
            // out of the wait that a re-lock would fix.
            Err(poisoned) => poisoned.into_inner(),
        };
        sessions = guard;
        let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} does not exist"),
            });
        };
        // Re-read rather than reusing what was read before the revoke. The
        // holder of a *live* session republishes its state as part of answering
        // the revoke, because a session shared while it is on screen goes on
        // changing — panes are split and closed, tabs are renamed — long after it
        // was first offered. Using the stale copy handed a joining client the
        // layout as of whenever sharing was switched on.
        let state = session.state.clone();
        let summary = Box::new(session.summary.clone());
        let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return connection.send(&Response::Error {
                message: format!("session {session_id} has no pane {pane_id}"),
            });
        };
        if pane.exited {
            finish_handover_waiter(pane, true);
            return connection.send(&Response::Error {
                message: format!("session {session_id} pane {pane_id} has ended"),
            });
        }
        match pane.attachment {
            Attachment::Shared(_) => {
                finish_handover_waiter(pane, false);
                return attach_shared(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::None => {
                finish_handover_waiter(pane, true);
                return attach_exclusive(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::Exclusive(holder) => {
                // Reachable, and it used to panic here. A revoking pane whose
                // holder died is reset to unheld — by this loop or by the
                // liveness sweep — and whichever waiter wakes first takes it
                // exclusively, so a second waiter finds it exclusive again. That
                // is another client holding the pane, which is exactly the case
                // this function already handles from the top, so start over
                // rather than treating it as impossible.
                if holder == client_process_id {
                    finish_handover_waiter(pane, true);
                    return attach_exclusive(
                        daemon,
                        sessions,
                        session_id,
                        pane_id,
                        client_process_id,
                        state,
                        summary,
                        connection,
                    );
                }
                pane.attachment = Attachment::Revoking { holder };
                let revoke = Event::Revoke {
                    session_id,
                    pane_id,
                };
                let subscriber = daemon
                    .subscribers
                    .lock()
                    .unwrap()
                    .get(&holder)
                    .map(|connection| connection.try_clone());
                if let Some(Ok(mut subscriber)) = subscriber
                    && let Err(error) = subscriber.send(&revoke)
                {
                    log::debug!("revoke delivery to client {holder} failed: {error:#}");
                }
            }
            // A grant whose taker died leaves nobody holding the descriptor: the
            // reply either never went out or went to a process that has gone.
            Attachment::Granting { holder } | Attachment::Revoking { holder }
                if !process_is_running(holder) =>
            {
                pane.attachment = Attachment::None;
                finish_handover_waiter(pane, true);
                return attach_exclusive(
                    daemon,
                    sessions,
                    session_id,
                    pane_id,
                    client_process_id,
                    state,
                    summary,
                    connection,
                );
            }
            Attachment::Revoking { .. } | Attachment::Granting { .. } => {}
        }
    }
}

/// Releases one attach request waiting for a revoke.
fn finish_handover_waiter(pane: &mut Pane, collapse_empty_shared: bool) {
    pane.handover_waiters = pane.handover_waiters.saturating_sub(1);
    if collapse_empty_shared
        && pane.handover_waiters == 0
        && matches!(&pane.attachment, Attachment::Shared(clients) if clients.is_empty())
    {
        pane.attachment = Attachment::None;
        pane.handed_over = None;
    }
}

/// An exclusive attach: the client takes the descriptor and reads it directly.
///
/// The daemon stops reading before handing the descriptor over, and sends
/// everything already read with it. Any bytes still in the PTY buffer stay
/// there for the client to read, so nothing is lost or duplicated across the
/// switch.
///
/// Takes the sessions guard by value: the publish that announces the attach
/// must not run while the lock is still held — the caller's guard would dead
/// lock against itself.
#[allow(clippy::too_many_arguments)]
fn attach_exclusive(
    daemon: &Arc<Daemon>,
    mut sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
) -> Result<()> {
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if let Err(error) = pause_pane_reader(pane) {
        return connection.send(&Response::Error {
            message: format!("could not stop the pane reader before attaching: {error:#}"),
        });
    }
    pane.attachment = exclusive_attachment(client_process_id);
    // The screen, and then nothing: the client reads the terminal itself from
    // here, so what the daemon holds stops describing this pane and must not be
    // served to a later attach as though it did.
    let replay = pane.retained.snapshot();
    pane.retained.clear();
    let child_pid = pane.pty.child_pid();
    let handles = handover_handles(daemon, pane, client_process_id)?;
    // The guard is this function's to release: publish re-locks the sessions
    // mutex, and the persistence catalogue must be updated before the reply is
    // visible to a client that may immediately refresh it.
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Err(error) = forget_persisted_session(daemon, session_id) {
        log::warn!("could not remove the attached session's persisted record: {error:#}");
    }
    connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: replay.len(),
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments,
    )?;
    if !replay.is_empty() {
        connection.write_all(&replay)?;
    }
    publish(daemon);
    wake_drain(daemon);
    Ok(())
}

/// Joins a pane's shared set, keeping `connection` open as this client's
/// shared data plane: output and size events arrive on it, and input and size
/// reports go back over it.
///
/// Takes the sessions guard by value: after the handshake the guard is
/// dropped, because this function then serves the client's connection for its
/// whole lifetime, and that serve loop re-locks the sessions mutex on every
/// input and resize.
#[allow(clippy::too_many_arguments)]
fn attach_shared(
    daemon: &Arc<Daemon>,
    mut sessions: MutexGuard<'_, Vec<Session>>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    state: serde_json::Value,
    summary: Box<BackgroundSessionSummary>,
    connection: &mut Connection,
) -> Result<()> {
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    anyhow::ensure!(
        matches!(pane.attachment, Attachment::Shared(_)),
        "pane {pane_id} is not shared"
    );
    // Everything that can fail happens before the attachment is touched. Taking
    // the shared set out first and then failing dropped every client already in
    // it — silently, since a relayed viewer learns nothing until its output
    // stops.
    //
    // The relay is a queue and its own thread, not a write from wherever the
    // output was read: the reader holds the sessions lock, and a socket write
    // under that lock is a viewer's stall becoming everybody's.
    let relay = spawn_relay(connection, client_process_id)?;
    let Attachment::Shared(clients) = &mut pane.attachment else {
        unreachable!("the attachment was just checked to be shared");
    };
    // A shared client starts at the size everyone is showing, and reports its
    // own over Resize right after attaching; that report is what may change
    // the pane's effective size.
    clients.push(SharedClient {
        process_id: client_process_id,
        relay,
        written_seen: 0,
        wrote_at: Instant::now(),
        columns: pane.size.columns,
        lines: pane.size.lines,
        input_sent: false,
    });
    let (columns, lines) = effective_size(pane);
    // Kept, not taken: every later viewer joins on the same screen, and the one
    // that hands the pane over consumed it first, leaving everybody after it
    // with nothing but the redraws the program happened to make since. A
    // full-screen program redraws only what changed, so that is a screen of
    // holes — its static text never arrives.
    // Kept, not consumed, and only the client that handed the pane over is
    // spared the screen it handed over. Taking the buffer left everybody who
    // joined afterwards with nothing but the redraws the program happened to
    // make since — and a full-screen program redraws only what changed, so that
    // is a screen of holes where its static text should be. Matched by client
    // rather than by being first, because the joining client that triggered the
    // revoke races the holder's own re-attach.
    let replay = match pane
        .handed_over
        .take_if(|handover| handover.client_process_id == client_process_id)
    {
        // The client that handed the pane over is still showing the screen it
        // handed over, so it gets the difference rather than the screen.
        Some(handover) => handover.output,
        None => pane.retained.snapshot(),
    };
    let child_pid = pane.pty.child_pid();
    connection.send(&Response::SharedAttached {
        pane_id,
        child_pid,
        replay_length: replay.len(),
        state,
        summary,
        columns,
        lines,
    })?;
    if !replay.is_empty() {
        connection.write_all(&replay)?;
    }

    drop(sessions);
    publish(daemon);
    wake_drain(daemon);

    // Serve the shared connection until the client goes away.
    let result = serve_shared(daemon, session_id, pane_id, client_process_id, connection);
    remove_shared_client(daemon, session_id, pane_id, client_process_id);
    result
}

/// One shared client's relay: the queue its frames go on, and how far behind it
/// is in bytes.
///
/// The backlog is measured in bytes rather than frames, because a frame count
/// punishes a viewer for the *shape* of the output instead of how far behind it
/// has fallen — a burst arriving as many small frames hit a frame limit while
/// barely a kilobyte outstanding, and the viewer was dropped for being slow for
/// an instant.
struct Relay {
    frames: async_channel::Sender<Arc<[u8]>>,
    queued: Arc<AtomicUsize>,
    /// Bytes this relay has actually written, ever increasing.
    ///
    /// This, and not the size of the backlog, is what says whether a viewer is
    /// making progress. A viewer slower than the program has a backlog that *grows*
    /// while it writes steadily, so "the backlog shrank" reads as no progress and
    /// evicted exactly the viewer that had to be waited for; a viewer whose socket
    /// is full writes nothing at all, which this shows plainly.
    written: Arc<AtomicUsize>,
}

/// How far behind a viewer may fall before its pane stops being read.
///
/// Small on purpose: a backpressure threshold, not a buffer budget. Past it the
/// pane is left unread so the *program* waits, which is the rate limit a client
/// reading its own pty provides for free.
const RELAY_BACKPRESSURE_BYTES: usize = 512 * 1024;

/// How long a viewer may accept nothing at all before the daemon gives up on it.
///
/// Deliberately far longer than anything a working window does. This is not a
/// performance policy — a viewer that is merely slow is *waited for*, which is what
/// backpressure is — it is a last resort for a window that has hung with a pane
/// still attached, so one frozen window cannot hold a session for ever.
///
/// It was one second, and that was a bad mistake: a window laying out a full-screen
/// redraw goes that long without draining its socket as a matter of course, so the
/// daemon dropped viewers that were working perfectly well and left them frozen
/// mid-repaint. Slowness is normal; not reading for half a minute is not.
const RELAY_STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Starts a shared client's relay: a queue, and a thread that writes it.
///
/// The thread owns a clone of the client's connection, so the serve loop keeps
/// reading input on the original while output goes out on the clone.
fn spawn_relay(connection: &Connection, client_process_id: u32) -> Result<Relay> {
    let writer = connection.try_clone()?;
    #[cfg(unix)]
    {
        writer
            .stream()
            .set_write_timeout(Some(RELAY_WRITE_TIMEOUT))
            .ok();
    }
    // Unbounded, with `queued` as the only bound: the limit that matters is how
    // many bytes are outstanding, and a channel can only count entries.
    // Unbounded, with `queued` as the only bound: the limit that matters is how
    // many bytes are outstanding, and a channel can only count entries.
    let (sender, frames) = async_channel::unbounded::<Arc<[u8]>>();
    let queued = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicUsize::new(0));
    let (loop_queued, loop_written) = (queued.clone(), written.clone());
    std::thread::Builder::new()
        .name("zmux relay".to_owned())
        .spawn(move || relay_loop(writer, frames, loop_queued, loop_written))
        .with_context(|| format!("starting the relay for client {client_process_id}"))?;
    Ok(Relay {
        frames: sender,
        queued,
        written,
    })
}

/// Writes one shared client's frames, off the sessions lock.
///
/// Ends when the client leaves the pane's shared set — dropping its
/// `SharedClient` drops the sender — or when the socket refuses a write, which
/// is what a viewer that has gone away looks like. [`RELAY_WRITE_TIMEOUT`] still
/// bounds a wedged write, but it now stalls this one client's relay and nothing
/// else.
fn relay_loop(
    mut writer: Connection,
    frames: async_channel::Receiver<Arc<[u8]>>,
    queued: Arc<AtomicUsize>,
    written: Arc<AtomicUsize>,
) {
    while let Ok(frame) = frames.recv_blocking() {
        let result = writer.write_all(&frame);
        queued.fetch_sub(frame.len(), Ordering::Relaxed);
        if result.is_ok() {
            written.fetch_add(frame.len(), Ordering::Relaxed);
        }
        if let Err(error) = result {
            log::debug!("a shared client stopped accepting output: {error:#}");
            break;
        }
    }
    // However this relay ended — retired with the pane, or given up on — the
    // client has to be able to tell that nothing more is coming.
    //
    // Dropping the connection is not enough: the serve loop holds a clone of the
    // same socket, so the client goes on waiting for output that will never
    // arrive. Returning early on a write error skipped this, which is how a viewer
    // the daemon had given up on was left with a half-drawn screen, no message and
    // no end of stream — a pane frozen with nothing to say why.
    let _ = writer.stream().shutdown(std::net::Shutdown::Write);
}

/// Serves one client's shared connection: its input is written to the pane
/// and its size reports feed the pane's size arbitration.
fn serve_shared(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    connection: &mut Connection,
) -> Result<()> {
    loop {
        let (request, _) = connection.receive::<Request>()?;
        match request {
            Request::Input { length } => {
                let bytes = connection.read_exact(length)?;
                let mut sessions = daemon.sessions.lock().unwrap();
                let Some(pane) = sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                    .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
                else {
                    anyhow::bail!("pane {pane_id} no longer exists");
                };
                match &mut pane.attachment {
                    Attachment::Shared(clients) => {
                        let Some(client) = clients
                            .iter_mut()
                            .find(|client| client.process_id == client_process_id)
                        else {
                            anyhow::bail!("client {client_process_id} is not shared on {pane_id}");
                        };
                        // Attribution: which clients typed into this pane is
                        // reported with its exit, because no single shared
                        // client can know what the others typed.
                        client.input_sent = true;
                    }
                    _ => anyhow::bail!("pane {pane_id} is no longer shared"),
                }
                // Queued rather than written outright. The master is
                // non-blocking, so a terminal whose buffer is full accepts part
                // of a write and refuses the rest; `write_all` reports that as
                // an error having already consumed some of the bytes, so
                // logging and moving on silently ate a shared client's
                // keystrokes — reliably, for anything larger than the buffer's
                // free space, such as a paste.
                pane.pending_input.extend_from_slice(&bytes);
                flush_pending_input(pane);
                drop(sessions);
                // Input is what the child reacts to; wake the drain so its
                // reply is relayed promptly rather than on the idle timer, and
                // so it finishes any input the terminal could not take yet.
                wake_drain(daemon);
            }
            Request::Resize { columns, lines, .. } => {
                let mut sessions = daemon.sessions.lock().unwrap();
                let Some(pane) = sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                    .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
                else {
                    anyhow::bail!("pane {pane_id} no longer exists");
                };
                if let Attachment::Shared(clients) = &mut pane.attachment {
                    if let Some(client) = clients
                        .iter_mut()
                        .find(|client| client.process_id == client_process_id)
                    {
                        client.columns = columns;
                        client.lines = lines;
                    }
                    let (columns, lines) = effective_size(pane);
                    if (columns, lines) != (pane.size.columns, pane.size.lines) {
                        apply_size(daemon, pane, columns, lines);
                        broadcast_size(
                            session_id,
                            pane_id,
                            &mut pane.attachment,
                            pane.handover_waiters,
                            columns,
                            lines,
                        );
                    }
                }
            }
            // A shared client that wants to leave the relay without touching
            // the session's state can detach: the connection closes and this
            // client is dropped from the shared set. Unlike [`crate::messages::DetachRequest`],
            // no summary or snapshots are exchanged, because the session
            // belongs to no single client any more.
            Request::Detach(_) => return connection.send(&Response::Detached),
            other => anyhow::bail!("unexpected request on a shared connection: {other:?}"),
        }
    }
}

/// How long to wait for a granted pane's last relayed frames to reach the client
/// before giving up on the handover.
///
/// The wait exists because those frames have to arrive *before* the descriptor
/// does: the client starts reading the terminal itself the moment it has it, and
/// output that was still queued would then arrive after output that came later.
const GRANT_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

/// Hands a shared pane's terminal back to its last remaining viewer.
///
/// The reverse of the revoke handover, and the reason it exists: a pane with one
/// viewer is being read by the daemon and relayed over a socket for no benefit,
/// which costs a quarter of the throughput of the client reading it itself.
///
/// The ordering is the whole difficulty. Everything already read has been queued
/// to this very client, so none of it may be replayed — but all of it has to land
/// before the descriptor does. So the pane stops being read, the queue is allowed
/// to empty, this end of the relay is shut down so the client's reader sees the
/// end of it, and only then does the descriptor go out.
fn take_exclusive(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(pane) = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
    else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    if !matches!(&pane.attachment, Attachment::Shared(clients) if clients.len() == 1 && clients[0].process_id == client_process_id)
    {
        return connection.send(&Response::Error {
            message: format!("pane {pane_id} is not shared with client {client_process_id} alone"),
        });
    }
    if let Err(error) = pause_pane_reader(pane) {
        return connection.send(&Response::Error {
            message: format!("could not stop the pane reader before taking it: {error:#}"),
        });
    }
    // Only the *sole* viewer may take it. With anybody else still attached the
    // pane has to keep being relayed, and handing the descriptor over would stop
    // the others being read to.
    let relay = match &mut pane.attachment {
        Attachment::Shared(clients)
            if clients.len() == 1 && clients[0].process_id == client_process_id =>
        {
            let Attachment::Shared(mut clients) = std::mem::replace(
                &mut pane.attachment,
                Attachment::Granting {
                    holder: client_process_id,
                },
            ) else {
                unreachable!("the attachment was just matched as shared");
            };
            clients.pop().expect("exactly one client was matched")
        }
        _ => {
            #[cfg(windows)]
            pane.pty.resume_reader();
            return connection.send(&Response::Error {
                message: format!(
                    "pane {pane_id} is not shared with client {client_process_id} alone"
                ),
            });
        }
    }
    .relay;
    // Released while the queue drains: the relay thread is what empties it, and
    // it must not be waiting on this lock. Nothing more is queued meanwhile —
    // `drain_reads` is false for a granting pane, and the size broadcast only
    // queues to a shared one.
    drop(sessions);

    let deadline = Instant::now() + GRANT_FLUSH_TIMEOUT;
    while relay.queued.load(Ordering::Relaxed) > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(1));
    }
    let flushed = relay.queued.load(Ordering::Relaxed) == 0;
    // Ends the relay: dropping the sender stops its thread, and closing this end
    // of the socket is what tells the client's reader it has everything.
    drop(relay);

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} went away mid-handover"),
        });
    };
    let summary = Box::new(session.summary.clone());
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} went away mid-handover"),
        });
    };
    if !flushed {
        // The client never drained what it was already sent, so it cannot be
        // trusted to read the terminal either. Leave the pane unheld rather than
        // handing the descriptor to a client that is not reading.
        pane.attachment = Attachment::None;
        #[cfg(windows)]
        pane.pty.resume_reader();
        drop(sessions);
        daemon.sessions_condvar.notify_all();
        publish(daemon);
        wake_drain(daemon);
        return connection.send(&Response::Error {
            message: format!("client {client_process_id} did not drain pane {pane_id}'s relay"),
        });
    }
    pane.attachment = exclusive_attachment(client_process_id);
    // Deliberately no replay. Everything read so far went to this client over the
    // relay; sending it again would print the pane's recent output twice. What is
    // left is still in the terminal, which the client now reads itself.
    pane.retained.clear();
    pane.handed_over = None;
    let child_pid = pane.pty.child_pid();
    let handles = handover_handles(daemon, pane, client_process_id)?;
    // Neither is read on this path: the client already has the tab this pane
    // belongs to and is only swapping what feeds it. They travel because the
    // exclusive attach's reply is the shape that carries a descriptor.
    let state = serde_json::Value::Null;
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Err(error) = forget_persisted_session(daemon, session_id) {
        log::warn!("could not remove the attached session's persisted record: {error:#}");
    }
    connection.send_with(
        &Response::Attached {
            pane_id,
            child_pid,
            replay_length: 0,
            state,
            summary,
            handles: handles.values,
        },
        &handles.attachments,
    )?;
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    Ok(())
}

/// Offers a pane back to its viewer when a *departure* has left it the only one.
///
/// Called where a viewer leaves, and deliberately not from a scan of how many
/// viewers a pane has. A revoke handover passes through one viewer on its way to
/// two — the holder re-attaches before the client that triggered the revoke joins —
/// so a count-based offer raced it: the grant went to the holder mid-handover, and
/// the transition that mattered a moment later was then treated as already offered
/// and never announced at all.
///
/// An offer rather than an instruction: a client that cannot take a pty ignores it
/// and stays shared, which is also what makes it safe to send more than once.
fn offer_exclusive_if_alone(daemon: &Arc<Daemon>, session_id: u64, pane: &Pane) {
    let Attachment::Shared(clients) = &pane.attachment else {
        return;
    };
    let [client] = clients.as_slice() else {
        return;
    };
    let client = client.process_id;
    let grant = Event::Grant {
        session_id,
        pane_id: pane.id,
    };
    let subscriber = daemon
        .subscribers
        .lock()
        .unwrap()
        .get(&client)
        .map(|connection| connection.try_clone());
    if let Some(Ok(mut subscriber)) = subscriber
        && let Err(error) = subscriber.send(&grant)
    {
        log::debug!(
            "offering pane {} back to client {client} failed: {error:#}",
            pane.id
        );
    }
}

/// Drops a client from a pane's shared set, ending shared mode when it was
/// the last one.
fn remove_shared_client(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
) {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return;
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return;
    };
    if let Attachment::Shared(clients) = &mut pane.attachment {
        let before = clients.len();
        clients.retain(|client| client.process_id != client_process_id);
        if clients.is_empty() && pane.handover_waiters == 0 {
            pane.attachment = Attachment::None;
            // Nobody is left to claim the handover, and the next one records
            // its own.
            pane.handed_over = None;
        } else if clients.len() != before {
            // A smaller set may want a bigger pane.
            let (columns, lines) = effective_size(pane);
            if (columns, lines) != (pane.size.columns, pane.size.lines) {
                apply_size(daemon, pane, columns, lines);
                broadcast_size(
                    session_id,
                    pane_id,
                    &mut pane.attachment,
                    pane.handover_waiters,
                    columns,
                    lines,
                );
            }
            // Down to one viewer: relaying to a single client is the daemon doing
            // work the client can do better itself, so offer it the terminal.
            offer_exclusive_if_alone(daemon, session_id, pane);
        }
    }
    drop(sessions);
    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
}

/// The revoke handover's answer: the holder stopped reading the pane and is
/// giving back the screen it was showing.
///
/// The snapshot seeds the retention, the pane becomes shared (even with no
/// clients yet — the revoke is committed), and every attach waiting on the
/// handover is woken to join.
#[allow(clippy::too_many_arguments)]
fn snapshot(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    length: usize,
    columns: u16,
    lines: u16,
    client_process_id: u32,
    connection: &mut Connection,
) -> Result<()> {
    anyhow::ensure!(
        length <= crate::retention::MAX_SNAPSHOT_BYTES,
        "a pane snapshot exceeded the retention limit"
    );
    // The raw bytes can be large; read them before taking the lock.
    let bytes = connection.read_exact(length)?;

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} has no pane {pane_id}"),
        });
    };
    let Attachment::Revoking { holder } = pane.attachment else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} pane {pane_id} is not being handed over"),
        });
    };
    if holder != client_process_id {
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} pane {pane_id} is being handed over by another client"
            ),
        });
    }
    // The holder is still showing this screen, so it is the one client that
    // must not be sent it back.
    pane.handed_over = Some(RevokeHandover {
        client_process_id,
        output: Vec::new(),
    });
    seed_retained_screen(pane, bytes);
    pane.attachment = Attachment::Shared(Vec::new());
    // The holder's size is what it was showing the pane at; shared clients
    // join at that size until their own reports refine it.
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    drop(sessions);
    daemon.sessions_condvar.notify_all();
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Ok)
}

/// The size every shared client must show the pane at: the smallest any of
/// them asked for, falling back to the size the daemon last applied.
fn effective_size(pane: &Pane) -> (u16, u16) {
    match &pane.attachment {
        Attachment::Shared(clients) => smallest_size(
            clients.iter().map(|client| (client.columns, client.lines)),
            pane.size,
        ),
        _ => (pane.size.columns, pane.size.lines),
    }
}

/// The smallest of the sizes asked for, or `fallback` when none were.
///
/// Split out from [`effective_size`] because a `SharedClient` owns a live
/// connection: the arbitration is the part worth testing on its own, and it
/// cannot be tested through a type that needs a socket to exist.
///
/// Independently per axis, as tmux does — the pane has to fit inside every
/// viewer, and a viewer that is wider but shorter constrains only the height.
fn smallest_size(sizes: impl Iterator<Item = (u16, u16)>, fallback: TerminalSize) -> (u16, u16) {
    let mut smallest: Option<(u16, u16)> = None;
    for (columns, lines) in sizes {
        smallest = Some(match smallest {
            Some((best_columns, best_lines)) => (best_columns.min(columns), best_lines.min(lines)),
            None => (columns, lines),
        });
    }
    smallest.unwrap_or((fallback.columns, fallback.lines))
}

/// Applies a size to a pane's terminal, recording it as the pane's size.
fn apply_size(_daemon: &Daemon, pane: &mut Pane, columns: u16, lines: u16) {
    use alacritty_terminal::event::OnResize as _;
    #[cfg(windows)]
    if let Err(error) = _daemon.pty_host.resize(pane.console_id, columns, lines) {
        log::warn!(
            "could not resize pseudoconsole {} to {columns}x{lines}: {error:#}",
            pane.console_id
        );
    }
    pane.pty.on_resize(window_size(TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    }));
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    // What is kept of the pane wraps where the pane wraps, or a reattach would
    // show the session rewrapped at a width nothing was drawn at.
    pane.retained.resize(columns, lines);
}

/// The size the pane's terminal is actually running at.
///
/// Asked of the terminal rather than remembered. A client spawns a pane before
/// its window has laid the pane out, so the size it sends is a stand-in — 80x24 —
/// and on Unix the resize that follows goes straight to the descriptor that
/// client holds, telling the multiplexer nothing. The remembered size is
/// therefore the stand-in for the whole life of an exclusively-held pane, and
/// seeding a 98x51 screen into a grid that size came back rewrapped and
/// interleaved: a full-screen program's screen crushed into 80 columns and 24
/// rows, which is what a joined session looked like.
fn terminal_size(pane: &Pane) -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;

        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is this process's pty master, and `size` is a
        // `winsize` the kernel fills in.
        let read =
            unsafe { libc::ioctl(pane.pty.file().as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
        (read == 0 && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
    }
    #[cfg(not(unix))]
    {
        let _ = pane;
        None
    }
}

/// Gives the retained screen the geometry the handed-over screen was drawn at,
/// then reads that screen into it.
///
/// The two have to agree: a snapshot is lines of text with no width of their own,
/// so a grid of a different width wraps them somewhere the program never did.
fn seed_retained_screen(pane: &mut Pane, snapshot: Vec<u8>) {
    if let Some((columns, lines)) = terminal_size(pane) {
        pane.retained.resize(columns, lines);
    }
    pane.retained.seed(snapshot);
}

/// Keeps a copy of `chunk` for a client that is mid-handover.
///
/// It handed its screen over and has not rejoined yet, so these are the bytes it
/// alone is missing. Bounded, and oldest-first when it overflows: a handover that
/// never completes must not grow without limit.
fn record_handover_output(pane: &mut Pane, chunk: &[u8]) {
    let Some(handover) = pane.handed_over.as_mut() else {
        return;
    };
    handover.output.extend_from_slice(chunk);
    if handover.output.len() > HANDOVER_OUTPUT_LIMIT {
        let excess = handover.output.len() - HANDOVER_OUTPUT_LIMIT;
        handover.output.drain(..excess);
    }
}

/// Tells every shared client the size the pane is now shown at.
fn broadcast_size(
    session_id: u64,
    pane_id: u64,
    attachment: &mut Attachment,
    handover_waiters: usize,
    columns: u16,
    lines: u16,
) {
    if !matches!(attachment, Attachment::Shared(_)) {
        return;
    }
    let event = Event::Size {
        session_id,
        pane_id,
        columns,
        lines,
    };
    // Queued alongside the pane's output rather than written past it, so a
    // viewer applies the new size at the point in the stream where it happened.
    match crate::transport::encode_message(&event) {
        Ok(frame) => queue_for_shared_clients(attachment, handover_waiters, &Arc::from(frame)),
        Err(error) => log::warn!("could not frame a pane's new size: {error:#}"),
    }
}

/// Hands one framed message to every shared client, dropping those that cannot
/// take it.
///
/// `try_send` never blocks: a full queue means this viewer is not keeping up, and
/// making the pane wait for it would stall every other viewer and the drain with
/// it. Collapses the attachment when that leaves nobody.
fn queue_for_shared_clients(
    attachment: &mut Attachment,
    handover_waiters: usize,
    frame: &Arc<[u8]>,
) {
    let Attachment::Shared(clients) = attachment else {
        return;
    };
    let mut failed = Vec::new();
    for client in clients.iter() {
        // No size ceiling here. Dropping a viewer for having a large backlog
        // meant dropping every viewer of any sustained output, because a terminal
        // that parses and renders is always slower than a program that only
        // writes: `zetta benchmark-output --size 1000` cut the viewer off 4 MiB in,
        // every time, and left the pane connected to nothing. The backlog is
        // bounded by `relay_backpressure`, which stops reading the pane instead of
        // punishing whoever is reading it.
        client
            .relay
            .queued
            .fetch_add(frame.len(), Ordering::Relaxed);
        if client.relay.frames.try_send(frame.clone()).is_err() {
            client
                .relay
                .queued
                .fetch_sub(frame.len(), Ordering::Relaxed);
            failed.push(client.process_id);
        }
    }
    if failed.is_empty() {
        return;
    }
    log::debug!(
        "dropped {} shared client(s) that stopped keeping up",
        failed.len()
    );
    clients.retain(|client| !failed.contains(&client.process_id));
    collapse_empty_shared(attachment, handover_waiters);
}

/// The exclusive attachment a client process id maps to. A client that does
/// not identify itself (`0`) is recorded as holding nothing, which preserves
/// the original behaviour for test paths that do not name a process.
fn exclusive_attachment(client_process_id: u32) -> Attachment {
    match client_process_id {
        0 => Attachment::None,
        process_id => Attachment::Exclusive(process_id),
    }
}

/// How long an attach waits for a pane's holder to answer a revoke before
/// giving up. Generous: the holder has to stop its terminal, snapshot a large
/// screen, and re-attach, all between two network round trips.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

fn resume(
    daemon: &Arc<Daemon>,
    request: crate::messages::ResumeRequest,
    _client_process_id: u32,
    _peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    #[cfg(not(feature = "session-persistence"))]
    {
        let _ = (daemon, request);
        connection.send(&Response::Error {
            message: "resume needs the session-persistence feature".to_owned(),
        })
    }
    #[cfg(feature = "session-persistence")]
    {
        let mut request = request;
        // The metadata frame intentionally contains lengths only. Read the
        // potentially large screen bytes through the raw side of the same
        // connection, just as detach does, so a record's scrollback cannot
        // exceed the control-message ceiling.
        let mut snapshots = HashMap::new();
        for snapshot in &request.snapshots {
            anyhow::ensure!(
                snapshot.length <= crate::retention::MAX_SNAPSHOT_BYTES,
                "a pane snapshot exceeded the retention limit"
            );
            snapshots.insert(snapshot.pane_id, connection.read_exact(snapshot.length)?);
        }
        anyhow::ensure!(
            daemon
                .persistence
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|persistence| {
                    persistence
                        .records()
                        .iter()
                        .any(|record| record.id == request.record_id && record.restorable)
                }),
            "restorable session {} does not exist",
            request.record_id
        );
        anyhow::ensure!(
            !daemon
                .sessions
                .lock()
                .unwrap()
                .iter()
                .any(|session| session.id == request.record_id),
            "session {} is already live",
            request.record_id
        );
        if let Some(verifier) = request.verifier.as_ref() {
            let authentication = SessionAuthentication::from_verifier(verifier.clone())?;
            let elapsed = unix_now().saturating_sub(request.updated_at);
            if request.backoff_seconds > elapsed {
                return connection.send(&Response::AuthenticationFailed);
            }
            let Some(secret) = request.secret.as_deref() else {
                return connection.send(&Response::AuthenticationRequired);
            };
            if authentication.verify(secret).is_none() {
                request.secret = None;
                request.failed_authentications = request.failed_authentications.saturating_add(1);
                request.backoff_seconds =
                    crate::auth::failed_authentication_delay(request.failed_authentications)
                        .as_secs();
                request.updated_at = unix_now();
                if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
                    persistence
                        .update_session(&persisted_from_resume_request(&request, &snapshots))?;
                }
                return connection.send(&Response::AuthenticationFailed);
            }
        }
        // The secret is only a request credential. It must not survive in the
        // daemon's restored-session memory after the verifier has been checked.
        request.secret = None;
        if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
            persistence.forget(request.record_id)?;
        }
        daemon.restored.lock().unwrap().push(RestoredSession {
            request,
            restored_at: unix_now(),
        });
        connection.send(&Response::Resumed {
            session_id: daemon
                .restored
                .lock()
                .unwrap()
                .last()
                .map(|restored| restored.request.record_id)
                .unwrap_or_default(),
        })
    }
}

#[cfg(feature = "session-persistence")]
fn persisted_from_resume_request(
    request: &crate::messages::ResumeRequest,
    snapshots: &HashMap<u64, Vec<u8>>,
) -> PersistedSession {
    PersistedSession {
        id: request.record_id,
        created_at: request.created_at,
        updated_at: request.updated_at,
        summary: request.summary.clone(),
        state: request.state.clone(),
        verifier: request.verifier.clone(),
        failed_authentications: request.failed_authentications,
        backoff_seconds: request.backoff_seconds,
        snapshots: request
            .snapshots
            .iter()
            .filter_map(|snapshot| {
                snapshots
                    .get(&snapshot.pane_id)
                    .map(|bytes| PersistedSnapshot {
                        pane_id: snapshot.pane_id,
                        bytes: bytes.clone(),
                    })
            })
            .collect(),
    }
}

/// How long a shared client's socket write may stall before its relay gives up
/// and stops relaying to it.
///
/// A backstop, and deliberately the same patience as [`RELAY_STALL_TIMEOUT`]: both
/// answer "this viewer has accepted nothing for a while", so they should not
/// disagree about how long a while is. When this was five times more patient it won
/// the race often enough to matter — a viewer that had stopped reading held its
/// pane for four and a half seconds instead of the one the stall rule intends,
/// because the write blocked before the backlog had grown enough to be noticed.
///
/// It bounds that one client's relay thread and nothing else. It used to bound the
/// *drain* thread, because the relay wrote to every viewer's socket while holding
/// the sessions lock — so a viewer that stopped reading froze every pane the daemon
/// holds. The queue in [`SharedClient::relay`] is what took the drain out of that
/// path.
#[cfg(unix)]
const RELAY_WRITE_TIMEOUT: Duration = RELAY_STALL_TIMEOUT;

fn detach(
    daemon: &Arc<Daemon>,
    request: crate::messages::DetachRequest,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    // Read the snapshots before taking the lock: they arrive as raw bytes on
    // this connection and can be large.
    let mut snapshots = HashMap::new();
    for snapshot in &request.snapshots {
        anyhow::ensure!(
            snapshot.length <= crate::retention::MAX_SNAPSHOT_BYTES,
            "a pane snapshot exceeded the retention limit"
        );
        snapshots.insert(snapshot.pane_id, connection.read_exact(snapshot.length)?);
    }
    #[cfg(feature = "session-persistence")]
    let persisted_snapshots = snapshots
        .iter()
        .map(|(&pane_id, bytes)| PersistedSnapshot {
            pane_id,
            bytes: bytes.clone(),
        })
        .collect::<Vec<_>>();

    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
    else {
        return connection.send(&Response::Error {
            message: format!("session {} does not exist", request.session_id),
        });
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id),
        "session {} is protected and can only be detached by its owner or current holder",
        request.session_id
    );
    // The session's identity belongs to the daemon, not to whatever the
    // client puts in the summary. A client that published a summary whose id
    // disagreed with the session's own id (a tab id instead of the mux
    // session id, say) would make the catalog list a session under one id
    // while attach, kill and resize looked for it under another: listed, but
    // neither attachable nor killable.
    session.summary = request.summary;
    session.summary.id = session.id;
    session.state = request.state;
    session.keep = true;
    // A plain detach is private, but a tab that was already shared asked for
    // both properties: keep it after this window goes and leave it available to
    // another process. Preserve the offer through the handoff so closing a
    // keep-running tab does not narrow it back to the process that just closed.
    if !session.offered {
        session.owner = Some(client_process_id);
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.failed_authentications = 0;
        session.refuse_until = None;
    }
    session.summary.authentication_required = session.authentication.is_some();
    for pane in &mut session.panes {
        // Only this client lets go. Clearing every attachment evicted whichever
        // *other* windows were sharing the pane — silently, since they learn
        // nothing until their relay stops — and let this client's snapshot
        // overwrite a screen those windows were still driving. A pane the
        // detaching client was not holding is simply left alone.
        match pane.attachment {
            Attachment::Exclusive(holder) | Attachment::Revoking { holder }
                if holder == client_process_id =>
            {
                pane.attachment = Attachment::None;
                #[cfg(windows)]
                pane.pty.resume_reader();
            }
            // A shared viewer detaching the session gives up the session, not
            // the relay; its own connection closing is what removes it from the
            // shared set.
            Attachment::Shared(_) => continue,
            Attachment::None => {}
            // Held by somebody else, so not this client's to release.
            _ => continue,
        }
        if let Some(snapshot) = snapshots.remove(&pane.id) {
            seed_retained_screen(pane, snapshot);
        }
    }
    #[cfg(feature = "session-persistence")]
    let persisted = daemon
        .persistence
        .lock()
        .unwrap()
        .is_some()
        .then(|| PersistedSession {
            id: session.id,
            created_at: unix_now(),
            updated_at: unix_now(),
            summary: session.summary.clone(),
            state: session.state.clone(),
            verifier: session
                .authentication
                .as_ref()
                .map(|authentication| authentication.verifier().to_owned()),
            failed_authentications: session.failed_authentications,
            backoff_seconds: session
                .refuse_until
                .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
                .unwrap_or_default(),
            snapshots: persisted_snapshots,
        });
    #[cfg(feature = "session-persistence")]
    if let Some(persisted) = persisted {
        persist_session(daemon, &persisted)?;
    }
    drop(sessions);

    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Detached)
}

/// Offers, or withdraws, a session its client is still showing.
///
/// Everything a joining client needs is published here, exactly as a detach
/// publishes it: the summary the catalog lists and the state a joining client
/// rebuilds its tab from. What is *not* done is the rest of detaching — no
/// snapshot is taken, no attachment is released, and `keep` is left alone — so
/// the session carries on being displayed by the window that shared it.
fn share(
    daemon: &Arc<Daemon>,
    request: crate::messages::ShareRequest,
    client_process_id: u32,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions
        .iter_mut()
        .find(|session| session.id == request.session_id)
    else {
        return connection.send(&Response::Error {
            message: format!("session {} does not exist", request.session_id),
        });
    };
    // Only a client that is showing the session may offer it. Two reasons, and
    // the second is the one that matters: a client cannot describe a session it
    // is not displaying, and this request rewrites the verifier, so letting any
    // client send it would be a way to reprotect a session without knowing the
    // secret it already has.
    // A protected session's holder has to be an identity something vouched
    // for. Taking the client's word for it would let anyone who can reach the
    // socket name the real holder and rewrite the verifier from there.
    let holder_process_id = if session.authentication.is_some() {
        peer_process_id
    } else {
        Some(client_process_id)
    };
    if !holder_process_id.is_some_and(|process_id| session_is_held_by(session, process_id)) {
        return connection.send(&Response::Error {
            message: format!(
                "session {} cannot be shared by a client that is not showing it",
                request.session_id
            ),
        });
    }
    // Withdrawing means scoping the session back to one window, and that can only
    // be done while one window has it. There is no way to take a pane away from a
    // viewer that is still relaying it — a grant goes to the *last* viewer, not to
    // a chosen one — so a request that cannot be honoured is refused rather than
    // half-applied. Half-applying it was the old behaviour: the session stopped
    // being listed while other windows carried on driving it, so "not shared" and
    // "shared" looked the same from the tab that asked.
    if !request.offered
        && let Some(pane) = session
            .panes
            .iter()
            .find(|pane| shared_viewer_count(&pane.attachment) > 1)
    {
        let viewers = shared_viewer_count(&pane.attachment);
        return connection.send(&Response::Error {
            message: format!(
                "this session is still open in {} windows, so it cannot be scoped back to one;                  close it in the others first",
                viewers
            ),
        });
    }
    #[cfg(feature = "session-persistence")]
    if !request.offered {
        forget_persisted_session(daemon, request.session_id)?;
    }
    // The session's identity belongs to the daemon, as in `detach`: a summary
    // published under a tab id rather than the mux session id would be listed
    // under one id and attachable under another.
    session.summary = request.summary;
    session.summary.id = session.id;
    session.state = request.state;
    session.offered = request.offered;
    if !request.offered {
        // Scoped back to the window that asked, which the check above has
        // established is the one showing it.
        session.owner = Some(client_process_id);
    }
    if let Some(verifier) = request.verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.failed_authentications = 0;
        session.refuse_until = None;
    }
    session.summary.authentication_required = session.authentication.is_some();
    #[cfg(feature = "session-persistence")]
    if request.offered {
        persist_session(daemon, &persisted_live_session(session))?;
    }
    if !request.offered {
        // Scoped to one window now, so a pane still being relayed to its single
        // viewer should stop being relayed. Offered from here rather than left to a
        // departure, because this *is* the moment the session became one window's.
        for pane in &session.panes {
            offer_exclusive_if_alone(daemon, request.session_id, pane);
        }
    }
    drop(sessions);

    publish(daemon);
    wake_drain(daemon);
    connection.send(&Response::Ok)
}

/// Scopes a session to one process, or shares it with every process.
///
/// The CLI's half of the sharing toggle, for a session in the background. It is
/// deliberately not [`share`]: that request comes from the window displaying the
/// session and rewrites what the catalog says about it, so it insists the caller
/// is showing it. Nobody is showing a backgrounded session, so this asks only for
/// the flag — and, because the caller is a command that exits a moment later,
/// scoping back means scoping to the process that last held the session rather
/// than to whoever asked.
fn set_session_scope(
    daemon: &Arc<Daemon>,
    session_id: u64,
    shared: bool,
    verifier: Option<String>,
    peer_process_id: Option<u32>,
    connection: &mut Connection,
) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        return connection.send(&Response::Error {
            message: format!("session {session_id} does not exist"),
        });
    };
    // A session on screen has a window that can toggle this itself, and that
    // route enforces a rule this one cannot: a session may only be scoped back
    // while a single window has it. Refusing here keeps the two from
    // disagreeing about a session several windows are driving.
    if session.panes.iter().any(|pane| !pane.attachment.is_none()) {
        return connection.send(&Response::Error {
            message: format!(
                "session {session_id} is on screen; share or unshare it from the window showing it"
            ),
        });
    }
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id),
        "session {session_id} is protected and can only be changed by its owner or current holder"
    );
    if let Some(verifier) = verifier {
        session.authentication = Some(SessionAuthentication::from_verifier(verifier)?);
        session.failed_authentications = 0;
        session.refuse_until = None;
        session.summary.authentication_required = true;
    }
    if !shared {
        let Some(owner) = session.owner.filter(|owner| process_is_running(*owner)) else {
            return connection.send(&Response::Error {
                message: format!(
                    "session {session_id} has no window to scope it back to; attach it first, \
                     and it becomes that window's again"
                ),
            });
        };
        session.owner = Some(owner);
    }
    session.offered = shared;
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence.save_session(&persisted_live_session(session))?;
    }
    drop(sessions);
    publish(daemon);
    connection.send(&Response::Ok)
}

/// The identity a control request acts as: what the transport vouched for, or
/// the envelope's own claim where nothing could.
///
/// Only for checks whose failure is inconvenient rather than a privilege
/// boundary — "are you the window holding this pane" for a session with no
/// secret, which any same-user process could attach for itself anyway. A
/// protected session's controls go through [`session_control_authorized`] or
/// [`protected_holder_authorized`], neither of which accepts a claim.
fn control_process_id(client_process_id: u32, peer_process_id: Option<u32>) -> u32 {
    peer_process_id.unwrap_or(client_process_id)
}

/// Whether a protected session may be acted on by this peer as one of the
/// clients currently showing it.
///
/// Distinct from [`session_control_authorized`] in refusing the owner: resizing
/// a pane or repainting its palette is something only a window displaying it
/// can sensibly ask for.
fn protected_holder_authorized(session: &Session, peer_process_id: Option<u32>) -> bool {
    peer_process_id.is_some_and(|process_id| session_is_held_by(session, process_id))
}

fn session_control_authorized(session: &Session, peer_process_id: Option<u32>) -> bool {
    if session.authentication.is_none() {
        return true;
    }
    // Nothing vouched for this peer, so there is no identity to authorize. The
    // envelope's own value is not a fallback: a same-user process can read the
    // endpoint token and would otherwise only have to *name* the owner of a
    // protected session to act as it.
    let Some(process_id) = peer_process_id else {
        return false;
    };
    session.owner == Some(process_id) || session_is_held_by(session, process_id)
}

/// Whether this request's answer could depend on who the peer is, on a platform
/// that cannot tell without asking.
///
/// Narrow on purpose, because asking costs a round trip: only requests that can
/// act on a session somebody else owns, and only when the session they name is
/// actually protected. Nothing else has an authorization decision to make — a
/// session with no secret can be attached by any same-user process for itself,
/// so confirming which process is asking would protect nothing.
/// `Detach` and `Resume` are absent on purpose: they stream raw bytes after
/// their message, so there is nowhere to interject a challenge. Those ask to be
/// identified first, with [`Request::Attest`].
#[cfg(windows)]
fn attestation_needed(daemon: &Arc<Daemon>, request: &Request) -> bool {
    let session_id = match request {
        Request::Share(request) => Some(request.session_id),
        Request::Resize { session_id, .. }
        | Request::SetConsolePalette { session_id, .. }
        | Request::ClosePane { session_id, .. }
        | Request::Kill { session_id }
        | Request::SetSessionScope { session_id, .. }
        | Request::Forget { session_id } => Some(*session_id),
        // Names panes rather than a session, and whose sessions those are is
        // exactly what it must not disclose, so it is decided against whatever
        // protected sessions exist.
        Request::PaneStates { .. } => None,
        _ => return false,
    };
    let sessions = daemon.sessions.lock().unwrap();
    match session_id {
        Some(session_id) => sessions
            .iter()
            .any(|session| session.id == session_id && session.authentication.is_some()),
        None => sessions
            .iter()
            .any(|session| session.authentication.is_some()),
    }
}

/// Asks the peer to prove it is the process its envelope named.
///
/// Answered means the identity is as good as a kernel-reported one for this
/// connection; unanswered, or answered wrongly, means the request goes ahead
/// with no identity and every protected-session check refuses it.
#[cfg(windows)]
fn attest_peer(connection: &mut Connection, client_process_id: u32) -> Result<Option<u32>> {
    /// Bounded, unlike most of the daemon's reads: until the answer arrives or
    /// the attempt is abandoned, the challenge is holding a handle inside
    /// another process. A client that names somebody else and then says nothing
    /// must not be able to leave it there.
    const ANSWER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    let challenge = crate::transport::PeerChallenge::issue(client_process_id)?;
    connection.send(&Response::AttestationRequired {
        handle: challenge.handle(),
    })?;
    connection.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    let answer = connection
        .receive::<Request>()
        .context("reading a peer attestation answer");
    connection.set_read_timeout(None)?;
    let Request::Attested { nonce } = answer?.0 else {
        anyhow::bail!("expected an attestation answer");
    };
    Ok(challenge.matches(&nonce).then_some(client_process_id))
}

/// How many windows are being relayed this pane. Zero for a pane one window holds
/// outright, which is the state unsharing is trying to reach.
fn shared_viewer_count(attachment: &Attachment) -> usize {
    match attachment {
        Attachment::Shared(clients) => clients.len(),
        Attachment::None
        | Attachment::Exclusive(_)
        | Attachment::Revoking { .. }
        | Attachment::Granting { .. } => 0,
    }
}

/// Whether this client is one of the clients showing any of the session's panes.
fn session_is_held_by(session: &Session, client_process_id: u32) -> bool {
    session
        .panes
        .iter()
        .any(|pane| pane_is_held_by(pane, client_process_id))
}

fn pane_is_held_by(pane: &Pane, client_process_id: u32) -> bool {
    match &pane.attachment {
        Attachment::Exclusive(holder)
        | Attachment::Revoking { holder }
        | Attachment::Granting { holder } => *holder == client_process_id,
        Attachment::Shared(clients) => clients
            .iter()
            .any(|client| client.process_id == client_process_id),
        Attachment::None => false,
    }
}

/// Applies a client's new size to a pane's terminal, recording it as the
/// pane's size so a later shared attach starts from it.
fn resize_pane(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    columns: u16,
    lines: u16,
    peer_process_id: Option<u32>,
) -> Result<()> {
    use alacritty_terminal::event::OnResize as _;
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    if session.authentication.is_some() && !protected_holder_authorized(session, peer_process_id) {
        anyhow::bail!(
            "session {session_id} is protected and can only be resized by its current holder"
        );
    }
    let Some(pane) = sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .and_then(|session| session.panes.iter_mut().find(|pane| pane.id == pane_id))
    else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    pane.pty.on_resize(window_size(TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    }));
    #[cfg(windows)]
    daemon
        .pty_host
        .resize(pane.console_id, columns, lines)
        .context("resizing the pseudoconsole")?;
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    // What is kept of the pane wraps where the pane wraps, or a reattach would
    // show the session rewrapped at a width nothing was drawn at.
    pane.retained.resize(columns, lines);
    Ok(())
}

fn set_console_palette(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    palette: ConsolePalette,
    client_process_id: u32,
    peer_process_id: Option<u32>,
) -> Result<()> {
    let sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    let Some(pane) = session.panes.iter().find(|pane| pane.id == pane_id) else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    // As with resizing: for a protected session only a vouched-for holder, and
    // for one with no secret the claim it always was.
    let holder_process_id = if session.authentication.is_some() {
        peer_process_id
    } else {
        Some(control_process_id(client_process_id, peer_process_id))
    };
    if !holder_process_id.is_some_and(|process_id| pane_is_held_by(pane, process_id)) {
        anyhow::bail!("pane {pane_id} is not held by this client");
    }
    #[cfg(windows)]
    daemon
        .pty_host
        .set_console_palette(pane.console_id, palette)
        .context("updating the pseudoconsole palette")?;
    #[cfg(not(windows))]
    let _ = (daemon, pane, palette);
    Ok(())
}

fn kill(daemon: &Arc<Daemon>, session_id: u64, peer_process_id: Option<u32>) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id),
        "session {session_id} is protected and can only be ended by its owner or current holder"
    );
    // Dropping the panes drops their PTYs, which hangs up the children.
    #[cfg(windows)]
    let consoles = session
        .panes
        .iter()
        .map(|pane| pane.console_id)
        .collect::<Vec<_>>();
    sessions.retain(|session| session.id != session_id);
    drop(sessions);
    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence.forget(session_id)?;
    }
    #[cfg(windows)]
    for console_id in consoles {
        close_host_console(daemon, console_id);
    }
    Ok(())
}

fn forget(daemon: &Arc<Daemon>, session_id: u64, peer_process_id: Option<u32>) -> Result<()> {
    let mut sessions = daemon.sessions.lock().unwrap();
    if let Some(session) = sessions.iter().find(|session| session.id == session_id) {
        anyhow::ensure!(
            session_control_authorized(session, peer_process_id),
            "session {session_id} is protected and can only be forgotten by its owner or current holder"
        );
        #[cfg(windows)]
        let consoles = session
            .panes
            .iter()
            .map(|pane| pane.console_id)
            .collect::<Vec<_>>();
        sessions.retain(|session| session.id != session_id);
        drop(sessions);
        #[cfg(feature = "session-persistence")]
        if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
            persistence.forget(session_id)?;
        }
        #[cfg(windows)]
        for console_id in consoles {
            close_host_console(daemon, console_id);
        }
        return Ok(());
    }
    drop(sessions);

    #[cfg(feature = "session-persistence")]
    {
        let has_record = daemon
            .persistence
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|persistence| {
                persistence
                    .records()
                    .iter()
                    .any(|record| record.id == session_id)
            });
        if has_record {
            if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
                persistence.forget(session_id)?;
            }
            return Ok(());
        }
        let mut restored = daemon.restored.lock().unwrap();
        if restored
            .iter()
            .any(|session| session.request.record_id == session_id)
        {
            restored.retain(|session| session.request.record_id != session_id);
            return Ok(());
        }
    }

    anyhow::bail!("session {session_id} does not exist")
}

/// Reports what is known about each requested pane, in the order asked.
///
/// A pane the daemon is not holding is reported as `unknown` rather than
/// omitted: the caller is a client trying to find out whether it may stop
/// waiting, and "I have never heard of it" and "it is still running" must not
/// look the same to it.
fn pane_states(
    daemon: &Arc<Daemon>,
    pane_ids: &[u64],
    peer_process_id: Option<u32>,
) -> Vec<crate::messages::PaneStateReport> {
    let sessions = daemon.sessions.lock().unwrap();
    pane_ids
        .iter()
        .map(|&pane_id| {
            let session = sessions
                .iter()
                .find(|session| session.panes.iter().any(|pane| pane.id == pane_id));
            if session.is_some_and(|session| {
                session.authentication.is_some()
                    && !session_control_authorized(session, peer_process_id)
            }) {
                return crate::messages::PaneStateReport {
                    pane_id,
                    unknown: true,
                    exited: false,
                    raw_status: None,
                    input_sent: false,
                };
            }
            let pane =
                session.and_then(|session| session.panes.iter().find(|pane| pane.id == pane_id));
            match pane {
                Some(pane) => crate::messages::PaneStateReport {
                    pane_id,
                    unknown: false,
                    exited: pane.exited,
                    raw_status: pane.exit_status,
                    input_sent: shared_input_sent(&pane.attachment),
                },
                None => crate::messages::PaneStateReport {
                    pane_id,
                    unknown: true,
                    exited: false,
                    raw_status: None,
                    input_sent: false,
                },
            }
        })
        .collect()
}

/// Hands a pane back because the client showing it has closed that pane.
///
/// Only the holder may do this, and only for the pane it holds: a client that
/// could release another's pane could stop that client's terminal from ever
/// being read again. Once released the pane is drained like any unheld one, and
/// a session that nobody asked to keep and that nobody is holding any more ends
/// with the window it belonged to — the same rule the liveness reclaim applies,
/// reached here promptly instead of on a two-second timer.
fn close_pane(
    daemon: &Arc<Daemon>,
    session_id: u64,
    pane_id: u64,
    client_process_id: u32,
    peer_process_id: Option<u32>,
) -> Result<()> {
    let client_process_id = control_process_id(client_process_id, peer_process_id);
    let mut sessions = daemon.sessions.lock().unwrap();
    let Some(session) = sessions.iter_mut().find(|session| session.id == session_id) else {
        anyhow::bail!("session {session_id} does not exist");
    };
    anyhow::ensure!(
        session_control_authorized(session, peer_process_id),
        "session {session_id} is protected and can only be changed by its owner or current holder"
    );
    let Some(pane) = session.panes.iter_mut().find(|pane| pane.id == pane_id) else {
        anyhow::bail!("session {session_id} has no pane {pane_id}");
    };
    match pane.attachment {
        Attachment::Exclusive(holder) | Attachment::Revoking { holder }
            if holder == client_process_id =>
        {
            pane.attachment = Attachment::None;
        }
        // A shared client leaves by closing its own data-plane connection,
        // which is what `remove_shared_client` acts on; releasing the whole
        // pane here would evict the other viewers too.
        _ => anyhow::bail!("client {client_process_id} does not hold pane {pane_id}"),
    }
    let (released, _consoles) = end_abandoned_sessions(&mut sessions);
    drop(sessions);
    #[cfg(windows)]
    for console_id in _consoles {
        close_host_console(daemon, console_id);
    }
    // An attach may have been waiting on this pane's revoke handover.
    daemon.sessions_condvar.notify_all();
    prune_exited_panes(daemon);
    publish(daemon);
    wake_drain(daemon);
    if released {
        log::debug!("ended a session whose last pane was closed with its window");
    }
    Ok(())
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
        failed_authentications: session.failed_authentications,
        backoff_seconds: session
            .refuse_until
            .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
            .unwrap_or_default(),
        snapshots: session
            .panes
            .iter()
            .map(|pane| PersistedSnapshot {
                pane_id: pane.id,
                bytes: pane.retained.snapshot(),
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
    subscribers.retain(|_, subscriber| subscriber.send(event).is_ok());
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

/// Reaps children and tells whoever is holding their terminals.
///
/// Every `Pty` registers its own `SIGCHLD` pipe, so on any child's exit each
/// pane is asked in turn and only the one that actually exited answers.
#[cfg(unix)]
fn start_reaper(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux reaper", move || {
        let (signals, reader) = Stream::pair().expect("creating the SIGCHLD pipe");
        signal_hook::low_level::pipe::register(libc::SIGCHLD, signals)
            .expect("watching for terminal process exits");
        let daemon = daemon.clone();
        Box::new(move || reaper_loop(daemon, reader))
    });
    Ok(())
}

#[cfg(unix)]
fn reaper_loop(daemon: Arc<Daemon>, mut reader: Stream) {
    let mut byte = [0; 1];
    while reader.read(&mut byte).is_ok() {
        let mut exits = Vec::new();
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if pane.exited {
                        continue;
                    }
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        exits.push(exit);
                    }
                }
            }
        }
        for (session_id, pane_id, raw_status, input_sent) in exits {
            broadcast(
                &daemon,
                &Event::PaneExited {
                    session_id,
                    pane_id,
                    raw_status,
                    input_sent,
                },
            );
        }
        // A pane that exited while detached has nothing left to attach to:
        // end the session rather than keep offering a dead terminal. A
        // pane a client is still reading is kept until it lets go, which
        // the reclaim and detach paths then prune.
        if prune_exited_panes(&daemon) {
            publish(&daemon);
        }
    }
}

/// Notices that a pane's process ended, and records what was observed.
///
/// Returns the exit to broadcast, or `None` if the pane is still running. The
/// status is stored on the pane as well as broadcast, because a broadcast
/// reaches whoever is listening at that instant and only the parent can ever
/// observe a status — so a client that was not listening has no other way back
/// to it.
///
/// Shared by both platforms' reapers and by the drain thread's recovery sweep,
/// so "marked exited" and "status recorded" cannot drift apart.
fn observe_pane_exit(session_id: u64, pane: &mut Pane) -> Option<(u64, u64, Option<i32>, bool)> {
    if pane.exited {
        return None;
    }
    let raw_status = match pane.pty.next_child_event() {
        Some(ChildEvent::Exited(status)) => exit_status_raw(status),
        // The child ended but its status could not be obtained. Still an exit:
        // whoever is showing the pane has to be told, or it waits forever.
        Some(_) => None,
        None => return None,
    };
    pane.exited = true;
    pane.exit_status = raw_status;
    Some((
        session_id,
        pane.id,
        raw_status,
        shared_input_sent(&pane.attachment),
    ))
}

/// Whether any shared client typed into a pane. Only the shared data plane
/// can know: input from shared clients travels through the daemon, whereas an
/// exclusive client types through its own descriptor and its own keystrokes
/// are the truth for it.
fn shared_input_sent(attachment: &Attachment) -> bool {
    match attachment {
        Attachment::Shared(clients) => clients.iter().any(|client| client.input_sent),
        _ => false,
    }
}

/// The exit as the client will report it.
///
/// Unix carries a wait status and Windows an exit code; each platform's
/// terminal reads it back as its own, so no translation happens in between.
#[cfg(unix)]
fn exit_status_raw(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt as _;
    Some(status.into_raw())
}

#[cfg(windows)]
fn exit_status_raw(status: std::process::ExitStatus) -> Option<i32> {
    status.code()
}

/// Drains panes no client is reading directly.
///
/// A detached pane still has to be read or its child blocks on a full buffer,
/// so this runs whatever the retention setting is; only what is *kept* differs.
/// A shared pane is drained here too — it is the daemon, not any client, that
/// reads it — and what is read is relayed to every shared client, which is
/// the shared mode's data plane.
fn start_drain(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux drain", move || {
        let (wake, waker) = Stream::pair().expect("creating the drain wake channel");
        waker
            .set_nonblocking(true)
            .expect("making the drain wake channel non-blocking");
        *daemon.drain_wake.lock().unwrap() = Some(wake);
        let daemon = daemon.clone();
        Box::new(move || drain_loop(daemon, waker))
    });
    Ok(())
}

fn drain_loop(daemon: Arc<Daemon>, mut waker: Stream) {
    let mut buffer = vec![0; 16 * 1024];
    let mut evicted = false;
    let mut last_liveness_check = std::time::Instant::now();
    let mut instant_idle_waits = 0u32;
    loop {
        // A client that exits without detaching leaves its panes marked
        // as taken, and a pane nobody reads blocks its program as soon as
        // the terminal's buffer fills. Checking periodically rather than
        // per iteration keeps this off the hot path.
        // The same cadence covers a second recovery. The reaper is woken by
        // `SIGCHLD` and then asks each pane whether it was the one that ended,
        // which needs that pane's own signal byte to have been written already —
        // and that write is not ordered against the reaper's wake. So an exit
        // can be missed, and with a single pane there is no later signal to
        // notice it on, leaving a terminal waiting forever for an exit that had
        // already happened. Sweeping on a timer is what stops the exit path
        // depending on winning that race.
        //
        // Deliberately not every drain tick: this costs a syscall per pane and
        // the drain runs at fifty hertz, whereas recovering a rare missed signal
        // a second or two late is indistinguishable from recovering it at once.
        let mut missed_exits = Vec::new();
        if last_liveness_check.elapsed() >= CLIENT_LIVENESS_INTERVAL {
            last_liveness_check = std::time::Instant::now();
            reclaim_panes_from_departed_clients(&daemon);
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        missed_exits.push(exit);
                    }
                }
            }
        }
        let mut idle = true;
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                let session_id = session.id;
                for pane in session.panes.iter_mut() {
                    if !drain_reads(&pane.attachment) {
                        continue;
                    }
                    // Whatever the terminal could not take when it arrived.
                    if !pane.pending_input.is_empty() {
                        flush_pending_input(pane);
                        idle = false;
                    }
                    if pane.exited {
                        // The child is gone, but the master still holds
                        // what it wrote while dying — the pane's last
                        // lines. Read those out now: once a pane is marked
                        // exited the reaper has already broadcast its
                        // PaneExited, and nobody else will ever drain it.
                        loop {
                            match read_pane(&mut pane.pty, &mut buffer) {
                                Ok(0) | Err(_) => break,
                                Ok(read) => {
                                    pane.retained.push(&buffer[..read]);
                                    #[cfg(feature = "session-persistence")]
                                    record_persistence_output(
                                        &daemon,
                                        session_id,
                                        pane.id,
                                        &buffer[..read],
                                    );
                                    record_handover_output(pane, &buffer[..read]);
                                    relay_output(pane, &buffer[..read]);
                                }
                            }
                        }
                        continue;
                    }
                    // Held off while a viewer catches up. The wait below returns
                    // at once for a pane whose terminal is readable, and the
                    // instant-wait backoff turns that into a re-check about every
                    // millisecond — fine grained next to the threshold.
                    let held_off = relay_backpressure(pane, &mut evicted);
                    if evicted {
                        // A viewer dropped for not reading can leave one behind,
                        // and that is a departure like any other.
                        offer_exclusive_if_alone(&daemon, session_id, pane);
                        evicted = false;
                    }
                    if held_off {
                        continue;
                    }
                    // Read the pane out as far as the buffer goes before
                    // relaying any of it. One relayed frame per *read* made the
                    // frame count follow how eagerly the drain wakes rather than
                    // how much output there is — and since the wait became
                    // event-driven, that is once every few bytes. A burst then
                    // arrived as thousands of tiny frames, each with its own
                    // header, socket write and parse at the far end, which cost
                    // the relay far more than it cost to produce and made every
                    // viewer look like it could not keep up.
                    //
                    // Non-blocking on both platforms, so this returns promptly
                    // whether or not the pane has more to give.
                    let mut filled = 0;
                    while filled < buffer.len() {
                        match read_pane(&mut pane.pty, &mut buffer[filled..]) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => filled += read,
                        }
                    }
                    if filled > 0 {
                        pane.retained.push(&buffer[..filled]);
                        #[cfg(feature = "session-persistence")]
                        record_persistence_output(&daemon, session_id, pane.id, &buffer[..filled]);
                        record_handover_output(pane, &buffer[..filled]);
                        idle = false;
                        relay_output(pane, &buffer[..filled]);
                    }
                }
            }
        }
        if !missed_exits.is_empty() {
            for (session_id, pane_id, raw_status, input_sent) in missed_exits {
                log::debug!(
                    "pane {pane_id}'s exit was noticed by the drain rather than the reaper"
                );
                broadcast(
                    &daemon,
                    &Event::PaneExited {
                        session_id,
                        pane_id,
                        raw_status,
                        input_sent,
                    },
                );
            }
            if prune_exited_panes(&daemon) {
                publish(&daemon);
            }
            continue;
        }
        if idle {
            // A pty whose child has gone reports hangup continuously until the
            // reaper marks the pane exited, so a wait over it returns at once,
            // every time, with nothing to read. Only a *run* of such waits is
            // that case: a wait cut short by real output is followed by a pass
            // that reads something, which clears this.
            if instant_idle_waits >= INSTANT_IDLE_WAITS_BEFORE_BACKING_OFF {
                thread::sleep(HANGUP_BACKOFF);
            }
            let started = Instant::now();
            wait_for_drainable(&daemon, &mut waker, idle_wait(last_liveness_check));
            if started.elapsed() < HANGUP_BACKOFF {
                instant_idle_waits = instant_idle_waits.saturating_add(1);
            } else {
                instant_idle_waits = 0;
            }
        } else {
            instant_idle_waits = 0;
        }
    }
}

/// Whether this pane must not be read yet, because a viewer has fallen behind.
///
/// Backpressure, rather than buffering without limit or dropping the viewer.
/// Leaving the bytes in the terminal is what makes the *program* wait — exactly
/// what happens when a client reads its own pty — so a shared pane costs a program
/// what an exclusive one costs instead of letting it outrun every viewer.
///
/// A viewer that has stopped reading altogether is the other case, and is dropped
/// here rather than waited for: its backlog will never shrink, and holding the pane
/// for it would freeze the pane for every other viewer.
fn relay_backpressure(pane: &mut Pane, evicted: &mut bool) -> bool {
    let now = Instant::now();
    let mut hold = false;
    let mut stalled = Vec::new();
    if let Attachment::Shared(clients) = &mut pane.attachment {
        for client in clients.iter_mut() {
            let written = client.relay.written.load(Ordering::Relaxed);
            if written != client.written_seen {
                client.written_seen = written;
                client.wrote_at = now;
            }
            let backlog = client.relay.queued.load(Ordering::Relaxed);
            // Checked at *any* backlog, not only past the threshold. A blocked
            // relay drains what is queued into a socket that has room, so the
            // backlog dips below the threshold between passes — and gating the
            // check on the threshold meant a viewer that had stopped reading
            // entirely was never noticed here at all, leaving the pane to stutter
            // until the write timeout eventually killed the relay seconds later.
            if backlog > 0 && now.duration_since(client.wrote_at) >= RELAY_STALL_TIMEOUT {
                stalled.push(client.process_id);
                continue;
            }
            if backlog >= RELAY_BACKPRESSURE_BYTES {
                hold = true;
            }
        }
        if !stalled.is_empty() {
            log::debug!(
                "dropping {} shared viewer(s) whose backlog stopped shrinking",
                stalled.len()
            );
            clients.retain(|client| !stalled.contains(&client.process_id));
        }
    }
    if !stalled.is_empty() {
        collapse_empty_shared(&mut pane.attachment, pane.handover_waiters);
        *evicted = true;
    }
    hold
}

/// Whether the drain thread is the one reading this pane.
///
/// An exclusive client reads its own pane, and a pane under revoke is still
/// being read by its holder until the snapshot arrives. Shared by the drain's
/// read pass and by the set of terminals it waits on, so the two cannot drift
/// apart — waiting on a pane nobody then reads would stall the drain, and
/// reading one it never waits on would put that pane back on a timer.
fn drain_reads(attachment: &Attachment) -> bool {
    match attachment {
        // Mid-handover in either direction, nothing reads the pane: the holder
        // still has the descriptor on the way in, and the frames already queued
        // have to land before the descriptor does on the way out.
        Attachment::Exclusive(_) | Attachment::Revoking { .. } | Attachment::Granting { .. } => {
            false
        }
        Attachment::None | Attachment::Shared(_) => true,
    }
}

/// How long a wait may last with nothing happening.
///
/// Bounded by the liveness sweep and nothing else: the wait ends by itself when
/// there is something to do, so the only reason to return empty-handed is to run
/// that sweep on schedule.
fn idle_wait(last_liveness_check: Instant) -> Duration {
    CLIENT_LIVENESS_INTERVAL
        .saturating_sub(last_liveness_check.elapsed())
        .max(HANGUP_BACKOFF)
}

/// How long to pause after a run of waits that returned instantly with nothing
/// to read, which is what a hung-up terminal looks like before it is reaped.
const HANGUP_BACKOFF: Duration = Duration::from_millis(1);

/// How many such waits to allow before pausing. More than one, because a single
/// instant return is the ordinary case of output arriving.
const INSTANT_IDLE_WAITS_BEFORE_BACKING_OFF: u32 = 2;

/// Blocks until a pane the drain is responsible for has output, a client
/// attaches or detaches, or the wait runs out.
///
/// This is what makes shared mode responsive. Sleeping a fixed twenty
/// milliseconds instead put half of that on every shared keystroke's round
/// trip — measured at a ten millisecond median, against ten *microseconds* for
/// a client reading the pty itself — and the waker could not shorten it,
/// because it was drained *before* the sleep rather than waited on, so
/// `wake_drain` had no effect at all.
#[cfg(unix)]
fn wait_for_drainable(daemon: &Arc<Daemon>, waker: &mut Stream, timeout: Duration) {
    use std::os::fd::AsRawFd as _;
    let mut fds = vec![libc::pollfd {
        fd: waker.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    {
        let sessions = daemon.sessions.lock().unwrap();
        for session in sessions.iter() {
            for pane in session.panes.iter() {
                // An exited pane is drained to the end by the pass above and
                // then reports hangup for ever; waiting on it would never block.
                if pane.exited || !drain_reads(&pane.attachment) {
                    continue;
                }
                fds.push(libc::pollfd {
                    fd: pane.pty.file().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
        }
    }
    let millis = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    // SAFETY: `fds` is a valid, initialised slice of exactly the length given,
    // and the descriptors outlive the call — the sessions lock is released only
    // after they are collected, and a pane closing while this waits shows up as
    // `POLLNVAL`, which ends the wait rather than corrupting it.
    unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, millis) };
    drain_waker(waker);
}

#[cfg(windows)]
fn wait_for_drainable(_daemon: &Arc<Daemon>, waker: &mut Stream, timeout: Duration) {
    // A pseudoconsole's pipes cannot be waited on alongside the wake channel in
    // one call the way a pty's descriptors can, so this keeps the fixed tick and
    // the latency that comes with it.
    drain_waker(waker);
    thread::sleep(timeout.min(WINDOWS_DRAIN_TICK));
}

#[cfg(windows)]
const WINDOWS_DRAIN_TICK: Duration = Duration::from_millis(20);

/// Empties the wake channel.
///
/// Every byte has to go: a wake channel left readable makes the next wait return
/// immediately, and the one after that, which is a busy loop rather than a wait.
fn drain_waker(waker: &mut Stream) {
    let mut buffer = [0; 64];
    while waker
        .read(&mut buffer)
        .is_ok_and(|read| read == buffer.len())
    {}
}

/// Writes as much queued shared input as the terminal will take right now.
///
/// A partial write is normal on a non-blocking master and is not an error: what
/// is left stays queued and the drain thread tries again, so a paste larger than
/// the terminal's free buffer arrives in full rather than in part.
fn flush_pending_input(pane: &mut Pane) {
    use alacritty_terminal::tty::EventedReadWrite as _;
    if pane.pending_input.is_empty() {
        return;
    }
    let mut written = 0;
    while written < pane.pending_input.len() {
        match pane.pty.writer().write(&pane.pending_input[written..]) {
            Ok(0) => break,
            Ok(count) => written += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => {
                // The terminal is gone, so nothing queued for it can ever be
                // delivered. Dropping it is right; keeping it would make the
                // drain thread retry a write that cannot succeed.
                log::debug!("writing shared input to pane {} failed: {error:#}", pane.id);
                pane.pending_input.clear();
                return;
            }
        }
    }
    pane.pending_input.drain(..written);
}

/// Sends a pane's output to every shared client, dropping clients whose
/// socket can no longer be written. A pane whose shared set empties stops
/// being shared.
fn relay_output(pane: &mut Pane, bytes: &[u8]) {
    if !matches!(pane.attachment, Attachment::Shared(_)) {
        return;
    }
    // One frame, shared between the viewers by reference: the fan-out costs a
    // refcount each rather than a copy of the pane's output per viewer.
    let frame = match crate::transport::encode_message(&Event::Output {
        pane_id: pane.id,
        length: bytes.len(),
    }) {
        Ok(mut frame) => {
            frame.extend_from_slice(bytes);
            Arc::<[u8]>::from(frame)
        }
        Err(error) => {
            log::warn!("could not frame a pane's output: {error:#}");
            return;
        }
    };
    queue_for_shared_clients(&mut pane.attachment, pane.handover_waiters, &frame);
}

/// Returns a pane whose last shared client has gone to being unheld.
///
/// Only [`remove_shared_client`] used to do this, so a client dropped for being
/// unwritable — a wedged viewer past the relay's write timeout — left the pane
/// "shared with nobody": still drained, but never exclusively attachable again
/// and never pruned, because both require [`Attachment::None`].
fn collapse_empty_shared(attachment: &mut Attachment, handover_waiters: usize) {
    if handover_waiters == 0
        && matches!(attachment, Attachment::Shared(clients) if clients.is_empty())
    {
        *attachment = Attachment::None;
    }
}

/// How often the daemon checks that the clients holding panes still exist.
const CLIENT_LIVENESS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Takes back panes whose client is gone.
///
/// Detaching is normally explicit, but a client that crashes — or is killed —
/// never sends it. Without this the pane stays marked as held: the daemon
/// never resumes reading it, so the session appears alive while its program
/// blocks on a terminal nobody is draining, and it can never be handed to
/// anyone else in a usable state.
fn reclaim_panes_from_departed_clients(daemon: &Arc<Daemon>) {
    let holders = {
        let sessions = daemon.sessions.lock().unwrap();
        sessions
            .iter()
            .flat_map(|session| session.panes.iter())
            .filter_map(|pane| match pane.attachment {
                Attachment::Exclusive(holder) => Some(holder),
                // A revoke whose holder died can never be answered; reclaim lets
                // the waiting attach take the pane.
                Attachment::Revoking { holder } => Some(holder),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    if holders.is_empty() {
        return;
    }

    let departed = holders
        .into_iter()
        .filter(|pid| !process_is_running(*pid))
        .collect::<std::collections::BTreeSet<_>>();
    if departed.is_empty() {
        return;
    }

    let mut reclaimed = false;
    let mut sessions = daemon.sessions.lock().unwrap();
    // Only the panes are reclaimed. A session's *scope* is deliberately left
    // alone: a private backgrounding is not a slow way of sharing it, so a
    // window exiting must not turn a session another Zetta could not see into
    // one it can attach. `Request::SetSessionScope` is how that happens, and it
    // is a request somebody makes. A session already offered by its window is
    // already shared, so reclaiming its pane does not need to widen anything.
    for session in sessions.iter_mut() {
        for pane in session.panes.iter_mut() {
            let departed_holder = match pane.attachment {
                Attachment::Exclusive(holder) => departed.contains(&holder),
                Attachment::Revoking { holder } => departed.contains(&holder),
                _ => false,
            };
            if departed_holder {
                log::info!(
                    "reclaiming pane {} from client {}, which exited without detaching",
                    pane.id,
                    match pane.attachment {
                        Attachment::Exclusive(holder) | Attachment::Revoking { holder } => holder,
                        _ => unreachable!(),
                    }
                );
                pane.attachment = Attachment::None;
                #[cfg(windows)]
                pane.pty.resume_reader();
                reclaimed = true;
            }
        }
    }

    let (_, _consoles) = end_abandoned_sessions(&mut sessions);
    drop(sessions);
    #[cfg(windows)]
    for console_id in _consoles {
        close_host_console(daemon, console_id);
    }
    if reclaimed {
        // An attach may be waiting on this pane's revoke handover.
        daemon.sessions_condvar.notify_all();
    }
    // A pane a departed client was showing may have ended while attached: it
    // was kept until the client let go, and now that it has, it has nothing
    // left to attach to.
    prune_exited_panes(daemon);
    publish(daemon);
}

/// Ends sessions that no client holds and that nobody asked to keep.
///
/// Being held is what the user asked for by detaching; it is not something a
/// client's death or a closed pane confers. A session that was never detached
/// belonged to a window that is now gone, so it ends with that window — which is
/// what "visible tabs do not become background sessions implicitly" means.
/// Promoting them instead turned every Zetta that crashed or was killed into a
/// pile of sessions holding stray shells the user never asked for and could not
/// meaningfully attach.
///
/// Dropping a `Session` drops its panes' PTYs, which hangs up their children, so
/// this is also what stops an orphaned shell from lingering. Returns whether
/// anything ended.
fn end_abandoned_sessions(sessions: &mut Vec<Session>) -> (bool, Vec<u64>) {
    let before = sessions.len();
    #[cfg(windows)]
    let mut consoles = Vec::new();
    #[cfg(not(windows))]
    let consoles = Vec::new();
    sessions.retain(|session| {
        let abandoned = !session.keep && session.panes.iter().all(|pane| pane.attachment.is_none());
        if abandoned {
            log::info!(
                "ending session {}, which no window holds and nobody asked to keep",
                session.id
            );
            #[cfg(windows)]
            consoles.extend(session.panes.iter().map(|pane| pane.console_id));
        }
        !abandoned
    });
    (sessions.len() != before, consoles)
}

fn process_is_running(process_id: u32) -> bool {
    let process_id = sysinfo::Pid::from_u32(process_id);
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[process_id]), true);
    system.process(process_id).is_some()
}

fn wake_drain(daemon: &Arc<Daemon>) {
    if let Some(wake) = daemon.drain_wake.lock().unwrap().as_mut() {
        use std::io::Write as _;
        let _ = wake.write_all(b".");
    }
}

#[cfg(windows)]
fn close_host_console(daemon: &Daemon, console_id: u64) {
    if let Err(error) = daemon.pty_host.close(console_id) {
        log::debug!("could not close pseudoconsole {console_id}: {error:#}");
    }
}

/// Why an upgrade did not happen, and whether the client has been told.
///
/// The answer is sent before the exec, because an exec never returns to send
/// one. Everything that fails after that point therefore must not be reported on
/// the connection again.
#[cfg(any(unix, windows))]
enum UpgradeRefused {
    Before(anyhow::Error),
    #[cfg_attr(windows, allow(dead_code))]
    AfterAnswering(anyhow::Error),
}

/// Replaces this daemon with a fresh image of itself.
///
/// Returns only on failure, in which case this daemon carries on holding
/// everything it had: an upgrade that cannot be completed must not be a way to
/// lose the sessions it was meant to preserve.
#[cfg(unix)]
fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    prepare_and_exec(daemon, connection)
}

#[cfg(unix)]
fn prepare_and_exec(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let prepared = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;
    let (executable, file) = prepared;

    // Answered before the exec, because the exec never returns to answer. The
    // pre-flight has already established that the replacement can take over, so
    // this reports what was accepted rather than what completed.
    connection
        .send(&Response::Ok)
        .map_err(UpgradeRefused::Before)?;

    // Tell the clients this is deliberate, so the disconnect the exec is about
    // to cause reads as a replacement rather than a failure. Their terminals are
    // unaffected either way — in exclusive mode they hold the descriptors
    // themselves — but a client that knows can reconnect at once instead of
    // waiting out a backoff, and it never logs a lost multiplexer as a fault.
    // Best effort by nature: whether or not this lands, no client may treat a
    // lost subscription as its panes' processes ending.
    broadcast(daemon, &Event::Replacing);

    // Past this point the process becomes the replacement, so anything that
    // was going to fail had to have failed already.
    match crate::upgrade::exec_replacement(&executable, file.as_raw_fd(), daemon.listener_fd) {
        Ok(_) => Ok(()),
        Err(error) => Err(UpgradeRefused::AfterAnswering(error)),
    }
}

/// Everything that can still be refused: resolving the image, checking it can
/// take over, and writing the handover.
#[cfg(unix)]
fn prepare_upgrade(daemon: &Arc<Daemon>) -> Result<(PathBuf, std::fs::File)> {
    let executable = daemon
        .executable
        .clone()
        .context("this multiplexer does not know its own path, so it cannot be replaced")?;
    anyhow::ensure!(
        executable.is_file(),
        "{} no longer exists, so this multiplexer cannot replace itself with it. Its sessions \
         are untouched; rebuild or reinstall in place and try again.",
        executable.display()
    );
    anyhow::ensure!(
        crate::upgrade::replacement_accepts_handover(&executable)?,
        "{} cannot take over this multiplexer's sessions, so it was not started",
        executable.display()
    );

    #[cfg(feature = "session-persistence")]
    if let Some(persistence) = daemon.persistence.lock().unwrap().as_mut() {
        persistence
            .flush_segments()
            .context("flushing encrypted scrollback before replacing the daemon")?;
    }

    let sessions = daemon.sessions.lock().unwrap();
    let now = std::time::Instant::now();
    let handover = crate::upgrade::Handover {
        version: crate::upgrade::HANDOVER_VERSION,
        generation: daemon.catalog.lock().unwrap().runner_id(),
        next_session_id: daemon.next_session_id.load(Ordering::SeqCst),
        next_pane_id: daemon.next_pane_id.load(Ordering::SeqCst),
        sessions: sessions
            .iter()
            .map(|session| {
                crate::upgrade::SessionHandover {
                    id: session.id,
                    summary: session.summary.clone(),
                    state: session.state.clone(),
                    keep: session.keep,
                    offered: session.offered,
                    owner: session.owner,
                    verifier: session
                        .authentication
                        .as_ref()
                        .map(|authentication| authentication.verifier().to_owned()),
                    failed_authentications: session.failed_authentications,
                    // A monotonic deadline means nothing in another image, so
                    // what is carried is how much of the window is left.
                    refuse_for: session
                        .refuse_until
                        .and_then(|until| until.checked_duration_since(now)),
                    panes: session
                        .panes
                        .iter()
                        .map(|pane| crate::upgrade::PaneHandover {
                            id: pane.id,
                            descriptor: pane.pty.file().as_raw_fd(),
                            child_pid: pane.pty.child_pid(),
                            attachment: attachment_handover(&pane.attachment),
                            columns: pane.size.columns,
                            lines: pane.size.lines,
                            exited: pane.exited,
                            exit_status: pane.exit_status,
                            // Copied, not taken. The exec is irreversible but
                            // everything before it is not, and emptying the live
                            // ring here meant a refused upgrade had already
                            // destroyed the output it was protecting.
                            retained: pane.retained.snapshot(),
                        })
                        .collect(),
                }
            })
            .collect(),
    };

    // Only the terminals. The `SIGCHLD` pipes are deliberately left
    // close-on-exec: the replacement registers its own, and a carried one is
    // worse than useless — see `PaneHandover::descriptor`.
    for session in sessions.iter() {
        for pane in &session.panes {
            crate::upgrade::keep_across_exec(pane.pty.file())?;
        }
    }
    // Keep the listener together with the PTY masters. Rebinding it after exec
    // introduced a refusal window in which clients could mistake an orderly
    // upgrade for a dead daemon.
    let listener = unsafe { BorrowedFd::borrow_raw(daemon.listener_fd) };
    crate::upgrade::keep_across_exec(&listener)?;
    let file = crate::upgrade::write_handover(&handover)?;
    drop(sessions);
    Ok((executable, file))
}

#[cfg(windows)]
fn upgrade_daemon(
    daemon: &Arc<Daemon>,
    connection: &mut Connection,
) -> std::result::Result<(), UpgradeRefused> {
    let (executable, handover, ready) = prepare_upgrade(daemon).map_err(UpgradeRefused::Before)?;
    let mut replacement = match crate::upgrade::spawn_replacement(&executable, &handover, &ready) {
        Ok(child) => child,
        Err(error) => {
            crate::upgrade::remove_handover(&handover, &ready);
            return Err(UpgradeRefused::Before(error));
        }
    };
    if let Err(error) = crate::upgrade::wait_for_ready(&mut replacement, &ready) {
        let _ = replacement.kill();
        crate::upgrade::remove_handover(&handover, &ready);
        return Err(UpgradeRefused::Before(error));
    }
    if let Err(error) = connection.send(&Response::Ok) {
        let _ = replacement.kill();
        crate::upgrade::remove_handover(&handover, &ready);
        return Err(UpgradeRefused::Before(error));
    }
    broadcast(daemon, &Event::Replacing);
    daemon.running.store(false, Ordering::SeqCst);
    let _ = Stream::connect(socket_path(&session_catalog_dir()));
    Ok(())
}

#[cfg(windows)]
fn prepare_upgrade(daemon: &Arc<Daemon>) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let executable = daemon
        .executable
        .clone()
        .context("this multiplexer does not know its own path, so it cannot be replaced")?;
    anyhow::ensure!(
        executable.is_file(),
        "{} no longer exists, so this multiplexer cannot replace itself with it. Its sessions \
         are untouched; rebuild or reinstall in place and try again.",
        executable.display()
    );
    anyhow::ensure!(
        crate::upgrade::replacement_accepts_handover(&executable)?,
        "{} cannot take over this multiplexer's sessions, so it was not started",
        executable.display()
    );

    let sessions = daemon.sessions.lock().unwrap();
    let now = Instant::now();
    let handover = crate::upgrade::Handover {
        version: crate::upgrade::HANDOVER_VERSION,
        generation: daemon.catalog.lock().unwrap().runner_id(),
        next_session_id: daemon.next_session_id.load(Ordering::SeqCst),
        next_pane_id: daemon.next_pane_id.load(Ordering::SeqCst),
        retention: *daemon.retention.lock().unwrap(),
        sessions: sessions
            .iter()
            .map(|session| crate::upgrade::SessionHandover {
                id: session.id,
                summary: session.summary.clone(),
                state: session.state.clone(),
                keep: session.keep,
                offered: session.offered,
                owner: session.owner,
                verifier: session
                    .authentication
                    .as_ref()
                    .map(|authentication| authentication.verifier().to_owned()),
                failed_authentications: session.failed_authentications,
                refuse_for: session
                    .refuse_until
                    .and_then(|until| until.checked_duration_since(now)),
                panes: session
                    .panes
                    .iter()
                    .map(|pane| crate::upgrade::PaneHandover {
                        id: pane.id,
                        console_id: pane.console_id,
                        child_pid: pane.pty.child_pid(),
                        attachment: attachment_handover(&pane.attachment),
                        columns: pane.size.columns,
                        lines: pane.size.lines,
                        exited: pane.exited,
                        exit_status: pane.exit_status,
                        retained: pane.retained.snapshot(),
                    })
                    .collect(),
            })
            .collect(),
    };
    drop(sessions);
    validate_handover_with_host(&daemon.pty_host, &handover)?;
    let (handover, ready) = crate::upgrade::write_handover(&session_catalog_dir(), &handover)?;
    Ok((executable, handover, ready))
}

/// A pane's attachment in the form that crosses an exec.
#[cfg(any(unix, windows))]
fn attachment_handover(attachment: &Attachment) -> crate::upgrade::AttachmentHandover {
    use crate::upgrade::{AttachmentHandover, SharedClientHandover};
    match attachment {
        Attachment::None => AttachmentHandover::None,
        Attachment::Exclusive(holder) => AttachmentHandover::Exclusive { holder: *holder },
        Attachment::Revoking { holder } => AttachmentHandover::Revoking { holder: *holder },
        // A grant in flight cannot survive: the descriptor was going out on a
        // connection the exec closes. It comes back as shared with nobody, which
        // is what every shared pane comes back as, and the client re-attaches —
        // whereupon it is alone again and offered the grant afresh.
        Attachment::Granting { .. } => AttachmentHandover::Shared {
            clients: Vec::new(),
        },
        Attachment::Shared(clients) => AttachmentHandover::Shared {
            clients: clients
                .iter()
                .map(|client| SharedClientHandover {
                    process_id: client.process_id,
                    columns: client.columns,
                    lines: client.lines,
                    input_sent: client.input_sent,
                })
                .collect(),
        },
    }
}

/// Takes over the sessions the previous image left behind.
///
/// The descriptors are the same ones it held: the `execv` kept them open, so a
/// pane is rebuilt around its existing terminal rather than being restarted.
/// The identifiers a resumed multiplexer must continue from.
///
/// Kept separate so the rule is testable without a live handover: never at or
/// below anything already held, whatever the handover claims.
#[cfg(any(unix, windows))]
fn next_ids_after(
    session_ids: &[u64],
    pane_ids: &[u64],
    handover_session: u64,
    handover_pane: u64,
) -> (u64, u64) {
    let next_session = session_ids
        .iter()
        .map(|id| id + 1)
        .chain(std::iter::once(handover_session))
        .max()
        .unwrap_or(1);
    let next_pane = pane_ids
        .iter()
        .map(|id| id + 1)
        .chain(std::iter::once(handover_pane))
        .max()
        .unwrap_or(1);
    (next_session, next_pane)
}

/// Restores a pane's attachment from what crossed the exec.
///
/// Shared clients come back with their sizes and attribution but no
/// connections — those cannot be carried — so the pane stays shared and each
/// viewer rejoins the relay through a fresh attach. It must stay shared: coming
/// back exclusive-capable would hand the descriptor to whichever viewer
/// reconnected first, and the rest would be reading a terminal nobody was
/// relaying.
#[cfg(any(unix, windows))]
fn adopt_attachment(attachment: crate::upgrade::AttachmentHandover) -> Attachment {
    use crate::upgrade::AttachmentHandover;
    match attachment {
        AttachmentHandover::None => Attachment::None,
        AttachmentHandover::Exclusive { holder } => Attachment::Exclusive(holder),
        // Still mid-handover: the holder has the descriptor and its snapshot is
        // still expected, so nothing here may read the pane. Collapsing this to
        // "no holder" let the drain thread start reading a terminal the holder
        // was still reading, and made the holder's snapshot arrive at a pane
        // that was no longer being handed over — so the handover failed and left
        // that pane inert.
        AttachmentHandover::Revoking { holder } => Attachment::Revoking { holder },
        AttachmentHandover::Shared { clients: _ } => Attachment::Shared(Vec::new()),
    }
}

#[cfg(unix)]
fn adopt_handover(daemon: &Arc<Daemon>, handover: crate::upgrade::Handover) -> Result<usize> {
    use std::os::fd::FromRawFd as _;

    let retention = *daemon.retention.lock().unwrap();
    let mut sessions = daemon.sessions.lock().unwrap();
    let now = std::time::Instant::now();
    for session in handover.sessions {
        // Adopting one twice would leave the multiplexer holding two entries
        // that no client could tell apart.
        if sessions.iter().any(|existing| existing.id == session.id) {
            log::warn!("ignoring session {}, which is already held", session.id);
            continue;
        }
        // The session's own id is authoritative. A previous image could have
        // been handed a summary whose id disagreed with it (see `detach`), and
        // letting that survive the upgrade would keep the session listed under
        // one id while being looked up under another. Normalizing here is what
        // lets an upgrade repair a session that had been corrupted.
        let mut summary = session.summary;
        summary.id = session.id;
        let mut panes = Vec::new();
        for pane in session.panes {
            anyhow::ensure!(
                crate::upgrade::descriptor_is_open(pane.descriptor),
                "pane {}'s terminal did not survive the upgrade",
                pane.id
            );
            // SAFETY: checked open just above; it was kept across the exec by
            // the previous image, and is claimed here exactly once.
            let descriptor = unsafe { std::os::fd::OwnedFd::from_raw_fd(pane.descriptor) };
            // Reclaimed regardless of who was holding it. An `execv` does not
            // change parentage, so every one of these shells is still this
            // process's own child and is reaped here with `waitpid` — which is
            // the whole reason the upgrade is a re-exec rather than a fresh
            // process. Treating a held pane as though its child belonged to
            // somebody else meant the daemon never reaped it (a zombie, and no
            // exit ever reported) and read a dead pipe as though the process had
            // ended (a false exit for a shell that was running fine).
            let pty = tty::reclaim(descriptor, pane.child_pid)?;
            // Rebuilt at the size the pane is running at, then given the screen
            // the previous image serialized for it.
            let mut retained = retention.new_retained(pane.columns, pane.lines);
            retained.seed(pane.retained);
            panes.push(Pane {
                id: pane.id,
                pty,
                attachment: adopt_attachment(pane.attachment),
                // The size the pane is actually running at, carried across so
                // arbitration continues from what its viewers are showing.
                // Restarting from a default silently resized every adopted pane.
                size: TerminalSize {
                    columns: pane.columns,
                    lines: pane.lines,
                    cell_width: 0,
                    cell_height: 0,
                },
                retained,
                // An upgrade cannot land mid-handover: the client that would be
                // waiting for one is the client that asked for the upgrade.
                handed_over: None,
                handover_waiters: 0,
                exited: pane.exited,
                exit_status: pane.exit_status,
                pending_input: Vec::new(),
            });
        }
        sessions.push(Session {
            id: session.id,
            summary,
            state: session.state,
            authentication: session
                .verifier
                .map(SessionAuthentication::from_verifier)
                .transpose()?,
            failed_authentications: session.failed_authentications,
            // Restored as a deadline again, from what was left of the window.
            refuse_until: session.refuse_for.and_then(|left| now.checked_add(left)),
            panes,
            keep: session.keep,
            offered: session.offered,
            owner: session.owner,
        });
    }
    // Derived from what was actually adopted, not merely taken from the
    // handover. A counter that lands at or below an existing identifier makes
    // the next spawn reuse it, and two sessions sharing an id are
    // indistinguishable to every client: they list identically, and attaching
    // one is a coin toss. Trusting the handover alone let exactly that happen.
    let session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<Vec<_>>();
    let pane_ids = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let (next_session_id, next_pane_id) = next_ids_after(
        &session_ids,
        &pane_ids,
        handover.next_session_id,
        handover.next_pane_id,
    );
    daemon
        .next_session_id
        .store(next_session_id, Ordering::SeqCst);
    daemon.next_pane_id.store(next_pane_id, Ordering::SeqCst);
    Ok(sessions.len())
}

#[cfg(windows)]
fn adopt_handover(daemon: &Arc<Daemon>, handover: crate::upgrade::Handover) -> Result<usize> {
    let mut sessions = daemon.sessions.lock().unwrap();
    let now = Instant::now();
    for session in handover.sessions {
        if sessions.iter().any(|existing| existing.id == session.id) {
            log::warn!("ignoring session {}, which is already held", session.id);
            continue;
        }
        let mut summary = session.summary;
        summary.id = session.id;
        let mut panes = Vec::new();
        for pane in session.panes {
            let (child_pid, handles) = daemon
                .pty_host
                .handles(pane.console_id, std::process::id())?;
            anyhow::ensure!(
                pane.child_pid == 0 || child_pid == 0 || pane.child_pid == child_pid,
                "pseudoconsole {} changed children during the upgrade",
                pane.console_id
            );
            let mut handles = crate::transport::claim_duplicated(&handles);
            anyhow::ensure!(
                handles.len() == 2,
                "pseudoconsole {} did not return two pipe handles",
                pane.console_id
            );
            let conin = handles.remove(1);
            let conout = handles.remove(0);
            let (pty, child_events) = tty::attach(conout, conin, child_pid)
                .context("attaching an adopted pseudoconsole to the daemon")?;
            let retention = *daemon.retention.lock().unwrap();
            let mut retained = retention.new_retained(pane.columns, pane.lines);
            retained.seed(pane.retained);
            panes.push(Pane {
                id: pane.id,
                pty,
                console_id: pane.console_id,
                child_events,
                attachment: adopt_attachment(pane.attachment),
                size: TerminalSize {
                    columns: pane.columns,
                    lines: pane.lines,
                    cell_width: 0,
                    cell_height: 0,
                },
                retained,
                handed_over: None,
                handover_waiters: 0,
                exited: pane.exited,
                exit_status: pane.exit_status,
                pending_input: Vec::new(),
            });
        }
        sessions.push(Session {
            id: session.id,
            summary,
            state: session.state,
            authentication: session
                .verifier
                .map(SessionAuthentication::from_verifier)
                .transpose()?,
            failed_authentications: session.failed_authentications,
            refuse_until: session.refuse_for.and_then(|left| now.checked_add(left)),
            panes,
            keep: session.keep,
            offered: session.offered,
            owner: session.owner,
        });
    }
    let session_ids = sessions
        .iter()
        .map(|session| session.id)
        .collect::<Vec<_>>();
    let pane_ids = sessions
        .iter()
        .flat_map(|session| session.panes.iter())
        .map(|pane| pane.id)
        .collect::<Vec<_>>();
    let (next_session_id, next_pane_id) = next_ids_after(
        &session_ids,
        &pane_ids,
        handover.next_session_id,
        handover.next_pane_id,
    );
    daemon
        .next_session_id
        .store(next_session_id, Ordering::SeqCst);
    daemon.next_pane_id.store(next_pane_id, Ordering::SeqCst);
    Ok(sessions.len())
}

/// Reaps children and tells whoever is holding their terminals.
///
/// Windows has no `SIGCHLD`: each PTY carries its own exit watcher, which
/// signals when its process ends, so this polls those watchers rather than
/// waiting on a signal.
#[cfg(windows)]
fn start_reaper(daemon: Arc<Daemon>) -> Result<()> {
    spawn_worker("zmux reaper", move || {
        let daemon = daemon.clone();
        Box::new(move || windows_reaper_loop(daemon))
    });
    Ok(())
}

#[cfg(windows)]
fn windows_reaper_loop(daemon: Arc<Daemon>) {
    loop {
        match daemon.pty_host.reap() {
            Ok(exits) if !exits.is_empty() => {
                let mut sessions = daemon.sessions.lock().unwrap();
                for exit in exits {
                    let Some(pane) = sessions
                        .iter_mut()
                        .flat_map(|session| session.panes.iter_mut())
                        .find(|pane| pane.console_id == exit.console_id)
                    else {
                        continue;
                    };
                    let result = match exit.exit_code {
                        Some(code) => pane.child_events.report_exit(code),
                        None => pane.child_events.report_status_unavailable(),
                    };
                    if let Err(error) = result {
                        log::debug!(
                            "could not report exit for pseudoconsole {}: {error}",
                            exit.console_id
                        );
                    }
                }
            }
            Ok(_) => {}
            Err(error) => log::warn!("could not collect pseudoconsole exits: {error:#}"),
        }
        let mut exits = Vec::new();
        {
            let mut sessions = daemon.sessions.lock().unwrap();
            for session in sessions.iter_mut() {
                for pane in session.panes.iter_mut() {
                    if pane.exited {
                        continue;
                    }
                    if let Some(exit) = observe_pane_exit(session.id, pane) {
                        exits.push(exit);
                    }
                }
            }
        }
        for (session_id, pane_id, raw_status, input_sent) in exits {
            broadcast(
                &daemon,
                &Event::PaneExited {
                    session_id,
                    pane_id,
                    raw_status,
                    input_sent,
                },
            );
        }
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// How a terminal reaches the client, which differs by platform.
///
/// Unix attaches the descriptor to the message itself. Windows has no such
/// mechanism, so the handles are duplicated into the client's process first
/// and travel in the message as plain values.
struct Handover {
    #[cfg(unix)]
    attachments: Vec<std::os::fd::BorrowedFd<'static>>,
    #[cfg(windows)]
    attachments: Vec<()>,
    values: Vec<i64>,
}

#[cfg(unix)]
fn handover_handles(_daemon: &Daemon, pane: &Pane, _client_process_id: u32) -> Result<Handover> {
    // SAFETY: the borrow lives only until the message has been sent, well
    // inside the lifetime of the PTY the daemon is holding.
    let descriptor = unsafe {
        std::os::fd::BorrowedFd::borrow_raw(std::os::fd::AsRawFd::as_raw_fd(pane.pty.file()))
    };
    Ok(Handover {
        attachments: vec![descriptor],
        values: Vec::new(),
    })
}

#[cfg(windows)]
fn handover_handles(daemon: &Daemon, pane: &Pane, client_process_id: u32) -> Result<Handover> {
    anyhow::ensure!(
        client_process_id != 0,
        "the client did not say which process to hand its terminal to"
    );
    let (_, values) = daemon
        .pty_host
        .handles(pane.console_id, client_process_id)?;
    Ok(Handover {
        attachments: Vec::new(),
        values,
    })
}

/// Stops the Windows daemon reader at the same boundary at which the console
/// handles are handed to an exclusive client. The reader's internal pipe may
/// already contain bytes after its last drain pass, so retain and relay those
/// bytes before the descriptor changes owner.
#[cfg(windows)]
fn pause_pane_reader(pane: &mut Pane) -> Result<()> {
    pane.pty
        .pause_reader()
        .context("pausing the Windows pseudoconsole reader")?;
    let mut buffer = [0; 16 * 1024];
    loop {
        let read = pane.pty.read_buffered(&mut buffer);
        if read == 0 {
            break;
        }
        pane.retained.push(&buffer[..read]);
        record_handover_output(pane, &buffer[..read]);
        relay_output(pane, &buffer[..read]);
    }
    Ok(())
}

#[cfg(unix)]
fn pause_pane_reader(_pane: &mut Pane) -> Result<()> {
    Ok(())
}

/// Reads whatever a detached pane has produced.
///
/// The two platforms expose different things to read: a Unix PTY is one
/// descriptor, while a pseudoconsole is an unblocked pipe with its own buffer.
#[cfg(unix)]
fn read_pane(pty: &mut tty::Pty, buffer: &mut [u8]) -> std::io::Result<usize> {
    pty.file().read(buffer)
}

#[cfg(windows)]
fn read_pane(pty: &mut tty::Pty, buffer: &mut [u8]) -> std::io::Result<usize> {
    use alacritty_terminal::tty::EventedReadWrite as _;
    Ok(pty.reader().try_read(buffer))
}

/// This process's own executable, if it can be established.
///
/// Called once, at startup, so the answer predates any rebuild. A path Linux
/// has marked `(deleted)` is rejected rather than repaired: the file it named
/// is gone, and guessing at which file replaced it is how a multiplexer ends
/// up executing something the user did not install.
fn resolve_own_executable() -> Option<PathBuf> {
    let current = std::env::current_exe().ok()?;
    #[cfg(unix)]
    let path = current;
    #[cfg(windows)]
    let path = current
        .parent()
        .map(|directory| directory.join("zmux.exe"))
        .filter(|candidate| candidate.is_file())
        .unwrap_or(current);
    if path.as_os_str().as_encoded_bytes().ends_with(b" (deleted)") {
        log::warn!(
            "this multiplexer's executable was already replaced at startup, so it cannot be \
             upgraded in place"
        );
        return None;
    }
    path.is_file().then_some(path)
}

#[cfg(all(test, unix))]
#[path = "tests/server.rs"]
mod tests;
