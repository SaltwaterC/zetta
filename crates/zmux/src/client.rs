//! Talking to the multiplexer.
//!
//! Each request opens its own connection, which keeps request and response
//! trivially paired. Asynchronous events — a pane's process exiting — arrive on
//! one long-lived subscription instead, because they are not answers to
//! anything.

use std::{
    collections::HashMap,
    io,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::tty::ConsolePalette;
use anyhow::{Context as _, Result};

#[cfg(unix)]
use std::os::fd::OwnedFd;

/// What a received message may carry alongside it: terminal descriptors on
/// Unix, and nothing on Windows, where handles travel inside the message
/// already duplicated into this process.
#[cfg(unix)]
type Descriptors = Vec<OwnedFd>;
#[cfg(windows)]
type Descriptors = Vec<()>;

use crate::{
    messages::{
        DetachRequest, Envelope, Event, PROTOCOL_VERSION, PaneSnapshot, PaneStateReport, Request,
        Response, SpawnRequest,
    },
    paths::session_catalog_dir,
    protocol::{BackgroundSessionSummary, RestorableSessionRecord},
    retention::Retention,
    server::endpoint_path,
    transport::{Connection, Endpoint, Stream},
};

#[cfg(feature = "session-persistence")]
use crate::messages::{ResumeRequest, ResumeSnapshot};

#[cfg(feature = "session-persistence")]
use crate::persistence::{DaemonOptionsFile, PersistenceOptions};

// Unconditional: what protects a session is a verifier and, when it was sealed
// rather than typed, an envelope this crate only ever passes along. Neither
// needs age, so detach, share and scope carry them in every build.
use crate::auth::SessionAuthentication;
use crate::auth::SessionSecret;
use crate::messages::ClientId;
use crate::remote::{RemoteTarget, RemoteTransport};
#[cfg(feature = "session-persistence")]
use crate::secret_prompt;

/// How long to wait for a daemon this process just started to publish its
/// endpoint. Generous, because the first start also creates the directory.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_POLL: Duration = Duration::from_millis(10);
/// A request must not be able to leave the terminal-opening task waiting on a
/// daemon forever. This also bounds a peer that accepted a connection but does
/// not understand the request framing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
pub struct Client {
    /// The endpoint can change while this client is alive: an in-place daemon
    /// replacement publishes a fresh token and socket.  Keeping it behind a
    /// mutex lets every pane-opening task share the repaired value without
    /// replacing the client (and, importantly, its subscription registries).
    endpoint: Mutex<Endpoint>,
    /// Where this client found its multiplexer, so a subscription that is lost
    /// can look again. Re-reading the endpoint file rather than reusing the
    /// `Endpoint` matters: a replacement publishes its own process id, and one
    /// day may publish a different socket.
    directory: PathBuf,
    /// Present for a client that reaches a remote daemon through one
    /// persistent stream-local SSH forward. The remote endpoint is refreshed
    /// through this transport rather than read from the local session
    /// directory.
    remote: Option<Arc<RemoteTransport>>,
    /// Stable across request connections and subscription reconnects.
    client_id: ClientId,
    /// Remote clients are deliberately restricted to the shared byte-stream
    /// protocol. Local clients retain descriptor handover and PID ownership.
    stream_only: bool,
    /// An authenticated remote session key used by later control requests and
    /// subscription reconciliation. It is shared by request clones and is
    /// zeroized when the logical client is dropped.
    session_secret: Arc<Mutex<Option<SessionSecret>>>,
}

/// The result of applying a retention policy to a daemon.
///
/// `requested_retention` remains the user's choice even when a temporary
/// recipient lookup makes the daemon use `effective_retention` for now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionConfiguration {
    pub requested_retention: Retention,
    pub effective_retention: Retention,
    pub degraded_reason: Option<String>,
}

/// A connected client together with the retention policy the daemon actually
/// accepted during startup.
pub struct ConfiguredClient {
    pub client: Client,
    pub requested_retention: Retention,
    pub effective_retention: Retention,
    pub degraded_reason: Option<String>,
}

#[cfg(feature = "session-persistence")]
fn resolve_effective_retention(
    retention: Retention,
    recipient_values: &[String],
    fallback_retention: Retention,
    resolve: impl FnOnce(
        &[String],
    ) -> std::result::Result<
        Vec<String>,
        crate::persistence::RecipientResolutionError,
    >,
) -> Result<(Retention, Option<String>, Vec<String>)> {
    if !matches!(retention, Retention::Disk) {
        return Ok((retention, None, Vec::new()));
    }
    match resolve(recipient_values) {
        Ok(recipients) => Ok((retention, None, recipients)),
        Err(error) if error.is_temporary() => {
            Ok((fallback_retention, Some(format!("{error:#}")), Vec::new()))
        }
        Err(error) => Err(error.into_anyhow()),
    }
}

/// A pane's terminal, as handed over by the multiplexer.
pub struct AttachedPane {
    pub session_id: u64,
    pub pane_id: u64,
    pub child_pid: u32,
    /// The PTY master. The holder reads and writes it directly, so an attached
    /// pane costs exactly what a locally spawned one costs.
    #[cfg(unix)]
    pub descriptor: OwnedFd,
    /// The pseudoconsole's pipes, duplicated into this process. A console
    /// handle itself cannot be shared, so resizing goes back to the owner.
    #[cfg(windows)]
    pub conout: std::os::windows::io::OwnedHandle,
    #[cfg(windows)]
    pub conin: std::os::windows::io::OwnedHandle,
    /// Output produced while the pane was detached, to be replayed into a
    /// fresh terminal before it is shown.
    pub replay: Vec<u8>,
}

/// A pane attached in shared mode.
///
/// No descriptor changes hands: the connection stays open and is the pane's
/// data plane — output and size events arrive on it, and input and size
/// reports go back over it. The connection is shared with the reader handed
/// to the terminal's byte-stream worker, so writes and reads run on different
/// threads and a clone of the stream serves each side.
pub struct SharedPane {
    session_id: u64,
    pane_id: u64,
    pub child_pid: u32,
    connection: Mutex<Connection>,
    /// The sizes the multiplexer arbitrated, recorded by the reader as
    /// [`Event::Size`] arrives. The holder of the pane applies the latest
    /// each time it is woken.
    sizes: Arc<Mutex<Vec<(u16, u16)>>>,
    /// Output produced while the pane was detached or shared, to be replayed
    /// into a fresh terminal before it is shown.
    pub replay: Vec<u8>,
}

impl SharedPane {
    pub fn session_id(&self) -> u64 {
        self.session_id
    }

    pub fn pane_id(&self) -> u64 {
        self.pane_id
    }

    /// Sends this client's input, to be written to the pane by the
    /// multiplexer.
    pub fn send_input(&self, bytes: &[u8]) -> Result<()> {
        let mut connection = self.connection.lock().unwrap();
        connection.send(&Request::Input {
            length: bytes.len(),
        })?;
        connection.write_all(bytes)
    }

    /// Reports this client's size, so the multiplexer can keep every shared
    /// client at the smallest of them.
    pub fn send_resize(&self, columns: u16, lines: u16) -> Result<()> {
        let mut connection = self.connection.lock().unwrap();
        connection.send(&Request::Resize {
            session_id: self.session_id,
            pane_id: self.pane_id,
            columns,
            lines,
        })
    }

    /// The reader side of the shared connection, for a terminal's byte-stream
    /// worker: parses [`Event::Output`] into plain bytes, records
    /// [`Event::Size`], and returns an I/O error when the multiplexer goes
    /// away so the terminal can report the connection lost.
    ///
    /// The multiplexer sends output in bursts around long idle stretches, so
    /// the reader uses a read timeout and translates it into `WouldBlock`,
    /// which a byte-stream worker treats as "nothing yet" rather than an
    /// error — that is also what lets a dropped terminal's worker thread
    /// notice it was asked to stop.
    pub fn reader(&self) -> SharedReader {
        let connection = self
            .connection
            .lock()
            .unwrap()
            .try_clone()
            .expect("cloning the shared connection");
        #[cfg(unix)]
        {
            connection
                .stream()
                .set_read_timeout(Some(SHARED_READ_TIMEOUT))
                .ok();
        }
        #[cfg(windows)]
        {
            connection
                .stream()
                .set_read_timeout(Some(SHARED_READ_TIMEOUT))
                .ok();
        }
        SharedReader {
            connection,
            pending: Vec::new(),
            offset: 0,
            sizes: self.sizes.clone(),
        }
    }

    /// The sizes the multiplexer arbitrated since the last call, oldest
    /// first.
    pub fn take_sizes(&self) -> Vec<(u16, u16)> {
        self.sizes.lock().unwrap().drain(..).collect()
    }
}

/// How long the shared reader blocks on the connection before returning
/// `WouldBlock` so the byte-stream worker can check whether it was stopped.
const SHARED_READ_TIMEOUT: Duration = Duration::from_millis(500);

/// Reads a shared pane's output off its connection.
///
/// This is what a terminal's byte-stream worker reads from: [`Event::Output`]
/// frames become plain bytes, [`Event::Size`] frames are recorded for the
/// pane's holder to apply, and a dead multiplexer surfaces as an I/O error.
pub struct SharedReader {
    connection: Connection,
    pending: Vec<u8>,
    offset: usize,
    sizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl io::Read for SharedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.offset < self.pending.len() {
            let available = self.pending.len() - self.offset;
            let count = available.min(buffer.len());
            buffer[..count].copy_from_slice(&self.pending[self.offset..self.offset + count]);
            self.offset += count;
            if self.offset == self.pending.len() {
                self.pending.clear();
                self.offset = 0;
            }
            return Ok(count);
        }
        loop {
            match self.connection.receive::<Event>() {
                Ok((Event::Output { length, .. }, _)) => {
                    let bytes = self
                        .connection
                        .read_exact(length)
                        .map_err(io::Error::other)?;
                    let count = bytes.len().min(buffer.len());
                    buffer[..count].copy_from_slice(&bytes[..count]);
                    self.pending = bytes[count..].to_vec();
                    return Ok(count);
                }
                Ok((Event::Size { columns, lines, .. }, _)) => {
                    self.sizes.lock().unwrap().push((columns, lines));
                }
                Ok(_) => {}
                Err(error) if is_would_block(&error) => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                // An ordinary end of stream, not a failure. The multiplexer closes
                // its end when a pane is handed back, and reporting that as an
                // error made the byte-stream worker print one into the terminal —
                // which shifted a full-screen program's display by the lines it
                // took and left the message on screen for good.
                Err(error) if is_closed(&error) => return Ok(0),
                Err(error) => return Err(io::Error::other(error)),
            }
        }
    }
}

/// Whether a connection error is really "no data arrived within the read
/// timeout", which the receive loop reports as an anyhow error with the
/// underlying I/O error somewhere in its chain.
fn is_would_block(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            )
        })
    })
}

/// Whether a connection error is the peer having finished, rather than anything
/// having gone wrong.
fn is_closed(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof)
    })
}

fn is_invalid_token_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "invalid multiplexer token")
}

fn is_transport_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::AddrNotAvailable
                    | io::ErrorKind::TimedOut
                    | io::ErrorKind::UnexpectedEof
                    | io::ErrorKind::WouldBlock
            )
        })
    })
}

fn is_unsupported_configure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let message = cause.to_string();
        message.contains("unknown variant `configure`")
            || message.contains("unknown variant 'configure'")
            || message.contains("unknown variant \"configure\"")
    })
}

/// Whether a connection insists that the multiplexer speaks this build's
/// protocol.
#[derive(Clone, Copy)]
enum VersionCheck {
    /// Every ordinary request: a client that cannot be understood must not send
    /// one, and finding that out here is what lets it fall back to a local
    /// process rather than failing to open a terminal.
    Required,
    /// `Request::Upgrade` alone, which the daemon accepts across the boundary
    /// precisely because crossing it is what the request is for.
    Tolerated,
}

impl Client {
    /// Connects to the running multiplexer, starting one if there is none.
    pub fn connect() -> Result<Self> {
        Self::connect_with_retention(Retention::default())
    }

    /// Connects to the running multiplexer, applying `retention` whether the
    /// daemon already exists or has just been started. The application
    /// resolves this before the first pane is spawned, so a constrained build
    /// cannot silently start or reuse a daemon with a different retention
    /// policy.
    pub fn connect_with_retention(retention: Retention) -> Result<Self> {
        #[cfg(feature = "session-persistence")]
        {
            Self::connect_with_retention_and_persistence(retention, PersistenceOptions::default())
        }
        #[cfg(not(feature = "session-persistence"))]
        {
            retention.validate()?;
            Self::connect_at_with_retention(&session_catalog_dir(), retention)
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_and_persistence(
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<Self> {
        retention.validate()?;
        Self::connect_at_with_retention_and_persistence(
            &session_catalog_dir(),
            retention,
            persistence,
        )
    }

    /// Connects with the application's requested retention, temporarily using
    /// `fallback_retention` when a valid GitHub recipient cannot be resolved
    /// because of a network failure.
    ///
    /// This is intentionally separate from [`Self::connect_with_retention_and_persistence`]:
    /// disk resume and CLI commands must keep reporting configuration and
    /// connectivity errors instead of silently changing their durability.
    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_and_persistence_resilient(
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<ConfiguredClient> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        Self::connect_at_with_retention_and_persistence_resilient(
            &session_catalog_dir(),
            retention,
            persistence,
            fallback_retention,
        )
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_with_retention_for_resume(retention: Retention) -> Result<Self> {
        Self::connect_at_with_retention_for_resume(&session_catalog_dir(), retention)
    }

    /// Connects only if a multiplexer is already running.
    pub fn connect_existing() -> Result<Option<Self>> {
        Self::connect_existing_at(&session_catalog_dir())
    }

    /// Connects to a multiplexer whose protocol this build may not speak.
    ///
    /// Only for `--upgrade`, which is the request that exists to cross that
    /// boundary: the daemon accepts it from a client that disagrees about the
    /// version, and refusing to *connect* on those grounds made the one way out
    /// of a mismatch unreachable — the client reported the very mismatch the
    /// command was there to resolve.
    pub fn connect_for_upgrade() -> Result<Option<Self>> {
        Self::connect_for_upgrade_at(&session_catalog_dir())
    }

    /// As [`Self::connect_for_upgrade`], against an explicit session directory.
    pub fn connect_for_upgrade_at(directory: &std::path::Path) -> Result<Option<Self>> {
        Self::connect_endpoint(directory, VersionCheck::Tolerated)
    }

    /// As [`Self::connect`], against an explicit session directory.
    ///
    /// The directory is a parameter rather than read from the environment so
    /// that tests — and, later, more than one multiplexer on a host — do not
    /// have to mutate process-global state to choose one.
    pub fn connect_at(directory: &std::path::Path) -> Result<Self> {
        Self::connect_at_with_retention(directory, Retention::default())
    }

    /// As [`Self::connect_with_retention`], against an explicit session
    /// directory. Tests use this so each daemon has isolated state.
    pub fn connect_at_with_retention(
        directory: &std::path::Path,
        retention: Retention,
    ) -> Result<Self> {
        #[cfg(feature = "session-persistence")]
        return Self::connect_at_with_retention_and_persistence(
            directory,
            retention,
            PersistenceOptions::default(),
        );
        #[cfg(not(feature = "session-persistence"))]
        {
            retention.validate()?;
            if let Some(client) = Self::connect_ready_at(directory)? {
                client.configure(retention, Vec::new())?;
                return Ok(client);
            }
            start_daemon(directory, None)?;
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            loop {
                if let Some(client) = Self::connect_ready_at(directory)? {
                    client.configure(retention, Vec::new())?;
                    return Ok(client);
                }
                anyhow::ensure!(
                    Instant::now() < deadline,
                    "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
                );
                thread::sleep(STARTUP_POLL);
            }
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_and_persistence(
        directory: &std::path::Path,
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<Self> {
        retention.validate()?;
        let resolved_recipients = if matches!(retention, Retention::Disk) {
            crate::persistence::resolve_recipient_strings(&persistence.recipients)?
        } else {
            Vec::new()
        };
        Self::connect_at_with_resolved_retention_and_persistence(
            directory,
            retention,
            resolved_recipients,
        )
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_and_persistence_resilient(
        directory: &std::path::Path,
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<ConfiguredClient> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        let (effective_retention, degraded_reason, recipients) = resolve_effective_retention(
            retention,
            &persistence.recipients,
            fallback_retention,
            crate::persistence::resolve_recipient_strings_for_startup,
        )?;
        let client = Self::connect_at_with_resolved_retention_and_persistence(
            directory,
            effective_retention,
            recipients,
        )?;
        Ok(ConfiguredClient {
            client,
            requested_retention: retention,
            effective_retention,
            degraded_reason,
        })
    }

    #[cfg(feature = "session-persistence")]
    fn connect_at_with_resolved_retention_and_persistence(
        directory: &std::path::Path,
        retention: Retention,
        resolved_recipients: Vec<String>,
    ) -> Result<Self> {
        if let Some(client) = Self::connect_ready_at(directory)? {
            client.configure(retention, resolved_recipients)?;
            return Ok(client);
        }
        start_daemon(directory, None, Some(resolved_recipients.clone()))?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(client) = Self::connect_ready_at(directory)? {
                client.configure(retention, resolved_recipients.clone())?;
                return Ok(client);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    #[cfg(unix)]
    fn configure_after_upgrade(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut last_error = None;
        loop {
            if let Some(client) = Self::connect_existing_at(&self.directory)? {
                match client.configure_raw(retention, persistence_recipients.clone()) {
                    Ok(()) => return Ok(()),
                    Err(error) if is_closed(&error) || is_unsupported_configure(&error) => {
                        last_error = Some(error);
                    }
                    Err(error) => return Err(error),
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not apply the new session retention settings within \
                 {STARTUP_TIMEOUT:?}{}",
                last_error
                    .as_ref()
                    .map(|error| format!(": {error:#}"))
                    .unwrap_or_default()
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    /// Starts a disk daemon in recovery mode, allowing it to reuse the saved
    /// public recipient set for a `resume` command that has no Zetta config in
    /// the client process.
    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_for_resume(
        directory: &std::path::Path,
        retention: Retention,
    ) -> Result<Self> {
        retention.validate()?;
        if let Some(client) = Self::connect_existing_at(directory)? {
            return Ok(client);
        }
        // Standalone `zmux resume` has no Zetta configuration to apply after
        // startup, so it still uses the explicit disk bootstrap mode. The
        // normal Zetta connection path starts with the daemon's default and
        // configures it from the loaded application settings instead.
        start_daemon(directory, Some(retention), None)?;
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(client) = Self::connect_existing_at(directory)? {
                return Ok(client);
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not start within {STARTUP_TIMEOUT:?}"
            );
            thread::sleep(STARTUP_POLL);
        }
    }

    pub fn connect_existing_at(directory: &std::path::Path) -> Result<Option<Self>> {
        Self::connect_endpoint(directory, VersionCheck::Required)
    }

    /// Connects to a multiplexer only after it has answered a request.
    ///
    /// A socket can accept a connection before the daemon has finished its
    /// startup work. The ping makes readiness mean that the request loop is
    /// actually serving the current protocol, rather than merely that a
    /// listener has been bound. `connect_for_upgrade_at` deliberately remains
    /// liveness-only so it can reach a daemon across a protocol boundary.
    pub fn connect_ready_at(directory: &std::path::Path) -> Result<Option<Self>> {
        let Some(client) = Self::connect_endpoint(directory, VersionCheck::Required)? else {
            return Ok(None);
        };
        client.ping_until_ready(Instant::now() + STARTUP_TIMEOUT)?;
        Ok(Some(client))
    }

    /// The process ID published by the daemon endpoint. It is part of a
    /// session's stable catalog identifier and lets administrative commands
    /// reject an identifier belonging to a different catalog in the same
    /// session directory.
    pub fn process_id(&self) -> u32 {
        self.endpoint.lock().unwrap().process_id
    }

    fn connect_endpoint(
        directory: &std::path::Path,
        version_check: VersionCheck,
    ) -> Result<Option<Self>> {
        let Ok(endpoint) = Endpoint::read(&endpoint_path(directory)) else {
            return Ok(None);
        };
        // Liveness first, and only then the version. An endpoint outlives the
        // daemon that wrote it, so checking the version first would let a dead
        // multiplexer's leftover file refuse every attempt to start a live
        // one — permanently, and with an error about a process that no longer
        // exists.
        if Stream::connect(&endpoint.socket_path).is_err() {
            return Ok(None);
        }
        // A multiplexer left over from an earlier build cannot serve this
        // client. Finding that out here lets the application report the
        // ownership conflict before it creates a terminal outside the daemon
        // contract.
        anyhow::ensure!(
            matches!(version_check, VersionCheck::Tolerated)
                || endpoint.protocol_version == PROTOCOL_VERSION,
            "the multiplexer running as process {} speaks protocol version {}, not \
             {PROTOCOL_VERSION}. Sessions it holds are still listed by `zmux list` but \
             cannot be attached until it is replaced; `zmux --upgrade` replaces it in place, \
             keeping them.",
            endpoint.process_id,
            endpoint.protocol_version
        );
        Ok(Some(Self {
            endpoint: Mutex::new(endpoint),
            directory: directory.to_owned(),
            remote: None,
            client_id: ClientId::random()?,
            stream_only: false,
            session_secret: Arc::new(Mutex::new(None)),
        }))
    }

    /// Connects to a remote daemon through a persistent OpenSSH stream-local
    /// forward. Remote clients never start or upgrade a daemon and never ask
    /// the remote host for a descriptor.
    pub fn connect_remote(target: RemoteTarget) -> Result<Self> {
        let remote = Arc::new(RemoteTransport::new(target)?);
        let endpoint = remote.endpoint()?;
        Ok(Self {
            endpoint: Mutex::new(endpoint),
            directory: PathBuf::new(),
            client_id: ClientId::random()?,
            stream_only: true,
            remote: Some(remote),
            session_secret: Arc::new(Mutex::new(None)),
        })
    }

    /// Testable variant of [`Self::connect_remote`] that uses a supplied SSH
    /// executable.
    #[cfg(any(test, feature = "test-support"))]
    pub fn connect_remote_with_ssh_program(
        target: RemoteTarget,
        ssh_program: impl Into<PathBuf>,
    ) -> Result<Self> {
        let remote = Arc::new(RemoteTransport::with_ssh_program(target, ssh_program)?);
        let endpoint = remote.endpoint()?;
        Ok(Self {
            endpoint: Mutex::new(endpoint),
            directory: PathBuf::new(),
            client_id: ClientId::random()?,
            stream_only: true,
            remote: Some(remote),
            session_secret: Arc::new(Mutex::new(None)),
        })
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    pub fn stream_only(&self) -> bool {
        self.stream_only
    }

    pub fn remote_target(&self) -> Option<&RemoteTarget> {
        self.remote.as_ref().map(|remote| remote.target())
    }

    /// Retains the session key needed by a remote runtime's later control
    /// requests. It is never persisted or included in an SSH argument.
    pub fn set_session_secret(&self, secret: Option<&SessionSecret>) {
        if self.stream_only {
            *self.session_secret.lock().unwrap() = secret.cloned();
        }
    }

    pub fn session_secret(&self) -> Option<SessionSecret> {
        self.session_secret.lock().unwrap().clone()
    }

    /// Makes a request-capable clone that retains the same logical client ID.
    /// Subscription recovery uses this so a daemon replacement or remote SSH
    /// forward restart does not look like a second viewer.
    fn reconnect_client(&self) -> Self {
        Self {
            endpoint: Mutex::new(self.endpoint_snapshot()),
            directory: self.directory.clone(),
            remote: self.remote.clone(),
            client_id: self.client_id.clone(),
            stream_only: self.stream_only,
            session_secret: self.session_secret.clone(),
        }
    }

    /// Attaches a pane on behalf of another process.
    ///
    /// Only useful for exercising what happens when the process holding a
    /// pane is not the one that asked for it — which is what a window dying
    /// mid-attach looks like from the multiplexer's side.
    #[cfg(any(test, feature = "test-support"))]
    pub fn attach_as(
        &self,
        session_id: u64,
        pane_id: u64,
        client_process_id: u32,
        secret: Option<String>,
    ) -> Result<AttachOutcome> {
        // Test callers use one `Client` to stand in for several processes. A
        // real process has one logical client, but those stand-ins still need
        // independent IDs so closing or resizing one shared connection cannot
        // address the other one.
        let mut client = self.reconnect_client();
        client.client_id = ClientId::random()?;
        client.attach_as_process(session_id, Some(pane_id), secret, client_process_id)
    }

    fn attach_as_process(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<String>,
        client_process_id: u32,
    ) -> Result<AttachOutcome> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let request = Request::Attach {
            session_id,
            pane_id,
            secret,
        };
        loop {
            self.ping_until_ready(deadline)?;
            let endpoint = self.endpoint_snapshot();
            let mut connection =
                self.open_as_with_endpoint(request.clone(), client_process_id, &endpoint)?;
            let (response, mut descriptors) = Self::receive(&mut connection)?;
            if matches!(
                &response,
                Response::Error { message } if message == "invalid multiplexer token"
            ) {
                self.refresh_endpoint_after_token(&endpoint, deadline)
                    .context("refreshing the multiplexer endpoint after an invalid token")?;
                continue;
            }
            return match response {
                Response::Attached {
                    pane_id,
                    child_pid,
                    replay_length,
                    state,
                    summary,
                    handles,
                } => {
                    let terminal = claim_terminal(&mut descriptors, handles)?;
                    let replay = connection.read_exact(replay_length)?;
                    Ok(AttachOutcome::Attached {
                        pane: AttachedPane {
                            session_id,
                            pane_id,
                            child_pid,
                            #[cfg(unix)]
                            descriptor: terminal,
                            #[cfg(windows)]
                            conout: terminal.0,
                            #[cfg(windows)]
                            conin: terminal.1,
                            replay,
                        },
                        state,
                        summary: *summary,
                    })
                }
                Response::SharedAttached {
                    pane_id,
                    child_pid,
                    replay_length,
                    state,
                    summary,
                    columns,
                    lines,
                } => {
                    let replay = connection.read_exact(replay_length)?;
                    Ok(AttachOutcome::SharedAttached {
                        pane: SharedPane {
                            session_id,
                            pane_id,
                            child_pid,
                            connection: Mutex::new(connection),
                            sizes: Arc::new(Mutex::new(vec![(columns, lines)])),
                            replay,
                        },
                        state,
                        summary: *summary,
                    })
                }
                Response::AuthenticationRequired => Ok(AttachOutcome::AuthenticationRequired),
                Response::AuthenticationFailed => Ok(AttachOutcome::AuthenticationFailed),
                Response::Error { message } => anyhow::bail!("{message}"),
                other => anyhow::bail!("unexpected response to attach: {other:?}"),
            };
        }
    }

    fn open(&self, request: Request) -> Result<Connection> {
        self.open_as(request, std::process::id())
    }

    fn endpoint_snapshot(&self) -> Endpoint {
        self.endpoint.lock().unwrap().clone()
    }

    /// Refreshes the cached endpoint after the daemon has rejected a request
    /// before dispatching it.  Waiting for the file to change avoids replaying
    /// the same request against the same stale token in a tight loop.
    fn refresh_endpoint_after_token(&self, previous: &Endpoint, deadline: Instant) -> Result<()> {
        if let Some(remote) = &self.remote {
            let endpoint = remote.refresh()?;
            *self.endpoint.lock().unwrap() = endpoint;
            return Ok(());
        }
        let path = endpoint_path(&self.directory);
        let mut last_error = None;
        loop {
            match Endpoint::read(&path) {
                Ok(endpoint) if endpoint != *previous => {
                    *self.endpoint.lock().unwrap() = endpoint;
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) => last_error = Some(error),
            }
            if Instant::now() >= deadline {
                return Err(last_error.unwrap_or_else(|| {
                    anyhow::anyhow!(
                        "the multiplexer endpoint did not change after an invalid token"
                    )
                }));
            }
            thread::sleep(STARTUP_POLL);
        }
    }

    fn open_ready(&self, request: Request) -> Result<Connection> {
        self.ping_until_ready(Instant::now() + STARTUP_TIMEOUT)?;
        self.open(request)
    }

    /// Opens a connection whose peer identity is settled before the request
    /// goes out.
    ///
    /// For the requests that stream raw bytes after their message: the
    /// multiplexer cannot interject a challenge into one of those, because the
    /// bytes would arrive where it was expecting the answer. Windows-only in
    /// effect — elsewhere the kernel identifies the peer from the socket and the
    /// exchange is a single round trip that changes nothing.
    fn open_attested(&self, request: Request, client_process_id: u32) -> Result<Connection> {
        #[cfg(windows)]
        {
            let mut connection = self.open_as(Request::Attest, client_process_id)?;
            match Self::receive(&mut connection)?.0 {
                Response::Ok => {}
                Response::Error { message } => anyhow::bail!("{message}"),
                other => anyhow::bail!("unexpected response to attestation: {other:?}"),
            }
            let endpoint = self.endpoint_snapshot();
            connection.send(&self.envelope(endpoint.token, client_process_id, request, None))?;
            Ok(connection)
        }
        #[cfg(unix)]
        self.open_as(request, client_process_id)
    }

    /// Receives a response, answering an attestation challenge first if the
    /// multiplexer asks for one.
    ///
    /// Only Windows ever asks: everywhere else the kernel already answers
    /// "which process is on the other end of this socket" about the socket
    /// itself. See [`crate::transport::PeerChallenge`].
    fn receive(connection: &mut Connection) -> Result<(Response, Descriptors)> {
        let received = connection.receive::<Response>()?;
        #[cfg(windows)]
        if let Response::AttestationRequired { handle } = received.0 {
            let nonce = crate::transport::answer_challenge(handle)?;
            connection.send(&Request::Attested { nonce })?;
            return Ok(connection.receive::<Response>()?);
        }
        Ok(received)
    }

    fn open_as(&self, request: Request, client_process_id: u32) -> Result<Connection> {
        let endpoint = self.endpoint_snapshot();
        self.open_as_with_endpoint(request, client_process_id, &endpoint)
    }

    fn open_with_session_secret(
        &self,
        request: Request,
        secret: Option<&SessionSecret>,
    ) -> Result<Connection> {
        self.open_as_with_endpoint_and_secret(
            request,
            std::process::id(),
            &self.endpoint_snapshot(),
            secret,
        )
    }

    fn open_as_with_endpoint(
        &self,
        request: Request,
        client_process_id: u32,
        endpoint: &Endpoint,
    ) -> Result<Connection> {
        self.open_as_with_endpoint_and_secret(request, client_process_id, endpoint, None)
    }

    fn open_as_with_endpoint_and_secret(
        &self,
        request: Request,
        client_process_id: u32,
        endpoint: &Endpoint,
        secret: Option<&SessionSecret>,
    ) -> Result<Connection> {
        let (endpoint, stream) = if let Some(remote) = &self.remote {
            let (endpoint, stream) = remote.connect()?;
            *self.endpoint.lock().unwrap() = endpoint.clone();
            (endpoint, stream)
        } else {
            (
                endpoint.clone(),
                Stream::connect(&endpoint.socket_path).context("connecting to the multiplexer")?,
            )
        };
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request read timeout")?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request write timeout")?;
        let mut connection = Connection::new(stream);
        connection.send(&self.envelope(endpoint.token, client_process_id, request, secret))?;
        Ok(connection)
    }

    fn envelope(
        &self,
        token: String,
        client_process_id: u32,
        request: Request,
        secret: Option<&SessionSecret>,
    ) -> Envelope {
        Envelope {
            version: PROTOCOL_VERSION,
            token,
            // Named so a platform without descriptor passing can duplicate a
            // terminal's handles into this process instead.
            client_process_id,
            client_id: self.client_id.clone(),
            stream_only: self.stream_only,
            session_secret: secret.map(|secret| secret.expose().to_owned()),
            request,
        }
    }

    fn ping_with_endpoint(&self, endpoint: &Endpoint) -> Result<()> {
        let mut connection =
            self.open_as_with_endpoint(Request::Ping, std::process::id(), endpoint)?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to ping: {other:?}"),
        }
    }

    fn ping_until_ready(&self, deadline: Instant) -> Result<()> {
        loop {
            let endpoint = self.endpoint_snapshot();
            match self.ping_with_endpoint(&endpoint) {
                Ok(()) => return Ok(()),
                Err(error) if is_invalid_token_error(&error) => {
                    self.refresh_endpoint_after_token(&endpoint, deadline)
                        .with_context(
                            || "refreshing the multiplexer endpoint after an invalid token",
                        )?;
                }
                Err(error) if is_transport_error(&error) && Instant::now() < deadline => {
                    thread::sleep(STARTUP_POLL);
                }
                Err(error) => return Err(error),
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "the multiplexer did not become ready within {STARTUP_TIMEOUT:?}"
            );
        }
    }

    /// Tells the multiplexer that an attached pane was resized.
    ///
    /// Only meaningful where the console belongs to the multiplexer; on Unix
    /// the resize has already taken effect through the descriptor.
    pub fn resize(&self, session_id: u64, pane_id: u64, columns: u16, lines: u16) -> Result<()> {
        self.resize_with_secret(session_id, pane_id, columns, lines, None)
    }

    pub fn resize_with_secret(
        &self,
        session_id: u64,
        pane_id: u64,
        columns: u16,
        lines: u16,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::Resize {
                session_id,
                pane_id,
                columns,
                lines,
            },
            secret,
        )?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to resize: {other:?}"),
        }
    }

    pub fn set_console_palette(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
    ) -> Result<()> {
        self.set_console_palette_with_secret(session_id, pane_id, palette, None)
    }

    pub fn set_console_palette_with_secret(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::SetConsolePalette {
                session_id,
                pane_id,
                palette,
            },
            secret,
        )?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected palette response: {other:?}"),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn set_console_palette_as(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        client_process_id: u32,
    ) -> Result<()> {
        let mut connection = self.open_as(
            Request::SetConsolePalette {
                session_id,
                pane_id,
                palette,
            },
            client_process_id,
        )?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected palette response: {other:?}"),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_console_palette_for_process(
        &self,
        session_id: u64,
        pane_id: u64,
        palette: ConsolePalette,
        client_process_id: u32,
    ) -> Result<()> {
        self.set_console_palette_as(session_id, pane_id, palette, client_process_id)
    }

    /// Sends a screen checkpoint for a pane this process is showing.
    ///
    /// During a revoke this is the screen handed back so the multiplexer can
    /// resume reading and relay the pane to every client that attaches. A live
    /// share uses the same request before publishing the session, while this
    /// process remains exclusive, so disk persistence has a current screen to
    /// save even if the window never gets as far as backgrounding the tab.
    ///
    /// `columns`/`lines` are the size the pane was being shown at, which the
    /// multiplexer records as the pane's size for shared clients to join at.
    pub fn send_snapshot(
        &self,
        session_id: u64,
        pane_id: u64,
        bytes: Vec<u8>,
        columns: u16,
        lines: u16,
    ) -> Result<()> {
        let mut connection = self.open_attested(
            Request::Snapshot {
                session_id,
                pane_id,
                length: bytes.len(),
                columns,
                lines,
            },
            std::process::id(),
        )?;
        connection.write_all(&bytes)?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to snapshot: {other:?}"),
        }
    }

    /// Starts a process under the multiplexer and takes its terminal.
    pub fn spawn(&self, request: SpawnRequest) -> Result<AttachedPane> {
        let mut connection = self.open(Request::Spawn(request))?;
        let (response, mut descriptors) = Self::receive(&mut connection)?;
        match response {
            Response::Spawned {
                session_id,
                pane_id,
                child_pid,
                handles,
            } => {
                let terminal = claim_terminal(&mut descriptors, handles)?;
                Ok(AttachedPane {
                    session_id,
                    pane_id,
                    child_pid,
                    #[cfg(unix)]
                    descriptor: terminal,
                    #[cfg(windows)]
                    conout: terminal.0,
                    #[cfg(windows)]
                    conin: terminal.1,
                    replay: Vec::new(),
                })
            }
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to spawn: {other:?}"),
        }
    }

    /// Takes a held pane's terminal back, with everything retained since it was
    /// detached.
    /// Takes a held pane back. `pane_id` is `None` for the session's first
    /// pane, which is where an attach starts.
    pub fn attach(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<String>,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(session_id, pane_id, secret, std::process::id())
    }

    /// Attaches with a secret kept in zeroizing memory by the caller. The
    /// serialized request is discarded as soon as the handshake is complete.
    pub fn attach_with_secret(
        &self,
        session_id: u64,
        pane_id: Option<u64>,
        secret: Option<&SessionSecret>,
    ) -> Result<AttachOutcome> {
        self.attach_as_process(
            session_id,
            pane_id,
            secret.map(|secret| secret.expose().to_owned()),
            std::process::id(),
        )
    }

    /// Gives a session back to the multiplexer to hold.
    ///
    /// The caller must already have dropped the panes' descriptors: the
    /// multiplexer resumes reading as soon as this returns, and two readers
    /// would split the output between them.
    pub fn detach(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier: protection.map(|protection| protection.verifier().to_owned()),
            key_envelope: protection
                .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            snapshots: snapshots
                .iter()
                .map(|(pane_id, bytes)| PaneSnapshot {
                    pane_id: *pane_id,
                    length: bytes.len(),
                })
                .collect(),
        };
        let mut connection = self.open_attested(Request::Detach(request), std::process::id())?;
        for (_, bytes) in &snapshots {
            connection.write_all(bytes)?;
        }
        match Self::receive(&mut connection)?.0 {
            Response::Detached => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to detach: {other:?}"),
        }
    }

    /// Detaches on behalf of another process.
    ///
    /// Only useful for exercising what a session's scope does after the window
    /// that backgrounded it has gone: the session belongs to the process that
    /// detached it, and a test cannot exit itself to prove that outlives it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn detach_as(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        client_process_id: u32,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier: protection.map(|protection| protection.verifier().to_owned()),
            key_envelope: protection
                .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            snapshots: Vec::new(),
        };
        let mut connection = self.open_attested(Request::Detach(request), client_process_id)?;
        match Self::receive(&mut connection)?.0 {
            Response::Detached => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to detach: {other:?}"),
        }
    }

    /// Offers, or withdraws, a session this client is still showing.
    ///
    /// Unlike [`Client::detach`] this changes nothing about who is reading the
    /// session's terminals: it publishes what a joining client needs and says
    /// whether the session is on offer. Only a client holding one of the
    /// session's panes may send it.
    pub fn share(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        offered: bool,
    ) -> Result<()> {
        self.share_with_secret(session_id, summary, state, protection, offered, None)
    }

    pub fn share_with_secret(
        &self,
        session_id: u64,
        summary: BackgroundSessionSummary,
        state: serde_json::Value,
        protection: Option<&SessionAuthentication>,
        offered: bool,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::Share(crate::messages::ShareRequest {
                session_id,
                summary,
                state,
                verifier: protection.map(|protection| protection.verifier().to_owned()),
                key_envelope: protection
                    .and_then(|protection| protection.key_envelope().map(str::to_owned)),
                offered,
            }),
            secret,
        )?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to share: {other:?}"),
        }
    }

    pub fn list(&self) -> Result<Vec<BackgroundSessionSummary>> {
        self.list_with_secret(None)
    }

    pub fn list_with_secret(
        &self,
        secret: Option<&SessionSecret>,
    ) -> Result<Vec<BackgroundSessionSummary>> {
        let mut connection = self.open_with_session_secret(Request::List, secret)?;
        match Self::receive(&mut connection)?.0 {
            Response::Sessions { sessions, .. } => Ok(sessions),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to list: {other:?}"),
        }
    }

    pub fn list_with_restorable(
        &self,
    ) -> Result<(Vec<BackgroundSessionSummary>, Vec<RestorableSessionRecord>)> {
        let mut connection = self.open(Request::List)?;
        match Self::receive(&mut connection)?.0 {
            Response::Sessions {
                sessions,
                restorable,
            } => Ok((sessions, restorable)),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to list: {other:?}"),
        }
    }

    #[cfg(feature = "session-persistence")]
    pub fn resume(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
    ) -> Result<crate::persistence::PersistedSession> {
        let identities = crate::persistence::IdentitySet::from_paths(identity_paths)?;
        let persisted = crate::persistence::load_session_from_directory(
            &self.directory,
            record_id,
            &identities,
        )?;
        // Only asked for when the record's protection is a typed secret. An
        // automatically protected one is opened by `resume_loaded` from the
        // envelope inside the record, using the identity that just decrypted it.
        let secret = persisted
            .verifier
            .as_ref()
            .filter(|_| persisted.key_envelope.is_none())
            .map(|_| secret_prompt::prompt_for_reconnect_secret())
            .transpose()?;
        self.resume_loaded(persisted, secret.as_ref(), &identities)
    }

    /// Resumes a record after the caller has handled any UI-specific secret
    /// prompt. The age identity is still loaded and the record is still
    /// decrypted here, so the daemon receives neither of them.
    #[cfg(feature = "session-persistence")]
    pub fn resume_with_secret(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
        secret: Option<&SessionSecret>,
    ) -> Result<crate::persistence::PersistedSession> {
        self.resume_with_secret_and_identity_passphrases(record_id, identity_paths, &[], secret)
    }

    /// Resumes a record after the caller has handled both UI-specific prompts.
    /// Identity passphrases are positional with `identity_paths`; they stay in
    /// the client and are never included in the daemon request.
    #[cfg(feature = "session-persistence")]
    pub fn resume_with_secret_and_identity_passphrases(
        &self,
        record_id: u64,
        identity_paths: &[PathBuf],
        identity_passphrases: &[Option<SessionSecret>],
        secret: Option<&SessionSecret>,
    ) -> Result<crate::persistence::PersistedSession> {
        // Only what the caller collected: this is the entry point a window uses,
        // and `age`'s own fallback would read `/dev/tty` from the UI thread.
        let identities = crate::persistence::IdentitySet::from_supplied_passphrases(
            identity_paths,
            identity_passphrases,
        )?;
        let persisted = crate::persistence::load_session_from_directory(
            &self.directory,
            record_id,
            &identities,
        )?;
        self.resume_loaded(persisted, secret, &identities)
    }

    /// Sends a decrypted record to the daemon, settling its secret first.
    ///
    /// A record protected automatically carries its own way in, and the
    /// identities that opened the record are the identities that open that too —
    /// so the secret is recovered here rather than asked for by every caller. A
    /// caller that already has one (a typed secret, or one it recovered itself)
    /// keeps it: this fills a gap, it does not overrule.
    #[cfg(feature = "session-persistence")]
    fn resume_loaded(
        &self,
        persisted: crate::persistence::PersistedSession,
        secret: Option<&SessionSecret>,
        identities: &crate::persistence::IdentitySet,
    ) -> Result<crate::persistence::PersistedSession> {
        let recovered = match (secret, persisted.key_envelope.as_deref()) {
            (None, Some(envelope)) => Some(crate::auto_protect::open(envelope, identities)?),
            _ => None,
        };
        let secret = secret.or(recovered.as_ref());
        let request = Request::Resume(ResumeRequest {
            record_id: persisted.id,
            summary: persisted.summary.clone(),
            state: persisted.state.clone(),
            verifier: persisted.verifier.clone(),
            key_envelope: persisted.key_envelope.clone(),
            failed_authentications: persisted.failed_authentications,
            backoff_seconds: persisted.backoff_seconds,
            created_at: persisted.created_at,
            updated_at: persisted.updated_at,
            secret: secret.map(|secret| secret.expose().to_owned()),
            snapshots: persisted
                .snapshots
                .iter()
                .map(|snapshot| ResumeSnapshot {
                    pane_id: snapshot.pane_id,
                    length: snapshot.bytes.len(),
                })
                .collect(),
        });
        let mut connection = self.open_attested(request, std::process::id())?;
        for snapshot in &persisted.snapshots {
            connection.write_all(&snapshot.bytes)?;
        }
        match Self::receive(&mut connection)?.0 {
            Response::Resumed { .. } => Ok(persisted),
            Response::AuthenticationRequired => {
                anyhow::bail!("the encrypted session is protected and needs its session secret")
            }
            Response::AuthenticationFailed => anyhow::bail!("session authentication failed"),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to resume: {other:?}"),
        }
    }

    pub fn kill(&self, session_id: u64) -> Result<()> {
        self.kill_with_secret(session_id, None)
    }

    pub fn kill_with_secret(&self, session_id: u64, secret: Option<&SessionSecret>) -> Result<()> {
        let mut connection = self.open_with_session_secret(Request::Kill { session_id }, secret)?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to kill: {other:?}"),
        }
    }

    /// Removes a session from the catalog without killing it.
    /// Scopes a backgrounded session to one process, or shares it with all.
    ///
    /// `protection` is what a joining process will have to present, and is
    /// required when sharing a session that has none.
    pub fn set_session_scope(
        &self,
        session_id: u64,
        shared: bool,
        protection: Option<&SessionAuthentication>,
    ) -> Result<()> {
        self.set_session_scope_with_secret(session_id, shared, protection, None)
    }

    pub fn set_session_scope_with_secret(
        &self,
        session_id: u64,
        shared: bool,
        protection: Option<&SessionAuthentication>,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection = self.open_with_session_secret(
            Request::SetSessionScope {
                session_id,
                shared,
                verifier: protection.map(|protection| protection.verifier().to_owned()),
                key_envelope: protection
                    .and_then(|protection| protection.key_envelope().map(str::to_owned)),
            },
            secret,
        )?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to a session scope change: {other:?}"),
        }
    }

    pub fn forget(&self, session_id: u64) -> Result<()> {
        self.forget_with_secret(session_id, None)
    }

    pub fn forget_with_secret(
        &self,
        session_id: u64,
        secret: Option<&SessionSecret>,
    ) -> Result<()> {
        let mut connection =
            self.open_with_session_secret(Request::Forget { session_id }, secret)?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to forget: {other:?}"),
        }
    }

    /// Applies the retention settings selected by the current client to a
    /// daemon that may have been started by an earlier client.
    ///
    /// A daemon from before `Configure` existed rejects the request as an
    /// unknown variant. On Unix, that is recoverable: replace the daemon in
    /// place, wait for the new image to answer, and send the same effective
    /// settings again. The retry deliberately uses [`Self::configure_raw`]
    /// rather than this method, because the replacement is expected to be
    /// current and must not trigger a second upgrade while the first one is
    /// settling.
    pub fn configure(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        retention.validate()?;
        match self.configure_raw(retention, persistence_recipients.clone()) {
            Ok(()) => Ok(()),
            Err(error) if is_unsupported_configure(&error) => {
                #[cfg(unix)]
                {
                    self.upgrade()
                        .context("upgrading the multiplexer to apply retention settings")?;
                    self.configure_after_upgrade(retention, persistence_recipients)
                }
                #[cfg(not(unix))]
                {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Sends a single wire-level configure request.
    ///
    /// This is intentionally separate from [`Self::configure`]. The latter
    /// may have just replaced the daemon, so retrying it here would attempt a
    /// recursive upgrade instead of merely applying the requested settings to
    /// the replacement.
    fn configure_raw(
        &self,
        retention: Retention,
        persistence_recipients: Vec<String>,
    ) -> Result<()> {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let request = Request::Configure {
            retention,
            persistence_recipients,
        };
        loop {
            self.ping_until_ready(deadline)?;
            let endpoint = self.endpoint_snapshot();
            let mut connection =
                self.open_as_with_endpoint(request.clone(), std::process::id(), &endpoint)?;
            let (response, _) = Self::receive(&mut connection)?;
            if matches!(
                &response,
                Response::Error { message } if message == "invalid multiplexer token"
            ) {
                self.refresh_endpoint_after_token(&endpoint, deadline)
                    .context("refreshing the multiplexer endpoint after an invalid token")?;
                continue;
            }
            return match response {
                Response::Ok => Ok(()),
                Response::Error { message } => anyhow::bail!("{message}"),
                other => anyhow::bail!("unexpected response to daemon configuration: {other:?}"),
            };
        }
    }

    /// Resolves and applies the persistence settings from the current Zetta
    /// configuration without creating a second client connection.
    #[cfg(feature = "session-persistence")]
    pub fn configure_with_retention_and_persistence(
        &self,
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<()> {
        retention.validate()?;
        let recipients = if matches!(retention, Retention::Disk) {
            crate::persistence::resolve_recipient_strings(&persistence.recipients)?
        } else {
            Vec::new()
        };
        self.configure(retention, recipients)
    }

    /// Applies persistence settings while allowing a temporary GitHub lookup
    /// failure to leave the daemon in the supplied in-memory mode.
    #[cfg(feature = "session-persistence")]
    pub fn configure_with_retention_and_persistence_resilient(
        &self,
        retention: Retention,
        persistence: PersistenceOptions,
        fallback_retention: Retention,
    ) -> Result<RetentionConfiguration> {
        retention.validate()?;
        fallback_retention.validate()?;
        anyhow::ensure!(
            matches!(fallback_retention, Retention::Memory { .. }),
            "retention fallback must be an in-memory policy"
        );
        let (effective_retention, degraded_reason, recipients) = resolve_effective_retention(
            retention,
            &persistence.recipients,
            fallback_retention,
            crate::persistence::resolve_recipient_strings_for_startup,
        )?;
        self.configure(effective_retention, recipients)?;
        Ok(RetentionConfiguration {
            requested_retention: retention,
            effective_retention,
            degraded_reason,
        })
    }

    /// Asks the daemon to replace itself, keeping its sessions.
    pub fn upgrade(&self) -> Result<()> {
        let mut connection = self.open(Request::Upgrade)?;
        let response = Self::receive(&mut connection)?.0;
        match response {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to upgrade: {other:?}"),
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut connection = self.open(Request::Shutdown)?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to shutdown: {other:?}"),
        }
    }

    /// Asks what the multiplexer currently knows about these panes.
    pub fn pane_states(&self, pane_ids: Vec<u64>) -> Result<Vec<PaneStateReport>> {
        let secret = self.session_secret();
        self.pane_states_with_secret(pane_ids, secret.as_ref())
    }

    pub fn pane_states_with_secret(
        &self,
        pane_ids: Vec<u64>,
        secret: Option<&SessionSecret>,
    ) -> Result<Vec<PaneStateReport>> {
        let mut connection =
            self.open_with_session_secret(Request::PaneStates { pane_ids }, secret)?;
        match Self::receive(&mut connection)?.0 {
            Response::PaneStates { panes } => Ok(panes),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to pane states: {other:?}"),
        }
    }

    /// Hands a pane back because the window showing it has closed.
    pub fn close_pane(&self, session_id: u64, pane_id: u64) -> Result<()> {
        let mut connection = self.open(Request::ClosePane {
            session_id,
            pane_id,
        })?;
        match Self::receive(&mut connection)?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to close pane: {other:?}"),
        }
    }

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
        let connection = self.open_ready(Request::Subscribe)?;
        // Subscription connections are intentionally long-lived and may be
        // idle for hours. Keep the write deadline from `open_as`, but remove
        // the request read deadline before handing the connection to the
        // event loop.
        connection
            .stream()
            .set_read_timeout(None)
            .context("clearing the multiplexer subscription read timeout")?;
        let exits = Arc::new(ExitReporters::default());
        let revokes = Arc::new(PaneSignals::default());
        let grants = Arc::new(PaneSignals::default());
        let subscription = Subscription {
            exits,
            revokes,
            grants,
        };
        let dispatch = subscription.clone();
        let client = self.reconnect_client();
        thread::spawn(move || {
            subscription_loop(client, connection, dispatch);
        });
        Ok(subscription)
    }

    /// Takes a shared pane's terminal back, in answer to [`Event::Grant`].
    ///
    /// Sent on its own connection: the shared one is being retired, and the
    /// multiplexer closes its end of it once the last relayed frame has gone, which
    /// is how this client knows it has everything before it starts reading the
    /// terminal itself.
    pub fn take_exclusive(&self, session_id: u64, pane_id: u64) -> Result<AttachedPane> {
        let mut connection = self.open(Request::TakeExclusive {
            session_id,
            pane_id,
        })?;
        let (response, mut descriptors) = Self::receive(&mut connection)?;
        match response {
            Response::Attached {
                pane_id,
                child_pid,
                replay_length,
                handles,
                ..
            } => {
                let terminal = claim_terminal(&mut descriptors, handles)?;
                // Nothing to replay: everything the multiplexer read went to this
                // client over the relay, and replaying it would print the pane's
                // recent output a second time.
                anyhow::ensure!(
                    replay_length == 0,
                    "a granted pane must not carry a replay, but {replay_length} bytes came with it"
                );
                Ok(AttachedPane {
                    session_id,
                    pane_id,
                    child_pid,
                    #[cfg(unix)]
                    descriptor: terminal,
                    #[cfg(windows)]
                    conout: terminal.0,
                    #[cfg(windows)]
                    conin: terminal.1,
                    replay: Vec::new(),
                })
            }
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to taking a pane back: {other:?}"),
        }
    }
}

/// The registries a subscription delivers into, handed out together because the
/// events arrive on one connection and a caller needs all of them.
#[derive(Clone)]
pub struct Subscription {
    pub exits: Arc<ExitReporters>,
    pub revokes: Arc<PaneSignals>,
    pub grants: Arc<PaneSignals>,
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
const RESUBSCRIBE_GRACE: Duration = Duration::from_secs(60);

/// The first and longest gaps between attempts to find a multiplexer again.
///
/// Backed off rather than flat, because each attempt costs the *daemon* a
/// connection to accept and a thread to serve it — twice, since establishing a
/// client probes for liveness before subscribing. Retrying every few
/// milliseconds for a minute is thousands of those, aimed at a replacement that
/// is starting up and is exactly when it can least afford them; enough of it
/// starves the daemon of threads. The first gap stays short so an upgrade, which
/// is over in well under a second, is not waited on needlessly.
const RESUBSCRIBE_FIRST_DELAY: Duration = Duration::from_millis(20);
const RESUBSCRIBE_MAX_DELAY: Duration = Duration::from_millis(500);

/// Reads events for as long as any multiplexer is reachable.
///
/// Returns only once the grace period has passed with nothing to talk to, at
/// which point the panes really are unreportable and are told so.
fn subscription_loop(client: Client, first: Connection, subscription: Subscription) {
    let Subscription {
        exits: reporters,
        revokes,
        grants,
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
                Ok(_) => {}
                Err(_) => break,
            }
        }
        if announced {
            log::debug!("the multiplexer is replacing itself; waiting for the replacement");
        } else {
            log::warn!("lost the multiplexer's event stream; trying to re-establish it");
        }
        match resubscribe(&client, &reporters, &revokes, &grants) {
            Some(Resubscribed::Connection(next)) => connection = next,
            // Nobody is left to tell. Reporting a disconnect here would be
            // writing into registries that no longer have an owner.
            Some(Resubscribed::Abandoned) => return,
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

enum Resubscribed {
    Connection(Connection),
    /// The registries' owner is gone, so this subscription has no purpose left.
    Abandoned,
}

/// Whether anything outside this thread still holds the registries.
///
/// The thread owns one reference to each; if that is the only one, whoever
/// subscribed has been dropped, nothing can register a pane again, and nothing
/// could observe a report if one arrived. Without this check a subscription whose
/// owner went away kept retrying for the whole grace period — a thread and a
/// steady trickle of connection attempts per closed window, and in a test suite
/// enough of them at once to exhaust the process's threads.
fn subscription_is_abandoned(
    reporters: &Arc<ExitReporters>,
    revokes: &Arc<PaneSignals>,
    grants: &Arc<PaneSignals>,
) -> bool {
    Arc::strong_count(reporters) == 1
        && Arc::strong_count(revokes) == 1
        && Arc::strong_count(grants) == 1
}

/// Re-establishes the event stream, then reports whatever was missed.
fn resubscribe(
    client: &Client,
    reporters: &Arc<ExitReporters>,
    revokes: &Arc<PaneSignals>,
    grants: &Arc<PaneSignals>,
) -> Option<Resubscribed> {
    let deadline = Instant::now() + RESUBSCRIBE_GRACE;
    let mut delay = RESUBSCRIBE_FIRST_DELAY;
    loop {
        if subscription_is_abandoned(reporters, revokes, grants) {
            return Some(Resubscribed::Abandoned);
        }
        if let Ok(connection) = client.open_ready(Request::Subscribe) {
            log::info!("re-established the multiplexer's event stream");
            // Subscribe first, then reconcile: an exit that happens between the
            // two arrives on the new subscription, whereas one reported between
            // a reconcile and a subscribe would fall down the same gap this is
            // closing.
            reconcile_missed_exits(&client, reporters);
            return Some(Resubscribed::Connection(connection));
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
fn reconcile_missed_exits(client: &Client, reporters: &Arc<ExitReporters>) {
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

pub enum AttachOutcome {
    Attached {
        pane: AttachedPane,
        state: serde_json::Value,
        summary: BackgroundSessionSummary,
    },
    /// The pane is shared: the connection stayed open and is the data plane.
    SharedAttached {
        pane: SharedPane,
        state: serde_json::Value,
        summary: BackgroundSessionSummary,
    },
    AuthenticationRequired,
    AuthenticationFailed,
}

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
struct ExitReporterState {
    reporters: HashMap<u64, alacritty_terminal::tty::AttachedChildEvents>,
    shared: HashMap<u64, async_channel::Sender<PaneExitReport>>,
    /// Exits reported for a pane that had no reporter yet.
    ///
    /// The multiplexer starts the process the moment it is asked, so a pane can
    /// end before the terminal showing it has been built — a bad shell, a
    /// failing `exec`, an instant command, a fast Ctrl-D. Dropping the report
    /// then left the terminal waiting for an event that had already happened,
    /// with nothing able to produce it a second time.
    pending: HashMap<u64, PaneExitReport>,
}

#[derive(Default)]
pub struct ExitReporters {
    /// Registration and pending delivery must be one atomic state transition:
    /// otherwise a report can observe no reporter, registration can observe no
    /// pending report, and the report can then be inserted after registration
    /// has finished checking.
    state: Mutex<ExitReporterState>,
}

impl ExitReporters {
    pub fn register(&self, pane_id: u64, reporter: alacritty_terminal::tty::AttachedChildEvents) {
        let delivery = {
            let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
        state.reporters.remove(&pane_id);
        state.pending.remove(&pane_id);
    }

    /// The panes still waiting to be told their process ended.
    ///
    /// Both registries, because either kind of holder needs catching up after a
    /// subscription is re-established.
    pub fn registered(&self) -> Vec<u64> {
        let state = self.state.lock().unwrap();
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
            let mut state = self.state.lock().unwrap();
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
        let mut state = self.state.lock().unwrap();
        state.shared.remove(&pane_id);
        state.pending.remove(&pane_id);
    }

    fn report(&self, pane_id: u64, raw_status: Option<i32>, input_sent: bool) {
        let report = PaneExitReport {
            raw_status,
            input_sent,
            disconnected: false,
        };
        let (reporter, shared) = {
            let mut state = self.state.lock().unwrap();
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

    fn report_all_disconnected(&self) {
        let (reporters, shared) = {
            let mut state = self.state.lock().unwrap();
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

    fn deliver_attached(
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

    fn deliver_shared(reporter: async_channel::Sender<PaneExitReport>, report: PaneExitReport) {
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
    reporters: Mutex<HashMap<u64, async_channel::Sender<()>>>,
    /// Signals that arrived before the pane had somewhere to deliver them.
    ///
    /// The multiplexer gives a holder a bounded time to answer a revoke, so a
    /// revoke dropped for a pane that was still being built costs the attaching
    /// client that whole timeout and then fails it.
    pending: Mutex<std::collections::HashSet<u64>>,
}

impl PaneSignals {
    pub fn register(&self, pane_id: u64, reporter: async_channel::Sender<()>) {
        self.reporters.lock().unwrap().insert(pane_id, reporter);
        if self.pending.lock().unwrap().remove(&pane_id) {
            self.report(pane_id);
        }
    }

    pub fn forget(&self, pane_id: u64) {
        self.reporters.lock().unwrap().remove(&pane_id);
        self.pending.lock().unwrap().remove(&pane_id);
    }

    fn report(&self, pane_id: u64) {
        let mut reporters = self.reporters.lock().unwrap();
        if let Some(reporter) = reporters.remove(&pane_id) {
            let _ = reporter.try_send(());
            return;
        }
        drop(reporters);
        self.pending.lock().unwrap().insert(pane_id);
    }
}

/// Starts a detached daemon that outlives this process.
///
/// Zetta passes `None` for `startup_retention`: it applies the configuration
/// through `Configure` after the fresh daemon publishes its endpoint. The
/// standalone disk-resume path supplies a bootstrap mode because it has no
/// Zetta configuration to apply.
fn start_daemon(
    directory: &std::path::Path,
    startup_retention: Option<Retention>,
    #[cfg(feature = "session-persistence")] persistence_recipients: Option<Vec<String>>,
) -> Result<()> {
    let (executable, mut arguments) = multiplexer_command()?;
    append_startup_retention_arguments(&mut arguments, startup_retention);
    #[cfg(feature = "session-persistence")]
    if let Some(recipients) = persistence_recipients
        && !recipients.is_empty()
    {
        crate::catalog::create_private_dir(directory)?;
        let options_path = directory.join(format!("daemon-options-{}.json", std::process::id()));
        let options = serde_json::to_vec(&DaemonOptionsFile { recipients })?;
        crate::catalog::write_private_file(&options_path, &options)?;
        arguments.push(format!("--daemon-options={}", options_path.display()));
    }
    #[cfg(not(feature = "session-persistence"))]
    let _ = directory;
    Command::new(&executable)
        .args(&arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting the multiplexer {}", executable.display()))?;
    Ok(())
}

fn append_startup_retention_arguments(arguments: &mut Vec<String>, retention: Option<Retention>) {
    let Some(retention) = retention else {
        return;
    };
    arguments.extend(["--retention".to_owned(), retention.name().to_owned()]);
    if let Retention::Memory { bytes } = retention {
        arguments.extend(["--retention-bytes".to_owned(), bytes.to_string()]);
    }
}

/// How to start the multiplexer: the `zmux` binary beside this executable, or
/// this executable's own `mux` subcommand when there is none.
///
/// Resolved from this process's own location rather than `PATH`, so an
/// unrelated `zmux` earlier in the path cannot be handed a session's terminals.
fn multiplexer_command() -> Result<(PathBuf, Vec<String>)> {
    let current = std::env::current_exe().context("locating this executable")?;
    let current_name = current
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if current_name == "zmux" || (cfg!(windows) && current_name.eq_ignore_ascii_case("zmux.exe")) {
        return Ok((current, vec!["--daemon".to_owned()]));
    }
    let beside = current.with_file_name(if cfg!(windows) { "zmux.exe" } else { "zmux" });
    if beside.is_file() {
        return Ok((beside, vec!["--daemon".to_owned()]));
    }
    Ok((current, vec!["mux".to_owned(), "--daemon".to_owned()]))
}

#[cfg(test)]
#[path = "tests/client.rs"]
mod tests;

/// Takes ownership of the terminal the multiplexer handed over.
#[cfg(unix)]
fn claim_terminal(descriptors: &mut Vec<OwnedFd>, _handles: Vec<i64>) -> Result<OwnedFd> {
    anyhow::ensure!(
        descriptors.len() == 1,
        "the multiplexer did not hand over a terminal"
    );
    Ok(descriptors.remove(0))
}

#[cfg(windows)]
fn claim_terminal(
    _descriptors: &mut Vec<()>,
    handles: Vec<i64>,
) -> Result<(
    std::os::windows::io::OwnedHandle,
    std::os::windows::io::OwnedHandle,
)> {
    anyhow::ensure!(
        handles.len() == 2,
        "the multiplexer did not hand over a console's pipes"
    );
    let mut claimed = crate::transport::claim_duplicated(&handles);
    let conin = claimed.remove(1);
    let conout = claimed.remove(0);
    Ok((conout, conin))
}
