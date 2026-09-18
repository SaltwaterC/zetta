//! Best-effort SSH-agent forwarding for a Zosh session.
//!
//! The Mosh loop never performs local agent I/O. Each remote agent connection
//! gets one bounded worker and one persistent local agent connection, which
//! preserves request ordering while keeping socket stalls away from UDP and
//! terminal rendering.

use std::{
    collections::HashMap,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::fs;
#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(any(unix, windows))]
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, net::UnixListener};

use mosh_rs::HostEvent;

pub(crate) const AGENT_PROTOCOL_VERSION: u32 = 1;
pub(crate) const AGENT_MAX_FRAME: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 16;
const MAX_OUTSTANDING: usize = 64;
const WORK_QUEUE_DEPTH: usize = 16;
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const BOOTSTRAP_RELAY_POLL: Duration = Duration::from_millis(5);
const SESSION_BIND_EXTENSION: &[u8] = b"session-bind@openssh.com";
const SSH_AGENT_EXTENSION: u8 = 27;

#[derive(Debug)]
pub(crate) enum AgentClientCommand {
    Response {
        connection_id: u64,
        request_id: u64,
        frame: Vec<u8>,
        closed: bool,
    },
}

struct Work {
    request_id: u64,
    frame: Vec<u8>,
}

enum WorkerResult {
    Response {
        connection_id: u64,
        request_id: u64,
        frame: Vec<u8>,
    },
    Closed {
        connection_id: u64,
        request_id: u64,
        error: String,
    },
}

type AgentStream = Box<dyn ReadWrite + Send>;

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

struct Connection {
    work: SyncSender<Work>,
    outstanding: usize,
}

/// The client-side forwarding bridge. It is deliberately independent of a
/// particular Mosh session so the standalone and pane loops use identical
/// limits and failure behavior.
pub(crate) struct AgentBridge {
    path: Option<PathBuf>,
    binding: Option<Vec<u8>>,
    results: Receiver<WorkerResult>,
    result_tx: SyncSender<WorkerResult>,
    connections: HashMap<u64, Connection>,
    outstanding: usize,
    negotiated: bool,
    waiting_since: Option<Instant>,
    warned: bool,
}

impl AgentBridge {
    pub(crate) fn with_agent(
        requested: bool,
        binding: Option<Vec<u8>>,
        agent_path: Option<PathBuf>,
    ) -> Self {
        let (result_tx, results) = mpsc::sync_channel(MAX_OUTSTANDING);
        let path = requested
            .then(|| agent_path.or_else(|| std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from)));
        let path = path
            .flatten()
            .filter(|path| !path.as_os_str().is_empty())
            .filter(|path| local_agent_path_exists(path));
        let mut bridge = Self {
            path,
            binding: binding.filter(|frame| is_forwarding_session_bind_frame(frame)),
            results,
            result_tx,
            connections: HashMap::new(),
            outstanding: 0,
            negotiated: false,
            waiting_since: requested.then(Instant::now),
            warned: false,
        };
        if requested && bridge.path.is_none() {
            bridge.warn(
                "SSH_AUTH_SOCK is not set or does not name a local agent socket; agent forwarding is disabled",
            );
            bridge.waiting_since = None;
        }
        bridge
    }

    pub(crate) fn enabled(&self) -> bool {
        self.path.is_some()
    }

    pub(crate) fn negotiation_timed_out(&mut self) {
        if self.negotiated || self.waiting_since.is_none() {
            return;
        }
        if self
            .waiting_since
            .is_some_and(|started| started.elapsed() >= NEGOTIATION_TIMEOUT)
        {
            self.warn(
                "the remote peer did not negotiate agent forwarding; terminal continues without it",
            );
            self.waiting_since = None;
        }
    }

    pub(crate) fn handle_events(&mut self, events: &[HostEvent]) -> Vec<AgentClientCommand> {
        let mut commands = self.drain_results();
        for event in events {
            match event {
                HostEvent::AgentReady { supported, error } => {
                    self.waiting_since = None;
                    if *supported && error.is_none() {
                        self.negotiated = true;
                    } else if let Some(error) = error {
                        self.warn(&format!("remote agent forwarding unavailable: {error}"));
                    } else {
                        self.warn(
                            "remote agent forwarding unavailable; terminal continues normally",
                        );
                    }
                }
                HostEvent::AgentRequest {
                    id,
                    connection_id,
                    frame,
                } => {
                    if !self.negotiated {
                        continue;
                    }
                    if !valid_frame(frame) {
                        commands.push(AgentClientCommand::Response {
                            connection_id: *connection_id,
                            request_id: *id,
                            frame: Vec::new(),
                            closed: true,
                        });
                        self.warn("remote agent request exceeded the frame limit or was malformed");
                        continue;
                    }
                    if self.outstanding >= MAX_OUTSTANDING {
                        commands.push(AgentClientCommand::Response {
                            connection_id: *connection_id,
                            request_id: *id,
                            frame: Vec::new(),
                            closed: true,
                        });
                        self.warn("SSH-agent forwarding is busy; the remote connection was closed");
                        continue;
                    }
                    let sender = if let Some(connection) = self.connections.get(connection_id) {
                        connection.work.clone()
                    } else {
                        if self.connections.len() >= MAX_CONNECTIONS {
                            commands.push(AgentClientCommand::Response {
                                connection_id: *connection_id,
                                request_id: *id,
                                frame: Vec::new(),
                                closed: true,
                            });
                            self.warn("too many forwarded SSH-agent connections");
                            continue;
                        }
                        let (work_tx, work_rx) = mpsc::sync_channel(WORK_QUEUE_DEPTH);
                        let path = self.path.clone().expect("enabled bridge has agent path");
                        spawn_worker(
                            *connection_id,
                            path,
                            self.binding.clone(),
                            work_rx,
                            self.result_tx.clone(),
                        );
                        self.connections.insert(
                            *connection_id,
                            Connection {
                                work: work_tx.clone(),
                                outstanding: 0,
                            },
                        );
                        work_tx
                    };
                    match sender.try_send(Work {
                        request_id: *id,
                        frame: frame.clone(),
                    }) {
                        Ok(()) => {
                            self.outstanding += 1;
                            if let Some(connection) = self.connections.get_mut(connection_id) {
                                connection.outstanding += 1;
                            }
                        }
                        Err(TrySendError::Full(_)) => {
                            self.abort_connection(*connection_id);
                            commands.push(AgentClientCommand::Response {
                                connection_id: *connection_id,
                                request_id: *id,
                                frame: Vec::new(),
                                closed: true,
                            });
                            self.warn("one forwarded SSH-agent connection is busy");
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            self.abort_connection(*connection_id);
                            commands.push(AgentClientCommand::Response {
                                connection_id: *connection_id,
                                request_id: *id,
                                frame: Vec::new(),
                                closed: true,
                            });
                        }
                    }
                }
                HostEvent::AgentClose { connection_id, .. } => {
                    self.abort_connection(*connection_id);
                }
                _ => {}
            }
        }
        commands.extend(self.drain_results());
        commands
    }

    fn drain_results(&mut self) -> Vec<AgentClientCommand> {
        let mut commands = Vec::new();
        loop {
            match self.results.try_recv() {
                Ok(WorkerResult::Response {
                    connection_id,
                    request_id,
                    frame,
                }) => {
                    self.outstanding = self.outstanding.saturating_sub(1);
                    if let Some(connection) = self.connections.get_mut(&connection_id) {
                        connection.outstanding = connection.outstanding.saturating_sub(1);
                    }
                    commands.push(AgentClientCommand::Response {
                        connection_id,
                        request_id,
                        frame,
                        closed: false,
                    });
                }
                Ok(WorkerResult::Closed {
                    connection_id,
                    request_id,
                    error,
                }) => {
                    self.outstanding = self.outstanding.saturating_sub(1);
                    let remaining = self
                        .connections
                        .remove(&connection_id)
                        .map_or(0, |connection| connection.outstanding.saturating_sub(1));
                    self.outstanding = self.outstanding.saturating_sub(remaining);
                    self.warn(&format!("local SSH agent failed: {error}"));
                    commands.push(AgentClientCommand::Response {
                        connection_id,
                        request_id,
                        frame: Vec::new(),
                        closed: true,
                    });
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        commands
    }

    fn abort_connection(&mut self, connection_id: u64) {
        if let Some(connection) = self.connections.remove(&connection_id) {
            self.outstanding = self.outstanding.saturating_sub(connection.outstanding);
        }
    }

    fn warn(&mut self, message: &str) {
        if !self.warned {
            eprintln!("zosh: {message}");
            self.warned = true;
        }
    }
}

fn spawn_worker(
    connection_id: u64,
    path: PathBuf,
    binding: Option<Vec<u8>>,
    work: Receiver<Work>,
    results: SyncSender<WorkerResult>,
) {
    thread::Builder::new()
        .name(format!("zosh-agent-{connection_id}"))
        .spawn(move || {
            let mut stream: Option<AgentStream> = None;
            while let Ok(work) = work.recv() {
                let stream_ref = match stream.as_mut() {
                    Some(stream) => stream,
                    None => match connect_agent_with_binding(&path, binding.as_deref()) {
                        Ok(new_stream) => stream.insert(new_stream),
                        Err(error) => {
                            let _ = results.send(WorkerResult::Closed {
                                connection_id,
                                request_id: work.request_id,
                                error: error.to_string(),
                            });
                            break;
                        }
                    },
                };
                if let Err(error) = stream_ref
                    .write_all(&work.frame)
                    .and_then(|_| stream_ref.flush())
                {
                    let _ = results.send(WorkerResult::Closed {
                        connection_id,
                        request_id: work.request_id,
                        error: error.to_string(),
                    });
                    break;
                }
                match read_frame_from_buffer(stream_ref) {
                    Ok(frame) => {
                        if results
                            .send(WorkerResult::Response {
                                connection_id,
                                request_id: work.request_id,
                                frame,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = results.send(WorkerResult::Closed {
                            connection_id,
                            request_id: work.request_id,
                            error: error.to_string(),
                        });
                        break;
                    }
                }
            }
        })
        .ok();
}

fn connect_agent_with_binding(path: &Path, binding: Option<&[u8]>) -> io::Result<AgentStream> {
    let mut stream = connect_agent(path)?;
    let Some(binding) = binding else {
        return Ok(stream);
    };
    if replay_binding(&mut stream, binding).is_err() {
        // A server that does not understand the extension, or an agent that
        // closes after rejecting it, still gets the old raw-frame behavior.
        stream = connect_agent(path)?;
    }
    Ok(stream)
}

fn replay_binding(stream: &mut AgentStream, binding: &[u8]) -> io::Result<()> {
    stream.write_all(binding)?;
    stream.flush()?;
    // The response is deliberately ignored. OpenSSH agents answer a rejected
    // extension with a normal failure frame, after which ordinary requests on
    // the same connection remain valid.
    let _ = read_frame_from_buffer(stream)?;
    Ok(())
}

fn read_frame_from_buffer(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let body_len = u32::from_be_bytes(length) as usize;
    if body_len == 0 || body_len > AGENT_MAX_FRAME - 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH-agent response exceeds the frame limit",
        ));
    }
    let mut frame = Vec::with_capacity(body_len + 4);
    frame.extend_from_slice(&length);
    frame.resize(body_len + 4, 0);
    stream.read_exact(&mut frame[4..])?;
    Ok(frame)
}

fn valid_frame(frame: &[u8]) -> bool {
    if frame.len() < 4 || frame.len() > AGENT_MAX_FRAME {
        return false;
    }
    u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize == frame.len() - 4
}

pub(crate) fn is_forwarding_session_bind_frame(frame: &[u8]) -> bool {
    if !valid_frame(frame) || frame.get(4) != Some(&SSH_AGENT_EXTENSION) {
        return false;
    }
    let body = &frame[4..];
    let mut offset = 1;
    let Some(extension) = read_agent_string(body, &mut offset) else {
        return false;
    };
    if extension != SESSION_BIND_EXTENSION {
        return false;
    }
    for _ in 0..3 {
        if read_agent_string(body, &mut offset).is_none() {
            return false;
        }
    }
    body.get(offset) == Some(&1) && offset + 1 == body.len()
}

fn read_agent_string<'a>(body: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    let length =
        u32::from_be_bytes(body.get(*offset..offset.checked_add(4)?)?.try_into().ok()?) as usize;
    *offset = offset.checked_add(4)?;
    let end = offset.checked_add(length)?;
    let value = body.get(*offset..end)?;
    *offset = end;
    Some(value)
}

fn local_agent_path_exists(path: &Path) -> bool {
    #[cfg(unix)]
    {
        path.exists()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn connect_agent(path: &Path) -> io::Result<AgentStream> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;
        Ok(Box::new(UnixStream::connect(path)?))
    }
    #[cfg(windows)]
    {
        return Ok(Box::new(
            OpenOptions::new().read(true).write(true).open(path)?,
        ));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "SSH-agent forwarding is unsupported on this platform",
        ))
    }
}

/// A short-lived local proxy used only while native SSH bootstraps a Zosh
/// endpoint. OpenSSH sends the forwarding session binding to the local agent
/// connection; recording that complete frame lets later Zosh workers replay
/// the same authenticated context on fresh connections.
pub(crate) struct BootstrapAgentRelay {
    #[cfg(any(unix, windows))]
    path: PathBuf,
    #[cfg(any(unix, windows))]
    agent_path: PathBuf,
    #[cfg(any(unix, windows))]
    binding: Arc<Mutex<Option<Vec<u8>>>>,
    #[cfg(any(unix, windows))]
    stop: Arc<AtomicBool>,
    #[cfg(any(unix, windows))]
    thread: Option<JoinHandle<()>>,
}

impl BootstrapAgentRelay {
    #[cfg(any(unix, windows))]
    pub(crate) fn for_agent_path(path: PathBuf) -> io::Result<Option<Self>> {
        if path.as_os_str().is_empty() || !local_agent_path_exists(&path) {
            return Ok(None);
        }
        Self::from_agent_path(path).map(Some)
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn for_agent_path(_path: PathBuf) -> io::Result<Option<Self>> {
        Ok(None)
    }

    #[cfg(unix)]
    fn from_agent_path(agent_path: PathBuf) -> io::Result<Self> {
        let (directory, socket_path) = create_bootstrap_relay_paths()?;
        let listener = match UnixListener::bind(&socket_path) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(error);
            }
        };
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let binding = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("zosh-agent-bootstrap-relay".to_owned())
            .spawn({
                let binding = Arc::clone(&binding);
                let agent_path = agent_path.clone();
                let stop = Arc::clone(&stop);
                move || relay_accept_loop(listener, agent_path, binding, stop)
            })?;
        Ok(Self {
            path: socket_path,
            agent_path,
            binding,
            stop,
            thread: Some(thread),
        })
    }

    #[cfg(windows)]
    fn from_agent_path(agent_path: PathBuf) -> io::Result<Self> {
        let path = create_bootstrap_pipe_path();
        let binding = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("zosh-agent-bootstrap-relay".to_owned())
            .spawn({
                let binding = Arc::clone(&binding);
                let agent_path = agent_path.clone();
                let stop = Arc::clone(&stop);
                let path = path.clone();
                move || relay_pipe_accept_loop(path, agent_path, binding, stop)
            })?;
        Ok(Self {
            path,
            agent_path,
            binding,
            stop,
            thread: Some(thread),
        })
    }

    #[cfg(test)]
    #[cfg(any(unix, windows))]
    fn for_test_agent_path(agent_path: &Path) -> io::Result<Self> {
        Self::from_agent_path(agent_path.to_owned())
    }

    #[cfg(any(unix, windows))]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(any(unix, windows))]
    pub(crate) fn agent_path(&self) -> &Path {
        &self.agent_path
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn agent_path(&self) -> &Path {
        Path::new("")
    }

    #[cfg(not(any(unix, windows)))]
    pub(crate) fn path(&self) -> &Path {
        Path::new("")
    }

    pub(crate) fn binding(&self) -> Option<Vec<u8>> {
        #[cfg(any(unix, windows))]
        {
            return self
                .binding
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
        }
        #[cfg(not(any(unix, windows)))]
        {
            None
        }
    }
}

impl Drop for BootstrapAgentRelay {
    fn drop(&mut self) {
        #[cfg(any(unix, windows))]
        {
            self.stop.store(true, Ordering::Release);
            #[cfg(windows)]
            let _wake = OpenOptions::new().read(true).write(true).open(&self.path);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            #[cfg(unix)]
            let _ = fs::remove_file(&self.path);
            #[cfg(unix)]
            if let Some(directory) = self.path.parent() {
                let _ = fs::remove_dir(directory);
            }
        }
    }
}

#[cfg(unix)]
fn create_bootstrap_relay_paths() -> io::Result<(PathBuf, PathBuf)> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let roots = [PathBuf::from("/tmp"), std::env::temp_dir()];
    let mut last_error = None;
    for root in roots {
        for attempt in 0..32_u32 {
            let directory = root.join(format!(
                "zosh-agent-bootstrap-{}-{:x}-{}",
                std::process::id(),
                stamp,
                attempt
            ));
            match fs::create_dir(&directory) {
                Ok(()) => return Ok((directory.clone(), directory.join("agent.sock"))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    last_error = Some(error);
                    break;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique Zosh bootstrap agent directory",
        )
    }))
}

#[cfg(unix)]
fn relay_accept_loop(
    listener: UnixListener,
    agent_path: PathBuf,
    binding: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let agent_path = agent_path.clone();
                let binding = Arc::clone(&binding);
                thread::spawn(move || relay_connection(stream, &agent_path, &binding));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::park_timeout(BOOTSTRAP_RELAY_POLL);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
}

#[cfg(windows)]
fn create_bootstrap_pipe_path() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    PathBuf::from(format!(
        r"\\.\pipe\zosh-agent-bootstrap-{}-{stamp:x}",
        std::process::id()
    ))
}

#[cfg(windows)]
fn relay_pipe_accept_loop(
    path: PathBuf,
    agent_path: PathBuf,
    binding: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
) {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    while !stop.load(Ordering::Acquire) {
        let handle = unsafe {
            CreateNamedPipeW(
                windows::core::PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                AGENT_MAX_FRAME as u32,
                AGENT_MAX_FRAME as u32,
                0,
                None,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            break;
        }
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { CloseHandle(handle) };
            break;
        }
        let connected = unsafe { ConnectNamedPipe(handle, None) };
        if connected.is_err() {
            let error = connected.unwrap_err();
            if error.code().0 as u32 & 0xffff != ERROR_PIPE_CONNECTED.0 {
                let _ = unsafe { CloseHandle(handle) };
                continue;
            }
        }
        let stream = unsafe { std::fs::File::from_raw_handle(handle.0 as _) };
        if stop.load(Ordering::Acquire) {
            drop(stream);
            break;
        }
        let agent_path = agent_path.clone();
        let binding = Arc::clone(&binding);
        thread::spawn(move || relay_connection(Box::new(stream), &agent_path, &binding));
    }
}

#[cfg(unix)]
fn relay_connection(
    forwarded: std::os::unix::net::UnixStream,
    agent_path: &Path,
    binding: &Mutex<Option<Vec<u8>>>,
) {
    relay_connection_stream(Box::new(forwarded), agent_path, binding);
}

#[cfg(windows)]
fn relay_connection(forwarded: AgentStream, agent_path: &Path, binding: &Mutex<Option<Vec<u8>>>) {
    relay_connection_stream(forwarded, agent_path, binding);
}

fn relay_connection_stream(
    mut forwarded: AgentStream,
    agent_path: &Path,
    binding: &Mutex<Option<Vec<u8>>>,
) {
    let Ok(mut agent) = connect_agent(agent_path) else {
        return;
    };
    loop {
        let Ok(frame) = read_frame_from_buffer(&mut forwarded) else {
            return;
        };
        if is_forwarding_session_bind_frame(&frame) {
            let mut captured = binding
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if captured.is_none() {
                *captured = Some(frame.clone());
            }
        }
        if agent.write_all(&frame).and_then(|_| agent.flush()).is_err() {
            return;
        }
        let Ok(response) = read_frame_from_buffer(&mut agent) else {
            return;
        };
        if forwarded
            .write_all(&response)
            .and_then(|_| forwarded.flush())
            .is_err()
        {
            return;
        }
    }
}

#[cfg(test)]
#[path = "tests/agent.rs"]
mod tests;
