//! OpenSSH transport for a remote `zmux` daemon.
//!
//! The daemon protocol remains the same framed JSON protocol used by local
//! Unix sockets. OpenSSH's stream-local forwarding only supplies the transport
//! between the local client and the remote Unix socket; no request or terminal
//! bytes are handled on an SSH process's output path.

use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::{
    messages::{ClientId, Envelope, PROTOCOL_VERSION, Request, Response},
    transport::{Connection, ENDPOINT_VERSION, Endpoint, Stream},
};
use anyhow::{Context as _, Result};

const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
const FORWARD_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_SSH_OUTPUT_BYTES: usize = 1024 * 1024;

/// A destination understood by OpenSSH.
///
/// `destination` is deliberately passed as one argument to `ssh`, so aliases,
/// `user@host`, IPv6 bracket forms, and the rest of OpenSSH's destination
/// syntax retain their normal meaning. Zetta does not maintain a second host
/// configuration format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTarget {
    destination: String,
    port: Option<u16>,
}

impl RemoteTarget {
    pub fn new(destination: impl Into<String>) -> Self {
        Self {
            destination: destination.into(),
            port: None,
        }
    }

    pub fn with_port(mut self, port: Option<u16>) -> Self {
        self.port = port;
        self
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.destination.trim().is_empty(),
            "SSH target must not be empty"
        );
        anyhow::ensure!(
            !self.destination.starts_with('-'),
            "SSH target must not start with '-'"
        );
        anyhow::ensure!(
            self.port.is_none_or(|port| port != 0),
            "SSH port must be between 1 and 65535"
        );
        Ok(())
    }
}

/// One persistent stream-local SSH forward.
///
/// The child and its private socket directory are owned by this value. When
/// the last client for a remote runtime goes away, dropping the transport
/// terminates SSH and removes the local socket automatically.
struct ForwardState {
    child: Child,
    directory: tempfile::TempDir,
    local_socket: PathBuf,
    endpoint: Endpoint,
}

impl ForwardState {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for ForwardState {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
        // Keep the field explicit: TempDir removes the socket and directory
        // after the child is gone, so no stale forwarding endpoint survives a
        // failed SSH startup or a dropped tab.
        let _ = &self.directory;
    }
}

struct RemoteState {
    forward: Option<ForwardState>,
}

/// A reusable remote mux connection factory.
pub struct RemoteTransport {
    target: RemoteTarget,
    ssh_program: PathBuf,
    state: Mutex<RemoteState>,
}

impl RemoteTransport {
    pub fn new(target: RemoteTarget) -> Result<Self> {
        Self::with_ssh_program(target, PathBuf::from("ssh"))
    }

    /// Uses a specific SSH executable. This is public so transport tests can
    /// run against a small fake SSH program without requiring an SSH server.
    pub fn with_ssh_program(target: RemoteTarget, ssh_program: impl Into<PathBuf>) -> Result<Self> {
        target.validate()?;
        let transport = Self {
            target,
            ssh_program: ssh_program.into(),
            state: Mutex::new(RemoteState { forward: None }),
        };
        transport.refresh()?;
        Ok(transport)
    }

    pub fn target(&self) -> &RemoteTarget {
        &self.target
    }

    /// Returns the endpoint currently exposed by the local side of the
    /// forward. Its token and protocol come from the remote daemon; only the
    /// socket path is replaced with the private local socket.
    pub fn endpoint(&self) -> Result<Endpoint> {
        let state = self.state.lock().unwrap();
        state
            .forward
            .as_ref()
            .map(|forward| forward.endpoint.clone())
            .context("remote SSH forwarding has not been established")
    }

    /// Opens one framed mux connection, rebuilding the forward once if the
    /// persistent SSH process or its local socket has gone away.
    pub fn connect(&self) -> Result<(Endpoint, Stream)> {
        let mut state = self.state.lock().unwrap();
        for attempt in 0..2 {
            if state.forward.is_none() {
                state.forward = Some(self.start_forward()?);
            }

            let (alive, local_socket, endpoint) = {
                let forward = state
                    .forward
                    .as_mut()
                    .expect("the remote forward was just installed");
                (
                    forward.is_alive(),
                    forward.local_socket.clone(),
                    forward.endpoint.clone(),
                )
            };
            if !alive {
                state.forward = None;
                if attempt == 0 {
                    continue;
                }
                anyhow::bail!(
                    "SSH forward for {} exited before a mux connection could be opened",
                    self.target.destination()
                );
            }

            match Stream::connect(&local_socket) {
                Ok(stream) => match self.probe(&endpoint, &stream) {
                    Ok(()) => return Ok((endpoint, stream)),
                    Err(error) if attempt == 0 => {
                        log::debug!(
                            "remote SSH forward for {} failed its mux probe: {error:#}",
                            self.target.destination()
                        );
                        // A local listener can remain open after the remote
                        // daemon has rotated its endpoint. Re-querying on the
                        // next attempt repairs both the token and socket path.
                        state.forward = None;
                    }
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!("checking the SSH forward for {}", self.target.destination())
                        });
                    }
                },
                Err(error) if attempt == 0 => {
                    log::debug!(
                        "remote SSH forward for {} is unavailable: {error}",
                        self.target.destination()
                    );
                    // A live SSH process can still have lost its local
                    // forwarding socket (for example after the daemon
                    // endpoint changed). Drop it before retrying so the
                    // next attempt creates a fresh private socket and SSH
                    // process instead of trying the same stale path.
                    state.forward = None;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "connecting to the SSH forward for {}",
                            self.target.destination()
                        )
                    });
                }
            }
            if attempt == 0 {
                continue;
            }
        }
        anyhow::bail!(
            "could not connect to the SSH forward for {}",
            self.target.destination()
        )
    }

    /// Confirms that the persistent forward still reaches the daemon described
    /// by its cached endpoint. A stream-local listener can outlive a remote
    /// daemon replacement, so checking only the local socket is insufficient:
    /// the next real request would otherwise be sent with a stale token or to a
    /// stale remote socket. The probe is a normal mux request and never starts
    /// another SSH process.
    fn probe(&self, endpoint: &Endpoint, stream: &Stream) -> Result<()> {
        let mut connection = Connection::new(stream.try_clone()?);
        connection.set_read_timeout(Some(PROBE_TIMEOUT))?;
        connection.stream().set_write_timeout(Some(PROBE_TIMEOUT))?;
        connection.send(&Envelope {
            version: PROTOCOL_VERSION,
            token: endpoint.token.clone(),
            client_process_id: std::process::id(),
            client_id: ClientId::random()?,
            stream_only: true,
            session_secret: None,
            request: Request::Ping,
        })?;
        match connection.receive::<Response>()?.0 {
            Response::Ok => Ok(()),
            Response::Error { message } => anyhow::bail!("{message}"),
            response => anyhow::bail!("unexpected response to mux probe: {response:?}"),
        }
    }

    /// Re-queries `zmux endpoint --json` and replaces the forward. This is used
    /// after an invalid token or a remote daemon replacement, where both the
    /// socket path and token may have changed.
    pub fn refresh(&self) -> Result<Endpoint> {
        let mut state = self.state.lock().unwrap();
        state.forward = None;
        state.forward = Some(self.start_forward()?);
        Ok(state
            .forward
            .as_ref()
            .expect("the forward was just installed")
            .endpoint
            .clone())
    }

    fn start_forward(&self) -> Result<ForwardState> {
        let remote_endpoint = self.query_endpoint()?;
        let directory = tempfile::Builder::new()
            .prefix("zetta-zmux-")
            .tempdir()
            .context("creating the private SSH forward directory")?;
        restrict_directory(directory.path())?;
        let local_socket = directory.path().join("mux.sock");
        let forwarding = format!(
            "{}:{}",
            local_socket.display(),
            remote_endpoint.socket_path.display()
        );
        let arguments = forward_arguments(&self.target, &forwarding);
        let mut command = Command::new(&self.ssh_program);
        command
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .with_context(|| format!("starting SSH for {}", self.target.destination))?;

        let deadline = Instant::now() + FORWARD_TIMEOUT;
        loop {
            match Stream::connect(&local_socket) {
                Ok(_) => {
                    if child
                        .try_wait()
                        .context("checking SSH forward status")?
                        .is_some()
                    {
                        terminate_child(&mut child);
                        anyhow::bail!(
                            "SSH exited before its stream-local forward became ready for {}",
                            self.target.destination
                        );
                    }
                    let endpoint = Endpoint {
                        version: remote_endpoint.version,
                        protocol_version: remote_endpoint.protocol_version,
                        process_id: remote_endpoint.process_id,
                        socket_path: local_socket.clone(),
                        token: remote_endpoint.token,
                    };
                    return Ok(ForwardState {
                        child,
                        directory,
                        local_socket,
                        endpoint,
                    });
                }
                Err(error) => {
                    if let Some(status) = child.try_wait().context("checking SSH forward status")? {
                        terminate_child(&mut child);
                        anyhow::bail!(
                            "SSH exited with {status} while forwarding {}: stream socket was not ready ({error})",
                            self.target.destination
                        );
                    }
                }
            }
            if Instant::now() >= deadline {
                terminate_child(&mut child);
                anyhow::bail!(
                    "SSH stream-local forward for {} did not become ready within {FORWARD_TIMEOUT:?}",
                    self.target.destination
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn query_endpoint(&self) -> Result<Endpoint> {
        let arguments = endpoint_arguments(&self.target);
        let output = run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT)?;
        let text = std::str::from_utf8(&output.stdout)
            .context("remote endpoint output was not UTF-8")?
            .trim();
        anyhow::ensure!(!text.is_empty(), "remote zmux endpoint returned no JSON");
        let endpoint: Endpoint =
            serde_json::from_str(text).context("parsing remote `zmux endpoint --json` output")?;
        anyhow::ensure!(
            endpoint.version == ENDPOINT_VERSION,
            "remote multiplexer endpoint has unsupported version {}",
            endpoint.version
        );
        anyhow::ensure!(
            endpoint.protocol_version == PROTOCOL_VERSION,
            "remote multiplexer speaks protocol version {}, not {PROTOCOL_VERSION}",
            endpoint.protocol_version
        );
        anyhow::ensure!(
            !endpoint.socket_path.as_os_str().is_empty(),
            "remote multiplexer endpoint has an empty socket path"
        );
        Ok(endpoint)
    }
}

struct SshOutput {
    stdout: Vec<u8>,
}

fn endpoint_arguments(target: &RemoteTarget) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    if let Some(port) = target.port {
        arguments.push("-p".to_owned());
        arguments.push(port.to_string());
    }
    arguments.extend([
        target.destination.clone(),
        "zmux".to_owned(),
        "endpoint".to_owned(),
        "--json".to_owned(),
    ]);
    arguments
}

fn forward_arguments(target: &RemoteTarget, forwarding: &str) -> Vec<String> {
    let mut arguments = vec![
        "-T".to_owned(),
        "-N".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
    ];
    if let Some(port) = target.port {
        arguments.push("-p".to_owned());
        arguments.push(port.to_string());
    }
    arguments.extend([
        "-L".to_owned(),
        forwarding.to_owned(),
        target.destination.clone(),
    ]);
    arguments
}

/// `Child` does not terminate a process when it is dropped. Every failed
/// forwarding attempt must therefore explicitly reap SSH, otherwise repeated
/// endpoint refreshes leave one zombie (or one live SSH process) per retry.
fn terminate_child(child: &mut Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_capture(program: &Path, arguments: &[String], timeout: Duration) -> Result<SshOutput> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting SSH executable {}", program.display()))?;
    let stdout = child.stdout.take().context("SSH stdout was not captured")?;
    let stderr = child.stderr.take().context("SSH stderr was not captured")?;
    let stdout_thread = thread::spawn(|| read_limited(stdout));
    let stderr_thread = thread::spawn(|| read_limited(stderr));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate_child(&mut child);
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(error).context("checking SSH endpoint query status");
            }
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            let _ = stdout_thread.join();
            let _ = stderr_thread.join();
            anyhow::bail!("SSH endpoint query timed out after {timeout:?}");
        }
        thread::sleep(POLL_INTERVAL);
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("SSH stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("SSH stderr reader panicked"))??;
    anyhow::ensure!(
        status.success(),
        "SSH endpoint query failed with {status}: {}",
        String::from_utf8_lossy(&stderr).trim()
    );
    Ok(SshOutput { stdout })
}

fn read_limited(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_SSH_OUTPUT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .context("reading SSH output")?;
    anyhow::ensure!(
        bytes.len() <= MAX_SSH_OUTPUT_BYTES,
        "SSH endpoint output exceeded {MAX_SSH_OUTPUT_BYTES} bytes"
    );
    Ok(bytes)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting SSH forward directory {}", path.display()))
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
#[path = "tests/remote.rs"]
mod tests;
