//! The server half of Zosh's opt-in SSH-agent forwarding extension.
//!
//! A private Unix-domain socket is created only after the authenticated client
//! has sent the agent hello. Every local connection is handled by one small
//! reader/writer thread; the session loop only moves complete, bounded frames
//! between that thread and cumulative Mosh host records.

use crate::protocol::AgentHostRecord;
use std::{
    collections::{BTreeMap, HashMap},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
};

#[cfg(unix)]
use std::fs;

pub const MAX_FRAME: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 16;
const MAX_OUTSTANDING: usize = 64;
const EVENT_QUEUE_DEPTH: usize = 64;

#[cfg(unix)]
use std::os::unix::{fs::PermissionsExt, net::UnixListener};

enum LocalEvent {
    Request {
        connection_id: u64,
        frame: Vec<u8>,
    },
    Closed {
        connection_id: u64,
        error: Option<String>,
    },
    #[cfg(windows)]
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

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// State owned by one negotiated remote agent socket.
pub struct AgentServer {
    #[cfg(unix)]
    listener: Option<UnixListener>,
    socket_path: Option<PathBuf>,
    events: Receiver<LocalEvent>,
    event_tx: SyncSender<LocalEvent>,
    connections: HashMap<u64, Connection>,
    outstanding: HashMap<(u64, u64), SyncSender<Reply>>,
    pending: Vec<(u64, AgentHostRecord)>,
    states: BTreeMap<u64, u64>,
    next_connection_id: u64,
    next_record_id: u64,
}

impl AgentServer {
    pub fn new() -> Self {
        let (event_tx, events) = mpsc::sync_channel(EVENT_QUEUE_DEPTH);
        #[cfg(unix)]
        let (listener, socket_path, error) = match create_listener() {
            Ok((listener, path)) => (Some(listener), Some(path), None),
            Err(error) => (None, None, Some(error.to_string())),
        };
        #[cfg(windows)]
        let (socket_path, error) = match create_named_pipe_listener(event_tx.clone()) {
            Ok(path) => (Some(path), None),
            Err(error) => (None, Some(error.to_string())),
        };
        #[cfg(not(any(unix, windows)))]
        let (socket_path, error) = (
            None,
            Some("this build has no SSH-agent socket adapter".to_owned()),
        );

        let mut server = Self {
            #[cfg(unix)]
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
        self.accept_connections();
        let mut dirty = false;
        loop {
            match self.events.try_recv() {
                #[cfg(windows)]
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

    pub fn records(&self) -> Vec<AgentHostRecord> {
        self.pending
            .iter()
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

    fn accept_connections(&mut self) {
        #[cfg(unix)]
        let Some(listener) = self.listener.as_ref() else {
            return;
        };
        #[cfg(unix)]
        for _ in 0..MAX_CONNECTIONS {
            match listener.accept() {
                Ok((stream, _)) => {
                    if self.connections.len() >= MAX_CONNECTIONS {
                        continue;
                    }
                    let connection_id = self.next_connection_id;
                    self.next_connection_id = self.next_connection_id.saturating_add(1);
                    let (replies, reply_rx) = mpsc::sync_channel(1);
                    self.connections
                        .insert(connection_id, Connection { replies });
                    spawn_connection(
                        connection_id,
                        Box::new(stream),
                        reply_rx,
                        self.event_tx.clone(),
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
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
fn create_listener() -> io::Result<(UnixListener, PathBuf)> {
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
    listener.set_nonblocking(true)?;
    Ok((listener, path))
}

#[cfg(any(unix, windows))]
fn spawn_connection(
    connection_id: u64,
    mut stream: AgentStream,
    replies: Receiver<Reply>,
    events: SyncSender<LocalEvent>,
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
fn create_named_pipe_listener(events: SyncSender<LocalEvent>) -> io::Result<PathBuf> {
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
        PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    let name = format!(
        r"\\.\pipe\zosh-agent-{}-{}",
        std::process::id(),
        next_pipe_suffix()
    );
    let wide = name
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    thread::Builder::new()
        .name("zosh-agent-pipe-listener".to_owned())
        .spawn(move || {
            loop {
                let handle = unsafe {
                    CreateNamedPipeW(
                        windows::core::PCWSTR(wide.as_ptr()),
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_BYTE
                            | PIPE_READMODE_BYTE
                            | PIPE_WAIT
                            | PIPE_REJECT_REMOTE_CLIENTS,
                        PIPE_UNLIMITED_INSTANCES,
                        MAX_FRAME as u32,
                        MAX_FRAME as u32,
                        0,
                        None,
                    )
                };
                if handle == INVALID_HANDLE_VALUE {
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
                if events
                    .send(LocalEvent::Connected {
                        stream: Box::new(stream),
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .map_err(io::Error::other)?;
    Ok(PathBuf::from(name))
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
