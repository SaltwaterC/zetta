//! The Linux half: the Unix socket `SSH_AUTH_SOCK` names inside the
//! distribution.
//!
//! The relay lives exactly as long as the `wsl.exe` that started it: its stdin
//! is `wslx.exe`'s, so when `wslx.exe` exits — however it exits — the relay
//! reads end-of-file, closes every connection and removes its socket. Nothing
//! is left listening for a session that has gone.
//!
//! The socket sits in a directory only the user can enter, so permission on
//! that directory is what keeps other users of the distribution away from the
//! agent; the socket's own mode is not relied on.

use std::{
    collections::HashMap,
    env, fs,
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::DirBuilderExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    bootstrap::LINE_PREFIX,
    lock,
    protocol::{self, Message, read_agent_frame},
};

/// The entry point of `wslx-relay serve`.
pub fn main() -> ExitCode {
    if env::args().skip(1).ne(["serve"]) {
        eprintln!("wslx-relay is started by wslx.exe and is not meant to be run directly");
        return ExitCode::from(2);
    }
    let mut stdout = io::stdout();
    let mut failures = Vec::new();
    let bound = socket_parents().find_map(|parent| {
        Relay::bind(&parent)
            .map_err(|error| failures.push(format!("{}: {error}", parent.display())))
            .ok()
    });
    let Some(relay) = bound else {
        let _ = writeln!(
            stdout,
            "{LINE_PREFIX}error the agent socket could not be created ({})",
            failures.join("; ")
        );
        return ExitCode::FAILURE;
    };
    let announced = writeln!(
        stdout,
        "{LINE_PREFIX}ready {}",
        relay.socket_path().display()
    )
    .and_then(|()| stdout.flush());
    if announced.is_err() {
        return ExitCode::FAILURE;
    }
    match relay.serve(io::stdin().lock(), stdout) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("wslx-relay: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Where the socket directory may go, best first. `XDG_RUNTIME_DIR` is
/// private to the user and cleared at shutdown; `/tmp` is the fallback when a
/// distribution has none. A path that is not UTF-8 is skipped, because the
/// socket path has to be announced on a text line.
fn socket_parents() -> impl Iterator<Item = PathBuf> {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.to_str().is_some())
        .into_iter()
        .chain(std::iter::once(PathBuf::from("/tmp")))
}

/// A listening socket in a private directory of its own, both removed when
/// the relay is dropped.
pub struct Relay {
    directory: PathBuf,
    socket: PathBuf,
    listener: UnixListener,
}

/// One client of the socket, as the dispatcher sees it.
struct Connection {
    replies: mpsc::Sender<Vec<u8>>,
    /// A second handle on the client, so the dispatcher can end a connection
    /// whose thread is blocked reading it.
    stream: UnixStream,
}

type Connections = Arc<Mutex<HashMap<u32, Connection>>>;

impl Relay {
    pub fn bind(parent: &Path) -> io::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let directory = parent.join(format!("wslx-{}-{nonce:x}", std::process::id()));
        // `create`, not `create_all`: an existing directory of that name is
        // somebody else's, and must not be used.
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let socket = directory.join("agent.sock");
        match UnixListener::bind(&socket) {
            Ok(listener) => Ok(Self {
                directory,
                socket,
                listener,
            }),
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                Err(error)
            }
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Serves the socket until `input` ends, carrying each connection over
    /// `input` and `output`.
    pub fn serve<W: Write + Send + 'static>(
        &self,
        mut input: impl Read,
        output: W,
    ) -> io::Result<()> {
        let output = Arc::new(Mutex::new(output));
        let connections = Connections::default();
        let stopping = Arc::new(AtomicBool::new(false));
        let acceptor = thread::Builder::new()
            .name("wslx-relay-accept".to_owned())
            .spawn({
                let listener = self.listener.try_clone()?;
                let output = Arc::clone(&output);
                let connections = Arc::clone(&connections);
                let stopping = Arc::clone(&stopping);
                move || accept_connections(&listener, &output, &connections, &stopping)
            })?;

        let result = dispatch(&mut input, &connections);

        stopping.store(true, Ordering::Release);
        // Wake the acceptor so it sees `stopping`; if this fails the
        // listener is already broken and the acceptor has returned.
        let _ = UnixStream::connect(&self.socket);
        let _ = acceptor.join();
        for (_, connection) in lock(&connections).drain() {
            let _ = connection.stream.shutdown(Shutdown::Both);
        }
        result
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket);
        let _ = fs::remove_dir(&self.directory);
    }
}

/// Routes what `wslx.exe` sends to the connections it belongs to, until the
/// stream ends.
fn dispatch(input: &mut impl Read, connections: &Connections) -> io::Result<()> {
    while let Some(message) = Message::read_from(input)? {
        match message {
            Message::Reply(id, frame) => {
                if let Some(connection) = lock(connections).get(&id) {
                    // A closed receiver means the client left while the reply
                    // was in flight; its thread has already said so.
                    let _ = connection.replies.send(frame);
                }
            }
            Message::Close(id) => {
                if let Some(connection) = lock(connections).remove(&id) {
                    let _ = connection.stream.shutdown(Shutdown::Both);
                }
            }
            Message::Open(_) | Message::Request(..) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "wslx.exe sent a message only the relay sends",
                ));
            }
        }
    }
    Ok(())
}

fn accept_connections<W: Write + Send + 'static>(
    listener: &UnixListener,
    output: &Arc<Mutex<W>>,
    connections: &Connections,
    stopping: &AtomicBool,
) {
    let mut next_id = 0_u32;
    for stream in listener.incoming() {
        if stopping.load(Ordering::Acquire) {
            return;
        }
        let Ok(stream) = stream else {
            // Typically out of descriptors; give the open connections a
            // moment to finish rather than spinning.
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        let Ok(handle) = stream.try_clone() else {
            continue;
        };
        next_id = next_id.wrapping_add(1);
        let id = next_id;
        let (replies, received) = mpsc::channel();
        lock(connections).insert(
            id,
            Connection {
                replies,
                stream: handle,
            },
        );
        // `Open` goes out before the thread exists, so it always precedes the
        // connection's first request.
        if protocol::send(output, &Message::Open(id)).is_err() {
            return;
        }
        let spawned = thread::Builder::new()
            .name(format!("wslx-relay-{id}"))
            .spawn({
                let output = Arc::clone(output);
                let connections = Arc::clone(connections);
                move || serve_connection(id, stream, &received, &output, &connections)
            });
        if spawned.is_err() && lock(connections).remove(&id).is_some() {
            let _ = protocol::send(output, &Message::Close(id));
        }
    }
}

/// Carries one client's requests across, a request and its reply at a time.
fn serve_connection(
    id: u32,
    mut client: UnixStream,
    replies: &mpsc::Receiver<Vec<u8>>,
    output: &Mutex<impl Write>,
    connections: &Mutex<HashMap<u32, Connection>>,
) {
    while let Ok(Some(request)) = read_agent_frame(&mut client) {
        if protocol::send(output, &Message::Request(id, request)).is_err() {
            break;
        }
        let Ok(reply) = replies.recv() else {
            break;
        };
        if client.write_all(&reply).is_err() {
            break;
        }
    }
    // Whoever removes the entry is the one who closed first, and the only
    // one who reports it.
    if lock(connections).remove(&id).is_some() {
        let _ = protocol::send(output, &Message::Close(id));
    }
}

#[cfg(test)]
#[path = "tests/relay.rs"]
mod tests;
