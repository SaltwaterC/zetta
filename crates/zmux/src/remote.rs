//! OpenSSH transport for a remote `zmux` daemon.
//!
//! The daemon protocol remains the same framed JSON protocol used by local
//! Unix sockets. Unix clients use OpenSSH stream-local forwarding. Windows
//! clients run a remote stdio proxy because Win32-OpenSSH cannot listen on a
//! local Unix socket. The proxy reads the daemon endpoint on the remote host;
//! no local TCP listener or client-supplied remote socket path is involved.

#[cfg(windows)]
use std::{io, net::Shutdown};
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
#[cfg(unix)]
const FORWARD_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_SSH_OUTPUT_BYTES: usize = 1024 * 1024;

// OpenSSH runs a remote command through the account's shell without making it
// interactive. Zetta's installed CLI path is commonly added to an
// interactive shell rc file, so ask that same shell to load its rc file
// before resolving zmux. Startup files may write prompts or terminal-control
// sequences to stdout, so keep that output away from the endpoint JSON and
// send only the command's stdout through fd 3. The POSIX wrapper owns the fd
// setup so the account's shell can be fish or another shell with different
// redirection syntax. Keep this as one command argument: OpenSSH joins the
// remote command arguments into the command string it gives the server.
const REMOTE_ENDPOINT_COMMAND: &str = r#"/bin/sh -c 'exec 3>&1 1>/dev/null; exec "${SHELL:-/bin/sh}" -lic "command zmux endpoint --json >&3"'"#;

/// Where that same shell resolves `zmux`. Written the same way, and for the
/// same reasons, as the endpoint command above.
const REMOTE_PROGRAM_COMMAND: &str =
    r#"/bin/sh -c 'exec 3>&1 1>/dev/null; exec "${SHELL:-/bin/sh}" -lic "command -v zmux >&3"'"#;

/// Runs the standalone profile discovery command without requiring a daemon.
const REMOTE_PROFILES_COMMAND: &str = r#"/bin/sh -c 'exec 3>&1 1>/dev/null; exec "${SHELL:-/bin/sh}" -lic "exec zmux profiles --json >&3"'"#;

#[cfg(windows)]
const REMOTE_STDIO_COMMAND: &str = r#"/bin/sh -c 'exec 3>&1 1>/dev/null; exec "${SHELL:-/bin/sh}" -lic "exec zmux proxy-stdio >&3"'"#;

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
    forward_agent: Option<bool>,
}

impl RemoteTarget {
    pub fn new(destination: impl Into<String>) -> Self {
        Self {
            destination: destination.into(),
            port: None,
            forward_agent: None,
        }
    }

    pub fn with_port(mut self, port: Option<u16>) -> Self {
        self.port = port;
        self
    }

    /// Explicitly enables or disables OpenSSH's native agent forwarding.
    pub fn with_forward_agent(mut self, forward_agent: bool) -> Self {
        self.forward_agent = Some(forward_agent);
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
#[cfg(unix)]
struct ForwardState {
    child: Child,
    directory: tempfile::TempDir,
    local_socket: PathBuf,
    endpoint: Endpoint,
}

#[cfg(unix)]
impl ForwardState {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[cfg(unix)]
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
    #[cfg(unix)]
    forward: Option<ForwardState>,
    #[cfg(windows)]
    endpoint: Option<Endpoint>,
    #[cfg(windows)]
    agent_holder: Option<Child>,
}

#[cfg(windows)]
impl Drop for RemoteState {
    fn drop(&mut self) {
        if let Some(mut child) = self.agent_holder.take() {
            terminate_child(&mut child);
        }
    }
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
        let transport = Self::for_creation_with_ssh_program(target, ssh_program)?;
        transport.refresh()?;
        Ok(transport)
    }

    /// Creates a transport for an operation that may need to start the remote
    /// daemon. Unlike [`Self::with_ssh_program`], this does not query an
    /// existing endpoint during construction.
    pub fn for_creation(target: RemoteTarget) -> Result<Self> {
        Self::for_creation_with_ssh_program(target, PathBuf::from("ssh"))
    }

    /// Testable form of [`Self::for_creation`].
    pub fn for_creation_with_ssh_program(
        target: RemoteTarget,
        ssh_program: impl Into<PathBuf>,
    ) -> Result<Self> {
        target.validate()?;
        Ok(Self {
            target,
            ssh_program: ssh_program.into(),
            state: Mutex::new(RemoteState {
                #[cfg(unix)]
                forward: None,
                #[cfg(windows)]
                endpoint: None,
                #[cfg(windows)]
                agent_holder: None,
            }),
        })
    }

    pub fn target(&self) -> &RemoteTarget {
        &self.target
    }

    /// Returns the endpoint currently exposed by the local side of the
    /// forward. Its token and protocol come from the remote daemon; only the
    /// socket path is replaced with the private local socket.
    #[cfg(unix)]
    pub fn endpoint(&self) -> Result<Endpoint> {
        let state = self.state.lock().unwrap();
        state
            .forward
            .as_ref()
            .map(|forward| forward.endpoint.clone())
            .context("remote SSH forwarding has not been established")
    }

    #[cfg(windows)]
    pub fn endpoint(&self) -> Result<Endpoint> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .endpoint
            .clone()
            .context("remote SSH endpoint has not been queried")
    }

    /// Opens one framed mux connection, rebuilding the forward once if the
    /// persistent SSH process or its local socket has gone away.
    #[cfg(unix)]
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
                Ok(stream) => match self.probe(&endpoint, stream) {
                    Ok(()) => match Stream::connect(&local_socket) {
                        Ok(stream) => return Ok((endpoint, stream)),
                        Err(error) if attempt == 0 => {
                            log::debug!(
                                "remote SSH forward for {} disappeared after its mux probe: {error}",
                                self.target.destination()
                            );
                            state.forward = None;
                        }
                        Err(error) => {
                            return Err(error).with_context(|| {
                                format!(
                                    "opening a request connection to the SSH forward for {}",
                                    self.target.destination()
                                )
                            });
                        }
                    },
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

    #[cfg(windows)]
    pub fn connect(&self) -> Result<(Endpoint, Stream)> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for attempt in 0..2 {
            if self.target.forward_agent == Some(true)
                && state
                    .agent_holder
                    .as_mut()
                    .is_none_or(|child| !matches!(child.try_wait(), Ok(None)))
            {
                if let Some(mut child) = state.agent_holder.take() {
                    terminate_child(&mut child);
                }
                state.endpoint = None;
            }
            let endpoint = match &state.endpoint {
                Some(endpoint) => endpoint.clone(),
                None => {
                    let endpoint = self.query_stdio_endpoint(&mut state)?;
                    state.endpoint = Some(endpoint.clone());
                    endpoint
                }
            };
            let probe = self.open_stdio_bridge()?;
            match self.probe(&endpoint, probe) {
                Ok(()) => return Ok((endpoint, self.open_stdio_bridge()?)),
                Err(error) if attempt == 0 => {
                    log::debug!(
                        "remote SSH stdio probe for {} failed: {error:#}",
                        self.target.destination()
                    );
                    state.endpoint = None;
                    if let Some(mut child) = state.agent_holder.take() {
                        terminate_child(&mut child);
                    }
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "checking the SSH stdio proxy for {} (the remote zmux must support proxy-stdio)",
                            self.target.destination()
                        )
                    });
                }
            }
        }
        anyhow::bail!(
            "could not connect to the SSH stdio proxy for {}",
            self.target.destination()
        )
    }

    #[cfg(windows)]
    fn open_stdio_bridge(&self) -> Result<Stream> {
        use crate::transport::Listener;

        let directory = tempfile::Builder::new()
            .prefix("zetta-zmux-stdio-")
            .tempdir()
            .context("creating the private SSH stdio directory")?;
        let socket = directory.path().join("mux.sock");
        let listener = Listener::bind(&socket).context("binding the local SSH stdio socket")?;
        let client =
            Stream::connect(&socket).context("connecting to the local SSH stdio socket")?;
        let (relay, _) = listener
            .accept()
            .context("accepting the local SSH stdio socket")?;
        drop(listener);
        let target = self.target.clone();
        let ssh_program = self.ssh_program.clone();
        thread::Builder::new()
            .name("zmux-ssh-stdio".to_owned())
            .spawn(move || run_stdio_bridge(ssh_program, target, relay, directory))
            .context("starting the SSH stdio bridge")?;
        Ok(client)
    }

    /// Confirms that the transport reaches the daemon described by its cached
    /// endpoint. A listener can outlive a daemon replacement, so checking only
    /// the local socket is insufficient: the next request could carry a stale
    /// token. On Windows each probe has its own SSH stdio process.
    fn probe(&self, endpoint: &Endpoint, stream: Stream) -> Result<()> {
        // Ping is served on a one-request connection and the daemon closes it
        // after replying. Consume this connection completely and let the
        // caller open a fresh one for its actual request; reusing it would
        // make the request race with the peer's EOF.
        let mut connection = Connection::new(stream);
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
    #[cfg(unix)]
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

    #[cfg(windows)]
    pub fn refresh(&self) -> Result<Endpoint> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.endpoint = None;
        if let Some(mut child) = state.agent_holder.take() {
            terminate_child(&mut child);
        }
        let endpoint = self.query_stdio_endpoint(&mut state)?;
        state.endpoint = Some(endpoint.clone());
        Ok(endpoint)
    }

    #[cfg(windows)]
    fn query_stdio_endpoint(&self, state: &mut RemoteState) -> Result<Endpoint> {
        let endpoint = self.query_endpoint()?;
        if self.target.forward_agent == Some(true)
            && let Some(directory) = endpoint.socket_path.parent()
        {
            let socket = directory.join("forwarded-agent.sock");
            let mut command = Command::new(&self.ssh_program);
            command
                .args(agent_holder_arguments(&self.target, &socket))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            state.agent_holder = Some(command.spawn().context("starting SSH agent forwarding")?);
        }
        Ok(endpoint)
    }

    #[cfg(unix)]
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
        let remote_agent_socket = remote_endpoint
            .socket_path
            .parent()
            .map(|directory| directory.join("forwarded-agent.sock"));
        let arguments =
            forward_arguments(&self.target, &forwarding, remote_agent_socket.as_deref());
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

    /// Resolves the remote host's own `zmux` executable.
    ///
    /// Same shell wrapper and the same fd discipline as the endpoint query,
    /// for the same reason: the directory Zetta's CLI is installed into is
    /// usually added to an interactive rc file, and rc files write to stdout.
    ///
    /// What comes back is an absolute path, which is what makes it worth
    /// asking for: a `zmux` started somewhere other than an SSH command —
    /// inside a Mosh server, say — cannot rely on the `PATH` that found this
    /// one.
    pub fn resolve_remote_program(&self) -> Result<PathBuf> {
        let arguments = program_arguments(&self.target);
        let output = run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT)?;
        let text = std::str::from_utf8(&output.stdout)
            .context("remote zmux path was not UTF-8")?
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned();
        anyhow::ensure!(
            !text.is_empty(),
            "the remote host has no zmux on the path its shell resolves"
        );
        let path = PathBuf::from(text);
        anyhow::ensure!(
            path.is_absolute(),
            "the remote host resolved zmux to {}, which is not an absolute path",
            path.display()
        );
        Ok(path)
    }

    /// Returns the names the remote host can resolve through its standalone
    /// `zmux` profile boundary. This intentionally does not start a daemon.
    pub fn query_profiles(&self) -> Result<Vec<String>> {
        let output = run_capture(
            &self.ssh_program,
            &profiles_arguments(&self.target),
            ENDPOINT_TIMEOUT,
        )?;
        let text = std::str::from_utf8(&output.stdout)
            .context("remote profile output was not UTF-8")?
            .trim();
        anyhow::ensure!(
            !text.is_empty(),
            "remote zmux profile query returned no JSON"
        );
        let mut profiles: Vec<String> =
            serde_json::from_str(text).context("parsing remote `zmux profiles --json` output")?;
        profiles.retain(|profile| !profile.trim().is_empty());
        profiles.sort_unstable_by_key(|profile| profile.to_ascii_lowercase());
        profiles.dedup_by(|left, right| left.eq_ignore_ascii_case(right));
        anyhow::ensure!(
            !profiles.is_empty(),
            "remote zmux profile query returned no profiles"
        );
        Ok(profiles)
    }

    /// Starts the remote standalone daemon when it is not already reachable,
    /// then waits for its endpoint to become available.
    pub fn ensure_daemon(&self) -> Result<Endpoint> {
        if let Ok(endpoint) = self.query_endpoint() {
            return Ok(endpoint);
        }
        let program = self.resolve_remote_program()?;
        let arguments = start_daemon_arguments(&self.target, &program);
        run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT)
            .context("starting the remote zmux daemon")?;
        let deadline = Instant::now() + ENDPOINT_TIMEOUT;
        loop {
            match self.query_endpoint() {
                Ok(endpoint) => return Ok(endpoint),
                Err(error) if Instant::now() < deadline => {
                    log::debug!(
                        "remote zmux daemon for {} is not ready: {error:#}",
                        self.target.destination()
                    );
                    thread::sleep(POLL_INTERVAL);
                }
                Err(error) => {
                    return Err(error).context("waiting for the remote zmux daemon endpoint");
                }
            }
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

/// The remote half of the Windows transport. The endpoint path comes from this
/// host's daemon catalog, never from an SSH command argument. The daemon still
/// validates the client's endpoint token on its normal framed connection.
#[cfg(unix)]
pub fn run_stdio_proxy() -> Result<()> {
    use std::io;

    let endpoint = Endpoint::read(&crate::server::endpoint_path(
        &crate::paths::session_catalog_dir(),
    ))?;
    let daemon = Stream::connect(&endpoint.socket_path)
        .context("connecting the SSH stdio proxy to the local daemon")?;
    proxy_stream(daemon, io::stdin(), io::stdout())
}

#[cfg(unix)]
fn proxy_stream(
    mut daemon: Stream,
    mut input: impl Read + Send + 'static,
    mut output: impl std::io::Write,
) -> Result<()> {
    use std::{io, net::Shutdown};

    let mut daemon_input = daemon.try_clone()?;
    thread::spawn(move || {
        let _ = io::copy(&mut input, &mut daemon_input);
        let _ = daemon_input.shutdown(Shutdown::Write);
    });
    let mut bytes = [0; 8192];
    loop {
        let count = daemon
            .read(&mut bytes)
            .context("reading the daemon response for SSH stdout")?;
        if count == 0 {
            break;
        }
        output
            .write_all(&bytes[..count])
            .context("copying the daemon response to SSH stdout")?;
        output.flush().context("flushing SSH stdout")?;
    }
    Ok(())
}

#[cfg(windows)]
fn run_stdio_bridge(
    ssh_program: PathBuf,
    target: RemoteTarget,
    relay: Stream,
    _directory: tempfile::TempDir,
) {
    let mut command = Command::new(ssh_program);
    command
        .args(stdio_arguments(&target))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Ok(mut child) = command.spawn() else {
        log::debug!(
            "could not start SSH stdio proxy for {}",
            target.destination()
        );
        return;
    };
    copy_stdio_child(&mut child, relay, target.destination());
}

#[cfg(windows)]
fn copy_stdio_child(child: &mut Child, mut relay: Stream, destination: &str) {
    let (Some(mut ssh_input), Some(mut ssh_output), Some(ssh_error)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        terminate_child(child);
        return;
    };
    let Ok(mut relay_input) = relay.try_clone() else {
        terminate_child(child);
        return;
    };
    let input = thread::spawn(move || {
        let _ = io::copy(&mut relay_input, &mut ssh_input);
    });
    let errors = thread::spawn(move || capture_ssh_stderr(ssh_error));
    let result = io::copy(&mut ssh_output, &mut relay);
    let _ = relay.shutdown(Shutdown::Both);
    let _ = input.join();
    terminate_child(child);
    let stderr = errors.join().unwrap_or_default();
    if let Err(error) = result {
        log::debug!("SSH stdio proxy for {destination} stopped: {error}");
    }
    if !stderr.is_empty() {
        log::debug!(
            "SSH stdio proxy for {}: {}",
            destination,
            String::from_utf8_lossy(&stderr).trim()
        );
    }
}

#[cfg(windows)]
fn capture_ssh_stderr(mut stderr: impl Read) -> Vec<u8> {
    let mut captured = Vec::new();
    let mut buffer = [0; 4096];
    while let Ok(count) = stderr.read(&mut buffer) {
        if count == 0 {
            break;
        }
        let remaining = MAX_SSH_OUTPUT_BYTES.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..count.min(remaining)]);
    }
    captured
}

struct SshOutput {
    stdout: Vec<u8>,
}

fn endpoint_arguments(target: &RemoteTarget) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    push_target_options(&mut arguments, target);
    arguments.extend([
        target.destination.clone(),
        REMOTE_ENDPOINT_COMMAND.to_owned(),
    ]);
    arguments
}

fn program_arguments(target: &RemoteTarget) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    push_target_options(&mut arguments, target);
    arguments.extend([
        target.destination.clone(),
        REMOTE_PROGRAM_COMMAND.to_owned(),
    ]);
    arguments
}

fn profiles_arguments(target: &RemoteTarget) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    push_target_options(&mut arguments, target);
    arguments.extend([
        target.destination.clone(),
        REMOTE_PROFILES_COMMAND.to_owned(),
    ]);
    arguments
}

#[cfg(windows)]
fn stdio_arguments(target: &RemoteTarget) -> Vec<String> {
    let mut arguments = vec![
        "-T".to_owned(),
        "-o".to_owned(),
        "ClearAllForwardings=yes".to_owned(),
    ];
    push_target_options(&mut arguments, target);
    arguments.extend([target.destination.clone(), REMOTE_STDIO_COMMAND.to_owned()]);
    arguments
}

#[cfg(windows)]
fn agent_holder_arguments(target: &RemoteTarget, socket: &Path) -> Vec<String> {
    let mut arguments = vec![
        "-T".to_owned(),
        "-o".to_owned(),
        "ClearAllForwardings=yes".to_owned(),
    ];
    push_target_options(&mut arguments, target);
    arguments.extend([target.destination.clone(), agent_holder_command(socket)]);
    arguments
}

fn start_daemon_arguments(target: &RemoteTarget, program: &Path) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    push_target_options(&mut arguments, target);
    let program = shell_escape_double_quoted(&program.to_string_lossy());
    let command = format!(
        r#"/bin/sh -c 'exec "${{SHELL:-/bin/sh}}" -lic "nohup \"{program}\" --daemon >/dev/null 2>&1 </dev/null &"'"#
    );
    arguments.extend([target.destination.clone(), command]);
    arguments
}

fn shell_escape_double_quoted(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
}

#[cfg(any(unix, test))]
fn forward_arguments(
    target: &RemoteTarget,
    forwarding: &str,
    remote_agent_socket: Option<&Path>,
) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    if target.forward_agent != Some(true) {
        arguments.push("-N".to_owned());
    }
    arguments.extend(["-o".to_owned(), "ExitOnForwardFailure=yes".to_owned()]);
    push_target_options(&mut arguments, target);
    arguments.extend(["-L".to_owned(), forwarding.to_owned()]);
    arguments.push(target.destination.clone());
    if target.forward_agent == Some(true)
        && let Some(socket) = remote_agent_socket
    {
        arguments.push(agent_holder_command(socket));
    }
    arguments
}

fn agent_holder_command(socket: &Path) -> String {
    let socket = shell_quote(&socket.to_string_lossy());
    let script = format!(
        "umask 077; link={socket}; temporary=\"$link.$$\"; if test -n \"$SSH_AUTH_SOCK\"; then rm -f \"$temporary\"; ln -s \"$SSH_AUTH_SOCK\" \"$temporary\" && mv -f \"$temporary\" \"$link\"; fi; exec sleep 2147483647"
    );
    format!("/bin/sh -c {}", shell_quote(&script))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn push_target_options(arguments: &mut Vec<String>, target: &RemoteTarget) {
    if let Some(forward_agent) = target.forward_agent {
        arguments.push(if forward_agent { "-A" } else { "-a" }.to_owned());
    }
    if let Some(port) = target.port {
        arguments.push("-p".to_owned());
        arguments.push(port.to_string());
    }
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

#[cfg(test)]
#[path = "tests/remote.rs"]
mod tests;
