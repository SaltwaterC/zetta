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

use anyhow::{Context as _, Result};

#[cfg(unix)]
use std::os::fd::OwnedFd;

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

#[cfg(feature = "session-persistence")]
use crate::{auth::SessionSecret, secret_prompt};

/// How long to wait for a daemon this process just started to publish its
/// endpoint. Generous, because the first start also creates the directory.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const STARTUP_POLL: Duration = Duration::from_millis(10);
/// A request must not be able to leave the terminal-opening task waiting on a
/// daemon forever. This also bounds a peer that accepted a connection but does
/// not understand the request framing.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
pub struct Client {
    endpoint: Endpoint,
    /// Where this client found its multiplexer, so a subscription that is lost
    /// can look again. Re-reading the endpoint file rather than reusing the
    /// `Endpoint` matters: a replacement publishes its own process id, and one
    /// day may publish a different socket.
    directory: PathBuf,
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

    /// Connects to the running multiplexer, starting one with `retention` if
    /// there is none. The application resolves this before the first pane is
    /// spawned, so a constrained build cannot silently start a daemon with a
    /// different retention policy.
    pub fn connect_with_retention(retention: Retention) -> Result<Self> {
        #[cfg(feature = "session-persistence")]
        {
            return Self::connect_with_retention_and_persistence(
                retention,
                PersistenceOptions::default(),
            );
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
            if let Some(client) = Self::connect_existing_at(directory)? {
                return Ok(client);
            }
            start_daemon(directory, retention)?;
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
    }

    #[cfg(feature = "session-persistence")]
    pub fn connect_at_with_retention_and_persistence(
        directory: &std::path::Path,
        retention: Retention,
        persistence: PersistenceOptions,
    ) -> Result<Self> {
        retention.validate()?;
        if let Some(client) = Self::connect_existing_at(directory)? {
            return Ok(client);
        }
        start_daemon(directory, retention, Some(persistence))?;
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
        start_daemon(directory, retention, None)?;
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

    /// The process ID published by the daemon endpoint. It is part of a
    /// session's stable catalog identifier and lets administrative commands
    /// reject an identifier belonging to a different catalog in the same
    /// session directory.
    pub fn process_id(&self) -> u32 {
        self.endpoint.process_id
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
            endpoint,
            directory: directory.to_owned(),
        }))
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
        let mut connection = self.open_as(
            Request::Attach {
                session_id,
                pane_id: Some(pane_id),
                secret,
            },
            client_process_id,
        )?;
        let (response, mut descriptors) = connection.receive::<Response>()?;
        match response {
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
        }
    }

    fn open(&self, request: Request) -> Result<Connection> {
        self.open_as(request, std::process::id())
    }

    fn open_as(&self, request: Request, client_process_id: u32) -> Result<Connection> {
        let stream =
            Stream::connect(&self.endpoint.socket_path).context("connecting to the multiplexer")?;
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request read timeout")?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .context("setting the multiplexer request write timeout")?;
        let mut connection = Connection::new(stream);
        connection.send(&Envelope {
            version: PROTOCOL_VERSION,
            token: self.endpoint.token.clone(),
            // Named so a platform without descriptor passing can duplicate a
            // terminal's handles into this process instead.
            client_process_id,
            request,
        })?;
        Ok(connection)
    }

    /// Tells the multiplexer that an attached pane was resized.
    ///
    /// Only meaningful where the console belongs to the multiplexer; on Unix
    /// the resize has already taken effect through the descriptor.
    pub fn resize(&self, session_id: u64, pane_id: u64, columns: u16, lines: u16) -> Result<()> {
        let mut connection = self.open(Request::Resize {
            session_id,
            pane_id,
            columns,
            lines,
        })?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to resize: {other:?}"),
        }
    }

    /// Answers a revoke: the screen this process was showing a pane, handed
    /// back so the multiplexer can resume reading and relay the pane to every
    /// client that attaches.
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
        let mut connection = self.open(Request::Snapshot {
            session_id,
            pane_id,
            length: bytes.len(),
            columns,
            lines,
        })?;
        connection.write_all(&bytes)?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to snapshot: {other:?}"),
        }
    }

    /// Starts a process under the multiplexer and takes its terminal.
    pub fn spawn(&self, request: SpawnRequest) -> Result<AttachedPane> {
        let mut connection = self.open(Request::Spawn(request))?;
        let (response, mut descriptors) = connection.receive::<Response>()?;
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
        let mut connection = self.open(Request::Attach {
            session_id,
            pane_id,
            secret,
        })?;
        let (response, mut descriptors) = connection.receive::<Response>()?;
        match response {
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
        }
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
        verifier: Option<String>,
        snapshots: Vec<(u64, Vec<u8>)>,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier,
            snapshots: snapshots
                .iter()
                .map(|(pane_id, bytes)| PaneSnapshot {
                    pane_id: *pane_id,
                    length: bytes.len(),
                })
                .collect(),
        };
        let mut connection = self.open(Request::Detach(request))?;
        for (_, bytes) in &snapshots {
            connection.write_all(bytes)?;
        }
        match connection.receive::<Response>()?.0 {
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
        verifier: Option<String>,
        client_process_id: u32,
    ) -> Result<()> {
        let request = DetachRequest {
            session_id,
            summary,
            state,
            verifier,
            snapshots: Vec::new(),
        };
        let mut connection = self.open_as(Request::Detach(request), client_process_id)?;
        match connection.receive::<Response>()?.0 {
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
        verifier: Option<String>,
        offered: bool,
    ) -> Result<()> {
        let mut connection = self.open(Request::Share(crate::messages::ShareRequest {
            session_id,
            summary,
            state,
            verifier,
            offered,
        }))?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to share: {other:?}"),
        }
    }

    pub fn list(&self) -> Result<Vec<BackgroundSessionSummary>> {
        let mut connection = self.open(Request::List)?;
        match connection.receive::<Response>()?.0 {
            Response::Sessions { sessions, .. } => Ok(sessions),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to list: {other:?}"),
        }
    }

    pub fn list_with_restorable(
        &self,
    ) -> Result<(Vec<BackgroundSessionSummary>, Vec<RestorableSessionRecord>)> {
        let mut connection = self.open(Request::List)?;
        match connection.receive::<Response>()?.0 {
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
        let secret = persisted
            .verifier
            .as_ref()
            .map(|_| secret_prompt::prompt_for_reconnect_secret())
            .transpose()?;
        self.resume_loaded(persisted, secret.as_ref())
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
        let identities = crate::persistence::IdentitySet::from_paths(identity_paths)?;
        let persisted = crate::persistence::load_session_from_directory(
            &self.directory,
            record_id,
            &identities,
        )?;
        self.resume_loaded(persisted, secret)
    }

    #[cfg(feature = "session-persistence")]
    fn resume_loaded(
        &self,
        persisted: crate::persistence::PersistedSession,
        secret: Option<&SessionSecret>,
    ) -> Result<crate::persistence::PersistedSession> {
        let request = Request::Resume(ResumeRequest {
            record_id: persisted.id,
            summary: persisted.summary.clone(),
            state: persisted.state.clone(),
            verifier: persisted.verifier.clone(),
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
                    bytes: snapshot.bytes.clone(),
                })
                .collect(),
        });
        let mut connection = self.open(request)?;
        match connection.receive::<Response>()?.0 {
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
        let mut connection = self.open(Request::Kill { session_id })?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to kill: {other:?}"),
        }
    }

    /// Removes a session from the catalog without killing it.
    /// Scopes a backgrounded session to one process, or shares it with all.
    ///
    /// `verifier` is the secret a joining process will have to present, and is
    /// required when sharing a session that has none.
    pub fn set_session_scope(
        &self,
        session_id: u64,
        shared: bool,
        verifier: Option<String>,
    ) -> Result<()> {
        let mut connection = self.open(Request::SetSessionScope {
            session_id,
            shared,
            verifier,
        })?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to a session scope change: {other:?}"),
        }
    }

    pub fn forget(&self, session_id: u64) -> Result<()> {
        let mut connection = self.open(Request::Forget { session_id })?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to forget: {other:?}"),
        }
    }

    /// Asks the daemon to replace itself, keeping its sessions.
    pub fn upgrade(&self) -> Result<()> {
        let mut connection = self.open(Request::Upgrade)?;
        let response = connection.receive::<Response>()?.0;
        match response {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to upgrade: {other:?}"),
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut connection = self.open(Request::Shutdown)?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            other => anyhow::bail!("unexpected response to shutdown: {other:?}"),
        }
    }

    /// Asks what the multiplexer currently knows about these panes.
    pub fn pane_states(&self, pane_ids: Vec<u64>) -> Result<Vec<PaneStateReport>> {
        let mut connection = self.open(Request::PaneStates { pane_ids })?;
        match connection.receive::<Response>()?.0 {
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
        match connection.receive::<Response>()?.0 {
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
        let connection = self.open(Request::Subscribe)?;
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
        let directory = self.directory.clone();
        thread::spawn(move || {
            subscription_loop(directory, connection, dispatch);
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
        let (response, mut descriptors) = connection.receive::<Response>()?;
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
fn subscription_loop(directory: PathBuf, first: Connection, subscription: Subscription) {
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
        match resubscribe(&directory, &reporters, &revokes, &grants) {
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
    directory: &std::path::Path,
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
        if let Ok(Some(client)) = Client::connect_existing_at(directory)
            && let Ok(connection) = client.open(Request::Subscribe)
        {
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
pub struct ExitReporters {
    reporters: Mutex<HashMap<u64, alacritty_terminal::tty::AttachedChildEvents>>,
    shared: Mutex<HashMap<u64, async_channel::Sender<PaneExitReport>>>,
    /// Exits reported for a pane that had no reporter yet.
    ///
    /// The multiplexer starts the process the moment it is asked, so a pane can
    /// end before the terminal showing it has been built — a bad shell, a
    /// failing `exec`, an instant command, a fast Ctrl-D. Dropping the report
    /// then left the terminal waiting for an event that had already happened,
    /// with nothing able to produce it a second time.
    pending: Mutex<HashMap<u64, PaneExitReport>>,
}

impl ExitReporters {
    pub fn register(&self, pane_id: u64, reporter: alacritty_terminal::tty::AttachedChildEvents) {
        self.reporters.lock().unwrap().insert(pane_id, reporter);
        self.deliver_pending(pane_id);
    }

    pub fn forget(&self, pane_id: u64) {
        self.reporters.lock().unwrap().remove(&pane_id);
        self.pending.lock().unwrap().remove(&pane_id);
    }

    /// The panes still waiting to be told their process ended.
    ///
    /// Both registries, because either kind of holder needs catching up after a
    /// subscription is re-established.
    pub fn registered(&self) -> Vec<u64> {
        let mut ids = self
            .reporters
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        ids.extend(self.shared.lock().unwrap().keys().copied());
        ids.sort_unstable();
        ids.dedup();
        ids
    }

    /// Hands a pane whatever was reported for it before it had a reporter.
    fn deliver_pending(&self, pane_id: u64) {
        let Some(report) = self.pending.lock().unwrap().remove(&pane_id) else {
            return;
        };
        if report.disconnected {
            self.disconnect_one(pane_id);
        } else {
            self.report(pane_id, report.raw_status, report.input_sent);
        }
    }

    /// Routes a shared pane's exit to a channel the pane's holder drains.
    ///
    /// The channel is asynchronous so the holder can await it from its main
    /// task; the sender is used from the subscription thread, where
    /// [`async_channel::Sender::try_send`] cannot fail on an unbounded
    /// channel unless the receiver is gone.
    pub fn register_shared(&self, pane_id: u64, reporter: async_channel::Sender<PaneExitReport>) {
        self.shared.lock().unwrap().insert(pane_id, reporter);
        self.deliver_pending(pane_id);
    }

    pub fn forget_shared(&self, pane_id: u64) {
        self.shared.lock().unwrap().remove(&pane_id);
        self.pending.lock().unwrap().remove(&pane_id);
    }

    fn report(&self, pane_id: u64, raw_status: Option<i32>, input_sent: bool) {
        let mut delivered = false;
        let mut reporters = self.reporters.lock().unwrap();
        if let Some(mut reporter) = reporters.remove(&pane_id) {
            delivered = true;
            let _ = match raw_status {
                Some(status) => reporter.report_exit(status),
                None => reporter.report_status_unavailable(),
            };
        }
        drop(reporters);
        let mut shared = self.shared.lock().unwrap();
        if let Some(reporter) = shared.remove(&pane_id) {
            delivered = true;
            let _ = reporter.try_send(PaneExitReport {
                raw_status,
                input_sent,
                disconnected: false,
            });
        }
        drop(shared);
        if !delivered {
            self.pending.lock().unwrap().insert(
                pane_id,
                PaneExitReport {
                    raw_status,
                    input_sent,
                    disconnected: false,
                },
            );
        }
    }

    /// Tells one pane that no report is coming, without touching the others.
    fn disconnect_one(&self, pane_id: u64) {
        let mut reporters = self.reporters.lock().unwrap();
        if let Some(mut reporter) = reporters.remove(&pane_id) {
            let _ = reporter.report_watcher_disconnected();
        }
        drop(reporters);
        let mut shared = self.shared.lock().unwrap();
        if let Some(reporter) = shared.remove(&pane_id) {
            let _ = reporter.try_send(PaneExitReport {
                raw_status: None,
                input_sent: false,
                disconnected: true,
            });
        }
    }

    fn report_all_disconnected(&self) {
        let mut reporters = self.reporters.lock().unwrap();
        for (_, mut reporter) in reporters.drain() {
            let _ = reporter.report_watcher_disconnected();
        }
        drop(reporters);
        let mut shared = self.shared.lock().unwrap();
        for (_, reporter) in shared.drain() {
            let _ = reporter.try_send(PaneExitReport {
                raw_status: None,
                input_sent: false,
                disconnected: true,
            });
        }
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
fn start_daemon(
    directory: &std::path::Path,
    retention: Retention,
    #[cfg(feature = "session-persistence")] persistence: Option<PersistenceOptions>,
) -> Result<()> {
    let (executable, mut arguments) = multiplexer_command()?;
    arguments.extend(["--retention".to_owned(), retention.name().to_owned()]);
    if let Retention::Memory { bytes } = retention {
        arguments.extend(["--retention-bytes".to_owned(), bytes.to_string()]);
    }
    #[cfg(feature = "session-persistence")]
    if matches!(retention, Retention::Disk)
        && let Some(persistence) = persistence
    {
        let recipients = crate::persistence::resolve_recipient_strings(&persistence.recipients)?;
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

/// How to start the multiplexer: the `zmux` binary beside this executable, or
/// this executable's own `mux` subcommand when there is none.
///
/// Resolved from this process's own location rather than `PATH`, so an
/// unrelated `zmux` earlier in the path cannot be handed a session's terminals.
fn multiplexer_command() -> Result<(PathBuf, Vec<String>)> {
    let current = std::env::current_exe().context("locating this executable")?;
    if current.file_name().and_then(|name| name.to_str()) == Some("zmux") {
        return Ok((current, vec!["--daemon".to_owned()]));
    }
    let beside = current.with_file_name("zmux");
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
