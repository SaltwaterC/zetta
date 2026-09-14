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

#[cfg(windows)]
use std::fs::OpenOptions;

use mosh_rs::HostEvent;

pub(crate) const AGENT_PROTOCOL_VERSION: u32 = 1;
pub(crate) const AGENT_MAX_FRAME: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 16;
const MAX_OUTSTANDING: usize = 64;
const WORK_QUEUE_DEPTH: usize = 16;
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(3);

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
    results: Receiver<WorkerResult>,
    result_tx: SyncSender<WorkerResult>,
    connections: HashMap<u64, Connection>,
    outstanding: usize,
    negotiated: bool,
    waiting_since: Option<Instant>,
    warned: bool,
}

impl AgentBridge {
    pub(crate) fn new(requested: bool) -> Self {
        let (result_tx, results) = mpsc::sync_channel(MAX_OUTSTANDING);
        let path = requested.then(|| std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from));
        let path = path
            .flatten()
            .filter(|path| !path.as_os_str().is_empty())
            .filter(|path| local_agent_path_exists(path));
        let mut bridge = Self {
            path,
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
                        spawn_worker(*connection_id, path, work_rx, self.result_tx.clone());
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
                    None => match connect_agent(&path) {
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

fn read_frame_from_buffer(stream: &mut AgentStream) -> io::Result<Vec<u8>> {
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

#[cfg(test)]
#[path = "tests/agent.rs"]
mod tests;
