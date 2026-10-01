//! The server half of Zosh's opt-in SSH-agent forwarding extension.
//!
//! A private Unix-domain socket is created only after the authenticated client
//! has sent the agent hello. A listener thread accepts on it and every local
//! connection is handled by one small reader/writer thread; the session loop
//! only moves complete, bounded frames between those threads and cumulative
//! Mosh host records. Each thread wakes the loop when it has something for it,
//! so the loop never polls the socket.

use crate::protocol::AgentHostRecord;
use crate::wake::{Waker, WakingSender};

#[cfg(windows)]
mod windows_pipe;
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
    time::Duration,
};

#[cfg(unix)]
use std::fs;
#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(any(unix, windows))]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub const MAX_FRAME: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 16;
const MAX_OUTSTANDING: usize = 64;
const EVENT_QUEUE_DEPTH: usize = 64;
const REQUEST_IDENTITIES: u8 = 11;
const PRIME_TIMEOUT: Duration = Duration::from_secs(1);
/// How long the listener backs off after a failed accept, such as running out
/// of descriptors, so a persistent failure cannot spin its thread.
#[cfg(unix)]
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

#[cfg(unix)]
use std::os::unix::{
    fs::PermissionsExt,
    net::{UnixListener, UnixStream},
};

/// Give a native SSH agent-forwarding channel one bounded request before the
/// bootstrap announces its Mosh endpoint. OpenSSH sends its forwarding
/// session binding on that channel before the identities response, which is
/// the context agents such as 1Password require for constrained keys.
pub fn prime_forwarded_agent(verbose: bool) {
    let Some(path) = std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from) else {
        return;
    };
    match prime_agent_path(&path) {
        Ok(()) if verbose => eprintln!("zosh-server-rs: native agent forwarding primed"),
        Ok(()) | Err(_) => {}
    }
}

fn prime_agent_path(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(PRIME_TIMEOUT))?;
        stream.set_write_timeout(Some(PRIME_TIMEOUT))?;
        prime_agent_stream(&mut stream)
    }
    #[cfg(windows)]
    {
        let path = path.to_owned();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .and_then(|mut stream| prime_agent_stream(&mut stream));
            let _ = sender.send(result);
        });
        receiver.recv_timeout(PRIME_TIMEOUT).unwrap_or_else(|_| {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out priming the forwarded SSH agent",
            ))
        })
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

fn prime_agent_stream(stream: &mut (impl Read + Write)) -> io::Result<()> {
    let request = [0, 0, 0, 1, REQUEST_IDENTITIES];
    stream.write_all(&request)?;
    stream.flush()?;
    let _ = read_agent_frame(stream)?;
    Ok(())
}

fn read_agent_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let body_len = u32::from_be_bytes(length) as usize;
    if body_len == 0 || body_len > MAX_FRAME - 4 {
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

enum LocalEvent {
    Request {
        connection_id: u64,
        frame: Vec<u8>,
    },
    Closed {
        connection_id: u64,
        error: Option<String>,
    },
    #[cfg(any(unix, windows))]
    Connected {
        stream: AgentStream,
    },
}

struct Connection {
    replies: SyncSender<Reply>,
}

struct Reply {
    frame: Vec<u8>,
    closed: bool,
}

type AgentStream = Box<dyn ReadWrite + Send>;

/// The accept thread on the private Unix socket. Dropping it stops the thread
/// by setting `stop` and then connecting once, which is what an `accept`
/// blocked on the socket is waiting for.
#[cfg(unix)]
struct UnixAgentListener {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl Drop for UnixAgentListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = UnixStream::connect(&self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(windows)]
struct WindowsPipeListener {
    path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

#[cfg(windows)]
impl Drop for WindowsPipeListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = OpenOptions::new().read(true).write(true).open(&self.path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// State owned by one negotiated remote agent socket.
pub struct AgentServer {
    #[cfg(unix)]
    listener: Option<UnixAgentListener>,
    #[cfg(windows)]
    listener: Option<WindowsPipeListener>,
    socket_path: Option<PathBuf>,
    events: Receiver<LocalEvent>,
    event_tx: WakingSender<LocalEvent>,
    connections: HashMap<u64, Connection>,
    outstanding: HashMap<(u64, u64), SyncSender<Reply>>,
    pending: Vec<(u64, AgentHostRecord)>,
    states: BTreeMap<u64, u64>,
    next_connection_id: u64,
    next_record_id: u64,
}

impl AgentServer {
    /// Opens the private agent socket. Every connection event wakes the
    /// session loop, which calls [`poll`](Self::poll), through `waker`.
    pub fn new(waker: Waker) -> Self {
        let (event_tx, events) = mpsc::sync_channel(EVENT_QUEUE_DEPTH);
        let event_tx = WakingSender::new(event_tx, waker);
        #[cfg(unix)]
        let (listener, socket_path, error) = match create_listener(event_tx.clone()) {
            Ok(listener) => {
                let path = listener.path.clone();
                (Some(listener), Some(path), None)
            }
            Err(error) => (None, None, Some(error.to_string())),
        };
        #[cfg(windows)]
        let (listener, error) = match create_named_pipe_listener(event_tx.clone()) {
            Ok(listener) => (Some(listener), None),
            Err(error) => (None, Some(error.to_string())),
        };
        #[cfg(windows)]
        let socket_path = listener.as_ref().map(|listener| listener.path.clone());
        #[cfg(not(any(unix, windows)))]
        let (socket_path, error) = (
            None,
            Some("this build has no SSH-agent socket adapter".to_owned()),
        );

        let mut server = Self {
            #[cfg(unix)]
            listener,
            #[cfg(windows)]
            listener,
            socket_path,
            events,
            event_tx,
            connections: HashMap::new(),
            outstanding: HashMap::new(),
            pending: Vec::new(),
            states: BTreeMap::new(),
            next_connection_id: 1,
            next_record_id: 1,
        };
        let supported = server.socket_path.is_some();
        server.queue(AgentHostRecord::Ready { supported, error });
        server
    }

    pub fn socket_path(&self) -> Option<&Path> {
        self.socket_path.as_deref()
    }

    pub fn poll(&mut self) -> bool {
        let mut dirty = false;
        loop {
            match self.events.try_recv() {
                #[cfg(any(unix, windows))]
                Ok(LocalEvent::Connected { stream }) => {
                    if self.connections.len() >= MAX_CONNECTIONS {
                        continue;
                    }
                    let connection_id = self.next_connection_id;
                    self.next_connection_id = self.next_connection_id.saturating_add(1);
                    let (replies, reply_rx) = mpsc::sync_channel(1);
                    self.connections
                        .insert(connection_id, Connection { replies });
                    spawn_connection(connection_id, stream, reply_rx, self.event_tx.clone());
                }
                Ok(LocalEvent::Request {
                    connection_id,
                    frame,
                }) => {
                    dirty |= self.queue_request(connection_id, frame);
                }
                Ok(LocalEvent::Closed {
                    connection_id,
                    error,
                }) => {
                    dirty |= self.close_connection(connection_id, error);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        dirty
    }

    pub fn apply_response(
        &mut self,
        connection_id: u64,
        request_id: u64,
        frame: &[u8],
        closed: bool,
    ) -> bool {
        let Some(replies) = self.outstanding.remove(&(connection_id, request_id)) else {
            return false;
        };
        if !closed && !valid_frame(frame) {
            let _ = replies.send(Reply {
                frame: Vec::new(),
                closed: true,
            });
            return true;
        }
        if replies
            .send(Reply {
                frame: frame.to_vec(),
                closed,
            })
            .is_err()
        {
            self.close_connection(
                connection_id,
                Some("local agent connection closed".to_owned()),
            );
        }
        true
    }

    /// The records a frame diffed from `base` has to carry: every pending one
    /// for the acknowledged state (`None`), otherwise those after the last
    /// one a state at or before `base` carried. Leaning on an earlier state
    /// than `base` only resends records, which the client deduplicates.
    pub fn records_after(&self, base: Option<u64>) -> Vec<AgentHostRecord> {
        let carried = base.and_then(|base| {
            self.states
                .range(..=base)
                .next_back()
                .map(|(_, record)| *record)
        });
        self.pending
            .iter()
            .filter(|(id, _)| carried.is_none_or(|carried| *id > carried))
            .map(|(_, record)| record.clone())
            .collect()
    }

    pub fn snapshot_for_state(&mut self, state: u64) {
        if let Some((last, _)) = self.pending.last() {
            self.states.insert(state, *last);
        }
    }

    pub fn acknowledge(&mut self, state: u64) {
        let Some(last_record) = self
            .states
            .range(..=state)
            .next_back()
            .map(|(_, record)| *record)
        else {
            return;
        };
        self.pending.retain(|(record, _)| *record > last_record);
        self.states.retain(|number, _| *number > state);
    }

    fn queue_request(&mut self, connection_id: u64, frame: Vec<u8>) -> bool {
        let Some(connection) = self.connections.get(&connection_id) else {
            return false;
        };
        if !valid_frame(&frame) || self.outstanding.len() >= MAX_OUTSTANDING {
            let _ = connection.replies.try_send(Reply {
                frame: Vec::new(),
                closed: true,
            });
            return true;
        }
        let request_id = self.next_record_id;
        self.next_record_id = self.next_record_id.saturating_add(1);
        self.outstanding
            .insert((connection_id, request_id), connection.replies.clone());
        self.pending.push((
            request_id,
            AgentHostRecord::Request {
                id: request_id,
                connection_id,
                frame,
            },
        ));
        true
    }

    fn close_connection(&mut self, connection_id: u64, error: Option<String>) -> bool {
        let had_connection = self.connections.remove(&connection_id).is_some();
        let mut had_outstanding = false;
        self.outstanding.retain(|(id, _), replies| {
            if *id == connection_id {
                had_outstanding = true;
                let _ = replies.try_send(Reply {
                    frame: Vec::new(),
                    closed: true,
                });
                false
            } else {
                true
            }
        });
        if !had_connection && !had_outstanding {
            return false;
        }
        let id = self.next_record_id;
        self.next_record_id = self.next_record_id.saturating_add(1);
        self.pending.push((
            id,
            AgentHostRecord::Close {
                id,
                connection_id,
                error,
            },
        ));
        true
    }

    fn queue(&mut self, record: AgentHostRecord) {
        let id = self.next_record_id;
        self.next_record_id = self.next_record_id.saturating_add(1);
        self.pending.push((id, record));
    }
}

impl Drop for AgentServer {
    fn drop(&mut self) {
        // The listener goes first: on Unix it stops by connecting to the
        // socket, which therefore has to still exist.
        #[cfg(any(unix, windows))]
        self.listener.take();
        #[cfg(unix)]
        if let Some(path) = self.socket_path.take() {
            let _ = fs::remove_file(&path);
            if let Some(parent) = path.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }
}

#[cfg(unix)]
fn create_listener(events: WakingSender<LocalEvent>) -> io::Result<UnixAgentListener> {
    let mut random = [0_u8; 8];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let name = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = std::env::temp_dir().join(format!("zosh-agent-{}-{name}", std::process::id()));
    fs::create_dir(&directory)?;
    let path = directory.join("agent.sock");
    let listener = UnixListener::bind(&path)?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread = thread::Builder::new()
        .name("zosh-agent-listener".to_owned())
        .spawn({
            let stop = Arc::clone(&stop);
            move || accept_connections(&listener, &stop, &events)
        })
        .map_err(io::Error::other)?;
    Ok(UnixAgentListener {
        path,
        stop,
        thread: Some(thread),
    })
}

/// The listener thread: hand every accepted connection to the session loop
/// until the listener is dropped. The listener blocks, so an accepted socket
/// does too — BSD-derived kernels, macOS among them, give it the listener's
/// `O_NONBLOCK`, and the connection thread reads with blocking `read_exact`.
#[cfg(unix)]
fn accept_connections(
    listener: &UnixListener,
    stop: &AtomicBool,
    events: &WakingSender<LocalEvent>,
) {
    loop {
        let accepted = listener.accept();
        if stop.load(Ordering::Acquire) {
            return;
        }
        match accepted {
            Ok((stream, _)) => match events.try_send(LocalEvent::Connected {
                stream: Box::new(stream),
            }) {
                // A full queue drops the connection, as the session loop
                // would refuse one past MAX_CONNECTIONS anyway.
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
                Err(mpsc::TrySendError::Disconnected(_)) => return,
            },
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => thread::sleep(ACCEPT_RETRY),
        }
    }
}

#[cfg(any(unix, windows))]
fn spawn_connection(
    connection_id: u64,
    mut stream: AgentStream,
    replies: Receiver<Reply>,
    events: WakingSender<LocalEvent>,
) {
    thread::spawn(move || {
        loop {
            let frame = match read_frame(&mut stream) {
                Ok(frame) => frame,
                Err(error) => {
                    let _ = events.send(LocalEvent::Closed {
                        connection_id,
                        error: (!matches!(error.kind(), io::ErrorKind::UnexpectedEof))
                            .then(|| error.to_string()),
                    });
                    break;
                }
            };
            if events
                .send(LocalEvent::Request {
                    connection_id,
                    frame,
                })
                .is_err()
            {
                break;
            }
            let Ok(reply) = replies.recv() else {
                break;
            };
            if reply.closed {
                let _ = events.send(LocalEvent::Closed {
                    connection_id,
                    error: Some("agent forwarding connection closed".to_owned()),
                });
                break;
            }
            if stream.write_all(&reply.frame).is_err() || stream.flush().is_err() {
                let _ = events.send(LocalEvent::Closed {
                    connection_id,
                    error: Some("writing the local SSH-agent response failed".to_owned()),
                });
                break;
            }
        }
    });
}

#[cfg(any(unix, windows))]
fn read_frame(stream: &mut AgentStream) -> io::Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let body_len = u32::from_be_bytes(length) as usize;
    if body_len == 0 || body_len > MAX_FRAME - 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SSH-agent frame exceeds the frame limit",
        ));
    }
    let mut frame = Vec::with_capacity(body_len + 4);
    frame.extend_from_slice(&length);
    frame.resize(body_len + 4, 0);
    stream.read_exact(&mut frame[4..])?;
    Ok(frame)
}

fn valid_frame(frame: &[u8]) -> bool {
    frame.len() >= 4
        && frame.len() <= MAX_FRAME
        && u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize == frame.len() - 4
}

#[cfg(windows)]
fn create_named_pipe_listener(events: WakingSender<LocalEvent>) -> io::Result<WindowsPipeListener> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED};
    use windows::Win32::System::Pipes::ConnectNamedPipe;

    let name = format!(
        r"\\.\pipe\zosh-agent-{}-{}",
        std::process::id(),
        next_pipe_suffix()
    );
    let wide = name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let path = PathBuf::from(name);
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("zosh-agent-pipe-listener".to_owned())
        .spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut ready_tx = Some(ready_tx);
                loop {
                    let handle = match windows_pipe::create(
                        windows::core::PCWSTR(wide.as_ptr()),
                        MAX_FRAME as u32,
                    ) {
                        Ok(handle) => handle,
                        Err(error) => {
                            if let Some(ready) = ready_tx.take() {
                                let _ = ready.send(Err(error));
                            }
                            break;
                        }
                    };
                    if let Some(ready) = ready_tx.take() {
                        let _ = ready.send(Ok(()));
                    }
                    if stop.load(Ordering::Acquire) {
                        let _ = unsafe { CloseHandle(handle) };
                        break;
                    }
                    let connected = unsafe { ConnectNamedPipe(handle, None) };
                    if let Err(error) = connected
                        && error.code().0 as u32 & 0xffff != ERROR_PIPE_CONNECTED.0
                    {
                        let _ = unsafe { CloseHandle(handle) };
                        continue;
                    }
                    let stream = unsafe { std::fs::File::from_raw_handle(handle.0 as _) };
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    match events.try_send(LocalEvent::Connected {
                        stream: Box::new(stream),
                    }) {
                        Ok(()) | Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => break,
                    }
                }
            }
        })
        .map_err(io::Error::other)?;
    ready_rx
        .recv()
        .map_err(|_| io::Error::other("agent pipe listener exited before readiness"))??;
    Ok(WindowsPipeListener {
        path,
        stop,
        thread: Some(thread),
    })
}

#[cfg(windows)]
fn next_pipe_suffix() -> String {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{:x}", ticks ^ u128::from(std::process::id()))
}

#[cfg(test)]
#[path = "tests/agent.rs"]
mod tests;
