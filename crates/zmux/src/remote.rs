//! OpenSSH transport for a remote `zmux` daemon.
//!
//! The daemon protocol remains the same framed JSON protocol used by local
//! Unix sockets; what differs per platform is how a connection gets there, and
//! both are built around one SSH login per remote host rather than one per
//! request, because a login is several round trips and a slow link makes each
//! of them felt:
//!
//! - **Unix clients** own a private OpenSSH `ControlMaster`. Everything else —
//!   the endpoint query, the stream-local forward (added with `-O forward`),
//!   the profile query, a daemon start, the agent holder, and the Mosh pane
//!   bootstraps that borrow [`RemoteTransport::control_path`] — is a session on
//!   that one connection.
//! - **Windows clients** cannot do that: Win32-OpenSSH has neither connection
//!   sharing nor local Unix socket forwarding. They keep one
//!   `ssh HOST zmux proxy-mux` running instead and carry every connection over
//!   its stdio; see `mux_bridge.rs`. The bridge reports the endpoint itself, so
//!   no separate query login is needed.
//! - **Windows hosts** refuse stream-local forwarding on the server side too,
//!   so every client reaches one through the same bridge — a Unix client runs
//!   it as a session on its master. Which hosts those are is learned the first
//!   time a POSIX command fails there; see `remote_host.rs`.
//!
//! A connection is probed (a `Ping` on a connection of its own) only when
//! nothing has recently shown the endpoint to be good. A listener can outlive
//! a daemon replacement, so an unprobed stream could carry a stale token — but
//! any response to a real request proves the token just as well, so the client
//! reports those back ([`RemoteTransport::confirm`]) and a failure withdraws
//! the trust ([`RemoteTransport::distrust`]).
//!
//! Transports are shared per target ([`RemoteTransport::shared`]). A process
//! that opts in with [`keep_idle_transports`] also keeps each one for
//! [`LINGER`] after its last user, so the picker's connection is still there
//! when the user picks a session from it. Opting in is a promise to call
//! [`release_idle_transports`] before exiting: a transport held by a static is
//! never dropped, and its SSH process would outlive this one.

#[cfg(unix)]
use std::sync::atomic::AtomicU64;
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    messages::{ClientId, Envelope, PROTOCOL_VERSION, Request, Response},
    remote_host::{self, HostPlatform},
    transport::{Connection, ENDPOINT_VERSION, Endpoint, Stream},
};
use anyhow::{Context as _, Result};

const ENDPOINT_TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(unix)]
const FORWARD_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a response keeps the endpoint trusted. Long enough that every
/// request of one attach reuses the proof of the one before it; short enough
/// that a request after an idle spell checks again.
const PROBE_TRUST: Duration = Duration::from_secs(10);
/// How long a shared transport keeps its SSH login after its last user.
pub const LINGER: Duration = Duration::from_secs(120);
const REAPER_INTERVAL: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);
const MAX_SSH_OUTPUT_BYTES: usize = 1024 * 1024;

// OpenSSH already runs the remote command through the account's shell. Use
// its PATH when zmux is available there: forcing an interactive login shell
// can run startup hooks that wait for a terminal even with SSH's -T option.
// Fall back to that shell when its rc file is needed to add zmux to PATH, but
// only to *find* zmux: the path it resolves is then run directly, so a heavy
// rc file costs one interactive shell per query rather than two. Startup
// files may write prompts or terminal-control sequences to stdout, so send
// only the command's output through fd 3. Keep the POSIX wrapper as one
// argument: OpenSSH joins remote command arguments into one string.
//
// Running the resolved path outside the rc file's environment is what a Mosh
// relay already does (see `resolve_remote_program`), so it has to find the
// same daemon either way.
//
// Some interactive shells abbreviate paths below the home directory as
// `~/...`, so the wrapper expands that prefix.
const RESOLVE_ZMUX: &str = r#"resolved=$(command -v zmux); if test -z "$resolved"; then resolved=$("${SHELL:-/bin/sh}" -lic "command -v zmux >&3" 3>&1 1>/dev/null); fi; case "$resolved" in "~/"*) resolved="$HOME/${resolved#\~/}";; esac"#;

/// Prints where `zmux` is, then the running daemon's endpoint: one round trip
/// for both, because every remote attach wants both.
#[cfg(any(unix, test))]
fn remote_endpoint_command() -> String {
    format!(
        r#"/bin/sh -c 'exec 3>&1 1>/dev/null; {RESOLVE_ZMUX}; test -n "$resolved" || {{ echo "zmux was not found on this host" >&2; exit 127; }}; printf "%s\n" "$resolved" >&3; exec "$resolved" endpoint --json >&3'"#
    )
}

/// Where that same shell resolves `zmux`, for when no daemon is running yet.
fn remote_program_command() -> String {
    format!(r#"/bin/sh -c 'exec 3>&1 1>/dev/null; {RESOLVE_ZMUX}; printf "%s\n" "$resolved" >&3'"#)
}

/// Runs the standalone profile discovery command without requiring a daemon.
const REMOTE_PROFILES_COMMAND: &str = r#"/bin/sh -c 'exec 3>&1 1>/dev/null; if command -v zmux >/dev/null 2>&1; then exec zmux profiles --json >&3; fi; exec "${SHELL:-/bin/sh}" -lic "exec zmux profiles --json >&3"'"#;

/// Starts the multiplexed bridge on a POSIX host.
fn remote_bridge_command(link_agent: bool) -> String {
    let arguments = if link_agent {
        "proxy-mux --forward-agent"
    } else {
        "proxy-mux"
    };
    format!(
        r#"/bin/sh -c 'exec 3>&1 1>/dev/null; if command -v zmux >/dev/null 2>&1; then exec zmux {arguments} >&3; fi; exec "${{SHELL:-/bin/sh}}" -lic "exec zmux {arguments} >&3"'"#
    )
}

/// A destination understood by OpenSSH.
///
/// `destination` is deliberately passed as one argument to `ssh`, so aliases,
/// `user@host`, IPv6 bracket forms, and the rest of OpenSSH's destination
/// syntax retain their normal meaning. Zetta does not maintain a second host
/// configuration format.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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

/// Captured stderr of a long-lived SSH process, for the error that explains
/// why it exited.
type CapturedOutput = Arc<Mutex<Vec<u8>>>;

fn capture_output(reader: impl Read + Send + 'static) -> CapturedOutput {
    let captured = CapturedOutput::default();
    let sink = captured.clone();
    thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0; 4096];
        while let Ok(count) = reader.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let remaining = MAX_SSH_OUTPUT_BYTES.saturating_sub(sink.len());
            sink.extend_from_slice(&buffer[..count.min(remaining)]);
        }
    });
    captured
}

fn captured_text(captured: &CapturedOutput) -> String {
    String::from_utf8_lossy(
        &captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
    .trim()
    .to_owned()
}

/// The one SSH login a Unix transport makes, shared by everything else.
///
/// The child and its private directory are owned by this value: the control
/// socket and every forward socket live in that directory, so dropping it
/// ends the login and removes every local path it exposed.
#[cfg(unix)]
struct Master {
    child: Child,
    directory: tempfile::TempDir,
    control_path: PathBuf,
    stderr: CapturedOutput,
}

#[cfg(unix)]
impl Master {
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[cfg(unix)]
impl Drop for Master {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
        // Keep the field explicit: TempDir removes the sockets and directory
        // after the child is gone, so no stale endpoint survives a failed SSH
        // startup or a dropped tab.
        let _ = &self.directory;
    }
}

/// One stream-local forward on the master, to the daemon it last reported.
#[cfg(unix)]
struct ForwardState {
    local_socket: PathBuf,
    /// The `-L` specification, kept so a refresh can cancel it.
    forwarding: String,
    endpoint: Endpoint,
    agent_holder: Option<Child>,
    verified_at: Option<Instant>,
}

#[cfg(unix)]
impl Drop for ForwardState {
    fn drop(&mut self) {
        if let Some(mut child) = self.agent_holder.take() {
            terminate_child(&mut child);
        }
    }
}

/// `zmux proxy-mux` on the far side: a Windows client's one SSH login, or a
/// session on a Unix client's master when the host is Windows.
struct BridgeLink {
    bridge: crate::mux_bridge::MuxBridge,
    child: Child,
    stderr: CapturedOutput,
}

impl BridgeLink {
    fn is_alive(&mut self) -> bool {
        self.bridge.is_alive() && matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for BridgeLink {
    fn drop(&mut self) {
        terminate_child(&mut self.child);
    }
}

struct RemoteState {
    // Declared before the master so a forward's agent holder is stopped before
    // the connection it runs on.
    #[cfg(unix)]
    forward: Option<ForwardState>,
    #[cfg(unix)]
    master: Option<Master>,
    link: Option<BridgeLink>,
    link_endpoint: Option<Endpoint>,
    link_verified_at: Option<Instant>,
    /// The remote host's own `zmux`, learned alongside the endpoint.
    program: Option<PathBuf>,
}

impl RemoteState {
    fn empty() -> Self {
        Self {
            #[cfg(unix)]
            forward: None,
            #[cfg(unix)]
            master: None,
            link: None,
            link_endpoint: None,
            link_verified_at: None,
            program: None,
        }
    }
}

fn is_trusted(verified_at: Option<Instant>) -> bool {
    verified_at.is_some_and(|verified| verified.elapsed() < PROBE_TRUST)
}

/// A reusable remote mux connection factory.
pub struct RemoteTransport {
    target: RemoteTarget,
    ssh_program: PathBuf,
    state: Mutex<RemoteState>,
}

static KEEP_IDLE: AtomicBool = AtomicBool::new(false);

/// Keeps shared transports for [`LINGER`] after their last user. Without this,
/// [`RemoteTransport::shared`] hands out a transport of the caller's own, which
/// is right for a command that exits as soon as it is done.
pub fn keep_idle_transports() {
    KEEP_IDLE.store(true, Ordering::Release);
}

/// Stops keeping idle transports and ends the SSH logins of those nobody is
/// using. For a process that called [`keep_idle_transports`], on its way out.
pub fn release_idle_transports() {
    KEEP_IDLE.store(false, Ordering::Release);
    let released = std::mem::take(
        &mut registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries,
    );
    drop(released);
}

/// Transports handed out by [`RemoteTransport::shared`], with when each was
/// last seen without a user.
struct Registry {
    entries: HashMap<RemoteTarget, SharedEntry>,
    reaping: bool,
}

struct SharedEntry {
    transport: Arc<RemoteTransport>,
    released_at: Option<Instant>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            entries: HashMap::new(),
            reaping: false,
        })
    })
}

/// Drops shared transports nobody has used for [`LINGER`], and exits once
/// there are none left to watch.
fn reap_shared_transports() {
    loop {
        thread::sleep(REAPER_INTERVAL);
        let mut expired = Vec::new();
        let finished = {
            let mut registry = registry()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let now = Instant::now();
            registry.entries.retain(|_, entry| {
                if Arc::strong_count(&entry.transport) > 1 {
                    entry.released_at = None;
                    return true;
                }
                let released = *entry.released_at.get_or_insert(now);
                if now.duration_since(released) < LINGER {
                    return true;
                }
                expired.push(entry.transport.clone());
                false
            });
            if registry.entries.is_empty() {
                registry.reaping = false;
            }
            !registry.reaping
        };
        // Dropping the last handle ends the SSH login, which waits for the
        // child; do it outside the registry lock.
        drop(expired);
        if finished {
            return;
        }
    }
}

/// Read a published endpoint only when a daemon still owns its socket. A
/// crashed daemon can leave the JSON file behind; treating it as live prevents
/// remote creation from starting its replacement.
pub(crate) fn live_endpoint(directory: &Path) -> Result<Endpoint> {
    let endpoint = Endpoint::read(&crate::server::endpoint_path(directory))?;
    anyhow::ensure!(
        Stream::connect(&endpoint.socket_path).is_ok(),
        "no multiplexer is running"
    );
    Ok(endpoint)
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
            state: Mutex::new(RemoteState::empty()),
        })
    }

    /// The process's transport for `target`, reused while anyone holds it and
    /// for [`LINGER`] after — when the process keeps idle transports at all
    /// ([`keep_idle_transports`]); otherwise a transport of the caller's own. Nothing is connected here; the first operation
    /// that needs the remote host logs in, and every later one reuses it.
    pub fn shared(target: RemoteTarget) -> Result<Arc<Self>> {
        target.validate()?;
        if !KEEP_IDLE.load(Ordering::Acquire) {
            return Ok(Arc::new(Self::for_creation(target)?));
        }
        let mut registry = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let transport = match registry.entries.get_mut(&target) {
            Some(entry) => {
                entry.released_at = None;
                entry.transport.clone()
            }
            None => {
                let transport = Arc::new(Self::for_creation(target.clone())?);
                registry.entries.insert(
                    target,
                    SharedEntry {
                        transport: transport.clone(),
                        released_at: None,
                    },
                );
                transport
            }
        };
        if !registry.reaping {
            registry.reaping = true;
            let spawned = thread::Builder::new()
                .name("zmux-remote-linger".to_owned())
                .spawn(reap_shared_transports);
            if spawned.is_err() {
                // Without a reaper nothing lingers: each transport lives as
                // long as its users, exactly as an unshared one would.
                registry.reaping = false;
                registry.entries.clear();
            }
        }
        Ok(transport)
    }

    pub fn target(&self) -> &RemoteTarget {
        &self.target
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, RemoteState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether this host is reached through the bridge rather than a forward:
    /// always from Windows, and from Unix once the host is known to be
    /// Windows.
    fn uses_link(&self) -> bool {
        cfg!(windows) || remote_host::learned(&self.target) == Some(HostPlatform::Windows)
    }

    /// Runs `forward` unless the host is known to need the bridge, and runs
    /// `link` instead when `forward` is what found that out.
    #[cfg(unix)]
    fn forward_or_link<T>(
        &self,
        state: &mut RemoteState,
        forward: impl FnOnce(&mut RemoteState) -> Result<T>,
        link: impl FnOnce(&mut RemoteState) -> Result<T>,
    ) -> Result<T> {
        if !self.uses_link() {
            match forward(state) {
                Err(error) if error.is::<WindowsHost>() => {}
                result => return result,
            }
        }
        link(state)
    }

    /// Returns the endpoint the daemon is reached at. Over a forward, its
    /// token and protocol come from the remote daemon and only the socket path
    /// is replaced with the private local socket.
    pub fn endpoint(&self) -> Result<Endpoint> {
        let state = self.lock_state();
        if self.uses_link() {
            return state
                .link_endpoint
                .clone()
                .context("remote SSH endpoint has not been queried");
        }
        #[cfg(unix)]
        {
            state
                .forward
                .as_ref()
                .map(|forward| forward.endpoint.clone())
                .context("remote SSH forwarding has not been established")
        }
        #[cfg(windows)]
        unreachable!("a Windows client always uses the bridge")
    }

    /// The endpoint, establishing the connection to it if nothing has yet.
    /// Unlike [`Self::refresh`] this keeps a connection that is already up,
    /// which is the point of sharing one.
    pub fn ensure_endpoint(&self) -> Result<Endpoint> {
        let mut state = self.lock_state();
        #[cfg(unix)]
        {
            self.forward_or_link(
                &mut state,
                |state| self.ensure_endpoint_locked(state),
                |state| self.install_link(state),
            )
        }
        #[cfg(windows)]
        self.install_link(&mut state)
    }

    /// Whether the host has been found to be Windows, whose shells a pane
    /// relay cannot reach yet. Known once anything has run there.
    pub fn is_windows_host(&self) -> bool {
        remote_host::learned(&self.target) == Some(HostPlatform::Windows)
    }

    /// The control socket of this transport's SSH login, while it is up.
    ///
    /// `ssh -S PATH -o ControlMaster=no HOST …` runs a command over it without
    /// logging in again. Always `None` on Windows, whose OpenSSH has no
    /// connection sharing.
    pub fn control_path(&self) -> Option<PathBuf> {
        #[cfg(unix)]
        {
            let mut state = self.lock_state();
            let master = state.master.as_mut()?;
            master.is_alive().then(|| master.control_path.clone())
        }
        #[cfg(windows)]
        {
            None
        }
    }

    /// A response arrived on a connection opened against `endpoint`, which
    /// proves its token is current: the next connection needs no probe.
    pub(crate) fn confirm(&self, endpoint: &Endpoint) {
        let mut state = self.lock_state();
        #[cfg(unix)]
        if let Some(forward) = state.forward.as_mut()
            && forward.endpoint == *endpoint
        {
            forward.verified_at = Some(Instant::now());
        }
        if state.link_endpoint.as_ref() == Some(endpoint) {
            state.link_verified_at = Some(Instant::now());
        }
    }

    /// A connection failed or presented a rejected token: probe the next one.
    pub(crate) fn distrust(&self) {
        let mut state = self.lock_state();
        #[cfg(unix)]
        if let Some(forward) = state.forward.as_mut() {
            forward.verified_at = None;
        }
        state.link_verified_at = None;
    }

    /// Opens one framed mux connection, rebuilding the SSH login once if it
    /// has gone away or no longer reaches the daemon.
    pub fn connect(&self) -> Result<(Endpoint, Stream)> {
        let mut state = self.lock_state();
        #[cfg(unix)]
        {
            self.forward_or_link(
                &mut state,
                |state| self.connect_forward(state),
                |state| self.connect_link(state),
            )
        }
        #[cfg(windows)]
        self.connect_link(&mut state)
    }

    #[cfg(unix)]
    fn connect_forward(&self, state: &mut RemoteState) -> Result<(Endpoint, Stream)> {
        for attempt in 0..2 {
            self.install_forward(state)?;
            let forward = state
                .forward
                .as_ref()
                .expect("the remote forward was just installed");
            let local_socket = forward.local_socket.clone();
            let endpoint = forward.endpoint.clone();
            let trusted = is_trusted(forward.verified_at);
            let opened = if trusted {
                Stream::connect(&local_socket)
                    .context("connecting to the SSH forward")
                    .map_err(|error| (error, "connecting to the SSH forward"))
            } else {
                self.probed_connection(&endpoint, || {
                    Stream::connect(&local_socket).context("connecting to the SSH forward")
                })
            };
            match opened {
                Ok(stream) => {
                    if !trusted && let Some(forward) = state.forward.as_mut() {
                        forward.verified_at = Some(Instant::now());
                    }
                    return Ok((endpoint, stream));
                }
                Err((error, _)) if attempt == 0 => {
                    let detail = state
                        .master
                        .as_ref()
                        .map(|master| captured_text(&master.stderr))
                        .unwrap_or_default();
                    log::debug!(
                        "remote SSH connection for {} is unusable: {error:#}; {detail}",
                        self.target.destination()
                    );
                    // A master whose TCP connection went quiet still accepts
                    // locally, so a failure here does not prove only the
                    // forward is stale. Start the login over, as a fresh SSH
                    // process always used to.
                    state.forward = None;
                    state.master = None;
                }
                Err((error, activity)) => {
                    return Err(error)
                        .with_context(|| format!("{activity} for {}", self.target.destination()));
                }
            }
        }
        anyhow::bail!(
            "could not connect to the SSH forward for {}",
            self.target.destination()
        )
    }

    fn connect_link(&self, state: &mut RemoteState) -> Result<(Endpoint, Stream)> {
        for attempt in 0..2 {
            let endpoint = self.install_link(state)?;
            let trusted = is_trusted(state.link_verified_at);
            let (opened, stderr) = {
                let link = state.link.as_ref().expect("the bridge was just installed");
                let opened = if trusted {
                    link.bridge
                        .open()
                        .map_err(|error| (error, "opening a stream over the zmux bridge"))
                } else {
                    self.probed_connection(&endpoint, || link.bridge.open())
                };
                (opened, link.stderr.clone())
            };
            match opened {
                Ok(stream) => {
                    if !trusted {
                        state.link_verified_at = Some(Instant::now());
                    }
                    return Ok((endpoint, stream));
                }
                Err((error, _)) if attempt == 0 => {
                    log::debug!(
                        "remote zmux bridge for {} is unusable: {error:#}; {}",
                        self.target.destination(),
                        captured_text(&stderr)
                    );
                    state.link = None;
                    state.link_endpoint = None;
                    state.link_verified_at = None;
                }
                Err((error, activity)) => {
                    let detail = ssh_detail(&captured_text(&stderr));
                    return Err(error).with_context(|| {
                        format!("{activity} for {}{detail}", self.target.destination())
                    });
                }
            }
        }
        anyhow::bail!(
            "could not connect to the zmux bridge for {}",
            self.target.destination()
        )
    }

    /// Probes the daemon on one connection, then opens the one the caller
    /// will use. Failures say which of the two steps failed.
    fn probed_connection(
        &self,
        endpoint: &Endpoint,
        open: impl Fn() -> Result<Stream>,
    ) -> std::result::Result<Stream, (anyhow::Error, &'static str)> {
        let probe = open().map_err(|error| (error, "opening a probe connection"))?;
        self.probe(endpoint, probe)
            .map_err(|error| (error, "checking the remote multiplexer"))?;
        open().map_err(|error| (error, "opening a request connection"))
    }

    /// Confirms that the transport reaches the daemon described by its cached
    /// endpoint. A listener can outlive a daemon replacement, so checking only
    /// the local socket is insufficient: the next request could carry a stale
    /// token.
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

    /// Re-queries the remote endpoint and replaces the forward. This is used
    /// after an invalid token or a remote daemon replacement, where both the
    /// socket path and token may have changed. The SSH login itself is kept.
    pub fn refresh(&self) -> Result<Endpoint> {
        let mut state = self.lock_state();
        #[cfg(unix)]
        {
            self.forward_or_link(
                &mut state,
                |state| self.refresh_forward(state),
                |state| self.refresh_link(state),
            )
        }
        #[cfg(windows)]
        self.refresh_link(&mut state)
    }

    #[cfg(unix)]
    fn refresh_forward(&self, state: &mut RemoteState) -> Result<Endpoint> {
        if let Some(forward) = state.forward.take()
            && let Some(master) = state.master.as_mut()
            && master.is_alive()
        {
            let arguments = forward_request_arguments(
                &self.target,
                &master.control_path,
                "cancel",
                &forward.forwarding,
            );
            if let Err(error) = run_capture(&self.ssh_program, &arguments, FORWARD_TIMEOUT) {
                log::debug!(
                    "could not cancel the previous forward for {}: {error:#}",
                    self.target.destination()
                );
            }
        }
        self.ensure_endpoint_locked(state)
    }

    #[cfg(unix)]
    fn ensure_endpoint_locked(&self, state: &mut RemoteState) -> Result<Endpoint> {
        self.install_forward(state)?;
        Ok(state
            .forward
            .as_ref()
            .expect("the forward was just installed")
            .endpoint
            .clone())
    }

    fn refresh_link(&self, state: &mut RemoteState) -> Result<Endpoint> {
        state.link_endpoint = None;
        state.link_verified_at = None;
        self.install_link(state)
    }

    /// Makes sure the SSH login is up, returning its control socket.
    #[cfg(unix)]
    fn ensure_master(&self, state: &mut RemoteState) -> Result<PathBuf> {
        if let Some(master) = state.master.as_mut() {
            if master.is_alive() {
                return Ok(master.control_path.clone());
            }
            log::debug!(
                "the SSH login for {} ended: {}",
                self.target.destination(),
                captured_text(&master.stderr)
            );
            state.forward = None;
            state.master = None;
        }
        let master = self.start_master()?;
        let control_path = master.control_path.clone();
        state.master = Some(master);
        Ok(control_path)
    }

    #[cfg(unix)]
    fn start_master(&self) -> Result<Master> {
        let directory = tempfile::Builder::new()
            .prefix("zetta-zmux-")
            .tempdir()
            .context("creating the private SSH control directory")?;
        restrict_directory(directory.path())?;
        let control_path = directory.path().join("ctl");
        let mut child = Command::new(&self.ssh_program)
            .args(master_arguments(&self.target, &control_path))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting SSH for {}", self.target.destination))?;
        let stderr = capture_output(child.stderr.take().context("SSH stderr was not captured")?);

        // OpenSSH listens on a temporary name and links it into place, so the
        // control path existing means the login is done and the socket is
        // accepting.
        let deadline = Instant::now() + ENDPOINT_TIMEOUT;
        loop {
            if let Some(status) = child.try_wait().context("checking SSH status")? {
                // Give the reader a moment to collect what SSH said last.
                thread::sleep(POLL_INTERVAL);
                anyhow::bail!(
                    "SSH exited with {status} while logging in to {}{}",
                    self.target.destination,
                    ssh_detail(&captured_text(&stderr))
                );
            }
            if control_path.exists() {
                return Ok(Master {
                    child,
                    directory,
                    control_path,
                    stderr,
                });
            }
            if Instant::now() >= deadline {
                terminate_child(&mut child);
                anyhow::bail!(
                    "SSH did not finish logging in to {} within {ENDPOINT_TIMEOUT:?}{}",
                    self.target.destination,
                    ssh_detail(&captured_text(&stderr))
                );
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// Makes sure a forward to the daemon exists on a live login.
    #[cfg(unix)]
    fn install_forward(&self, state: &mut RemoteState) -> Result<()> {
        if state
            .master
            .as_mut()
            .is_none_or(|master| !master.is_alive())
        {
            state.forward = None;
        }
        if state.forward.is_none() {
            let forward = self.start_forward(state)?;
            state.forward = Some(forward);
        }
        Ok(())
    }

    #[cfg(unix)]
    fn start_forward(&self, state: &mut RemoteState) -> Result<ForwardState> {
        static FORWARDS: AtomicU64 = AtomicU64::new(0);

        let control_path = self.ensure_master(state)?;
        let directory = state
            .master
            .as_ref()
            .expect("the login was just established")
            .directory
            .path()
            .to_owned();
        let (program, remote_endpoint) = self.query_endpoint(Some(&control_path))?;
        state.program = Some(program);
        let local_socket = directory.join(format!(
            "mux-{}.sock",
            FORWARDS.fetch_add(1, Ordering::Relaxed)
        ));
        let forwarding = format!(
            "{}:{}",
            local_socket.display(),
            remote_endpoint.socket_path.display()
        );
        run_capture(
            &self.ssh_program,
            &forward_request_arguments(&self.target, &control_path, "forward", &forwarding),
            FORWARD_TIMEOUT,
        )
        .with_context(|| {
            format!(
                "adding the stream-local forward for {}",
                self.target.destination
            )
        })?;
        anyhow::ensure!(
            local_socket.exists(),
            "SSH accepted the stream-local forward for {} but did not create it",
            self.target.destination
        );
        let agent_holder = match (
            self.target.forward_agent,
            remote_endpoint.socket_path.parent(),
        ) {
            (Some(true), Some(remote_directory)) => {
                let socket = remote_directory.join("forwarded-agent.sock");
                let child = Command::new(&self.ssh_program)
                    .args(agent_holder_arguments(&self.target, &control_path, &socket))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .context("starting SSH agent forwarding")?;
                Some(child)
            }
            _ => None,
        };
        Ok(ForwardState {
            endpoint: Endpoint {
                version: remote_endpoint.version,
                protocol_version: remote_endpoint.protocol_version,
                process_id: remote_endpoint.process_id,
                socket_path: local_socket.clone(),
                token: remote_endpoint.token,
            },
            local_socket,
            forwarding,
            agent_holder,
            verified_at: None,
        })
    }

    /// Makes sure the bridge is up and knows the daemon's endpoint.
    ///
    /// A bridge started while no daemon was running reports no endpoint; it
    /// is asked again over the same link rather than replaced, so waiting
    /// for a daemon to start costs a round trip per poll, not a login.
    fn install_link(&self, state: &mut RemoteState) -> Result<Endpoint> {
        if state.link.as_mut().is_some_and(|link| !link.is_alive()) {
            if let Some(link) = state.link.as_ref() {
                log::debug!(
                    "the zmux bridge for {} ended: {}",
                    self.target.destination(),
                    captured_text(&link.stderr)
                );
            }
            state.link = None;
            state.link_endpoint = None;
            state.link_verified_at = None;
        }
        if state.link.is_none() {
            let link = self.start_link(state)?;
            let info = link.bridge.initial_info().clone();
            state.link = Some(link);
            self.apply_bridge_info(state, info)?;
        }
        if let Some(endpoint) = state.link_endpoint.clone() {
            return Ok(endpoint);
        }
        let info = state
            .link
            .as_ref()
            .expect("the bridge was just installed")
            .bridge
            .query_info(ENDPOINT_TIMEOUT)?;
        self.apply_bridge_info(state, info)
    }

    fn apply_bridge_info(
        &self,
        state: &mut RemoteState,
        info: crate::mux_bridge::BridgeInfo,
    ) -> Result<Endpoint> {
        if let Some(program) = info.program {
            state.program = Some(program);
        }
        let endpoint = info.endpoint.with_context(|| {
            info.error
                .unwrap_or_else(|| "the remote host reported no multiplexer".to_owned())
        })?;
        validate_endpoint(&endpoint)?;
        state.link_endpoint = Some(endpoint.clone());
        state.link_verified_at = None;
        Ok(endpoint)
    }

    /// Starts the bridge in the host's dialect. A Windows client may not know
    /// it yet: the POSIX bridge failing is what finds out.
    fn start_link(&self, state: &mut RemoteState) -> Result<BridgeLink> {
        #[cfg(unix)]
        let control = Some(self.ensure_master(state)?);
        #[cfg(windows)]
        let control: Option<PathBuf> = {
            let _ = state;
            None
        };
        let known = remote_host::learned(&self.target);
        let platform = known.unwrap_or(HostPlatform::Posix);
        match self.spawn_link(control.as_deref(), platform) {
            Ok(link) => {
                remote_host::learn(&self.target, platform);
                Ok(link)
            }
            Err(error) if known.is_none() => {
                let error = self.explain_failure(control.as_deref(), error);
                if !error.is::<WindowsHost>() {
                    return Err(error);
                }
                self.spawn_link(control.as_deref(), HostPlatform::Windows)
            }
            Err(error) => Err(error),
        }
    }

    fn spawn_link(&self, control: Option<&Path>, platform: HostPlatform) -> Result<BridgeLink> {
        let mut child = Command::new(&self.ssh_program)
            .args(bridge_arguments(&self.target, control, platform))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting SSH for {}", self.target.destination))?;
        let (Some(input), Some(output), Some(errors)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            terminate_child(&mut child);
            anyhow::bail!("SSH did not expose its standard streams");
        };
        let stderr = capture_output(errors);
        match crate::mux_bridge::MuxBridge::connect(output, input, ENDPOINT_TIMEOUT) {
            Ok(bridge) => Ok(BridgeLink {
                bridge,
                child,
                stderr,
            }),
            Err(error) => {
                terminate_child(&mut child);
                Err(error).with_context(|| {
                    format!(
                        "starting the zmux bridge on {}{}",
                        self.target.destination,
                        ssh_detail(&captured_text(&stderr))
                    )
                })
            }
        }
    }

    /// The control socket commands should run over, logging in if nothing
    /// has yet. `None` on Windows, where each command is its own login.
    fn command_control(&self) -> Result<Option<PathBuf>> {
        #[cfg(unix)]
        {
            let mut state = self.lock_state();
            self.ensure_master(&mut state).map(Some)
        }
        #[cfg(windows)]
        {
            Ok(None)
        }
    }

    /// A POSIX command failed on a host whose dialect is not known yet: finds
    /// out whether that is because the host is Windows, which is the one
    /// failure a retry in the other dialect can fix. Returns [`WindowsHost`]
    /// if so, and the original error otherwise.
    fn explain_failure(&self, control: Option<&Path>, error: anyhow::Error) -> anyhow::Error {
        if remote_host::learned(&self.target).is_some() {
            return error;
        }
        let arguments = command_arguments(
            &self.target,
            control,
            remote_host::PLATFORM_PROBE.to_owned(),
        );
        match run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT) {
            Ok(output) => {
                let platform = remote_host::classify_probe(&output.stdout);
                remote_host::learn(&self.target, platform);
                match platform {
                    HostPlatform::Windows => {
                        log::debug!(
                            "{} is a Windows host; the POSIX command failed: {error:#}",
                            self.target.destination()
                        );
                        anyhow::Error::new(WindowsHost)
                    }
                    HostPlatform::Posix => error,
                }
            }
            Err(probe) => {
                log::debug!(
                    "could not tell which shell {} runs: {probe:#}",
                    self.target.destination()
                );
                error
            }
        }
    }

    /// Runs a one-shot command in the host's dialect, finding the dialect out
    /// if the POSIX form fails on a host not seen before.
    fn run_host_command(
        &self,
        control: Option<&Path>,
        arguments: impl Fn(HostPlatform) -> Vec<String>,
    ) -> Result<(HostPlatform, SshOutput)> {
        let known = remote_host::learned(&self.target);
        let platform = known.unwrap_or(HostPlatform::Posix);
        match run_capture(&self.ssh_program, &arguments(platform), ENDPOINT_TIMEOUT) {
            Ok(output) => {
                remote_host::learn(&self.target, platform);
                Ok((platform, output))
            }
            Err(error) if known.is_none() => {
                let error = self.explain_failure(control, error);
                if !error.is::<WindowsHost>() {
                    return Err(error);
                }
                let windows = arguments(HostPlatform::Windows);
                let output = run_capture(&self.ssh_program, &windows, ENDPOINT_TIMEOUT)?;
                Ok((HostPlatform::Windows, output))
            }
            Err(error) => Err(error),
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
    /// one. It is learned with the endpoint and kept, so this usually costs
    /// nothing.
    pub fn resolve_remote_program(&self) -> Result<PathBuf> {
        if let Some(program) = self.lock_state().program.clone() {
            return Ok(program);
        }
        let control = self.command_control()?;
        let (platform, output) = self.run_host_command(control.as_deref(), |platform| {
            program_arguments(&self.target, control.as_deref(), platform)
        })?;
        let program = parse_remote_program_path(&output.stdout, platform)?;
        self.lock_state().program = Some(program.clone());
        Ok(program)
    }

    /// Returns the names the remote host can resolve through its standalone
    /// `zmux` profile boundary. This intentionally does not start a daemon.
    pub fn query_profiles(&self) -> Result<Vec<String>> {
        let control = self.command_control()?;
        let (_, output) = self.run_host_command(control.as_deref(), |platform| {
            profiles_arguments(&self.target, control.as_deref(), platform)
        })?;
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
        // The endpoint file can outlive a crashed daemon. Probe the daemon
        // itself before deciding that a new one must not be started.
        if let Ok((endpoint, _)) = self.connect() {
            return Ok(endpoint);
        }
        let program = self.resolve_remote_program()?;
        let control = self.command_control()?;
        self.run_host_command(control.as_deref(), |platform| {
            start_daemon_arguments(&self.target, control.as_deref(), &program, platform)
        })
        .context("starting the remote zmux daemon")?;
        let deadline = Instant::now() + ENDPOINT_TIMEOUT;
        loop {
            match self.connect() {
                Ok((endpoint, _)) => return Ok(endpoint),
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

    /// Asks the remote host where `zmux` is and which daemon it is running.
    /// Fails with [`WindowsHost`] when the host turns out to need the bridge.
    #[cfg(unix)]
    fn query_endpoint(&self, control: Option<&Path>) -> Result<(PathBuf, Endpoint)> {
        let arguments = endpoint_arguments(&self.target, control);
        match run_capture(&self.ssh_program, &arguments, ENDPOINT_TIMEOUT) {
            Ok(output) => {
                remote_host::learn(&self.target, HostPlatform::Posix);
                parse_endpoint_output(&output.stdout)
            }
            Err(error) => Err(self.explain_failure(control, error)),
        }
    }
}

/// The host is Windows, which a forward cannot reach: use the bridge.
#[derive(Debug)]
struct WindowsHost;

impl std::fmt::Display for WindowsHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the remote host is Windows, which is reached through the zmux bridge")
    }
}

impl std::error::Error for WindowsHost {}

/// Splits the endpoint query's output: the resolved program, then the JSON.
#[cfg(any(unix, test))]
fn parse_endpoint_output(output: &[u8]) -> Result<(PathBuf, Endpoint)> {
    let text = std::str::from_utf8(output).context("remote endpoint output was not UTF-8")?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let program = parse_remote_program_path(
        lines.next().unwrap_or_default().as_bytes(),
        HostPlatform::Posix,
    )?;
    let json = lines.next().unwrap_or_default();
    anyhow::ensure!(!json.is_empty(), "remote zmux endpoint returned no JSON");
    let endpoint: Endpoint =
        serde_json::from_str(json).context("parsing remote `zmux endpoint --json` output")?;
    validate_endpoint(&endpoint)?;
    Ok((program, endpoint))
}

fn validate_endpoint(endpoint: &Endpoint) -> Result<()> {
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
    Ok(())
}

fn ssh_detail(stderr: &str) -> String {
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

/// The far side of the bridge: `zmux proxy-mux`.
///
/// Each stream the client opens becomes a fresh connection to the daemon
/// published in this host's session catalog, found again every time so a
/// daemon replaced while the link is up is still reached. With `link_agent`,
/// the agent this SSH session forwarded is linked where daemon-owned shells
/// look for it, for as long as the link is what is serving them — the job a
/// separate holder session used to do.
pub fn run_mux_proxy(link_agent: bool) -> Result<()> {
    use std::io;

    let directory = crate::paths::session_catalog_dir();
    let program = std::env::current_exe().ok();
    let connect_directory = directory.clone();
    let connect = move || {
        let endpoint = Endpoint::read(&crate::server::endpoint_path(&connect_directory))
            .map_err(io::Error::other)?;
        Stream::connect(&endpoint.socket_path)
    };
    let info = move || {
        let endpoint = live_endpoint(&directory);
        if link_agent && endpoint.is_ok() {
            link_forwarded_agent();
        }
        crate::mux_bridge::BridgeInfo {
            program: program.clone(),
            error: endpoint.as_ref().err().map(|error| format!("{error:#}")),
            endpoint: endpoint.ok(),
        }
    };
    crate::mux_bridge::serve(io::stdin(), io::stdout(), connect, info)
}

/// A Windows host's shells have no forwarded-agent name to find: agent
/// forwarding into daemon-owned shells is Unix-only for now.
#[cfg(windows)]
fn link_forwarded_agent() {
    log::debug!("forwarded agents are not linked for a Windows host's shells");
}

/// Points the stable forwarded-agent name at this session's agent, replacing
/// whatever it pointed at before in one rename.
#[cfg(unix)]
fn link_forwarded_agent() {
    let Some(agent) = std::env::var_os("SSH_AUTH_SOCK") else {
        return;
    };
    let link = crate::paths::forwarded_agent_socket();
    let temporary = link.with_extension(format!("sock.{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let linked = std::os::unix::fs::symlink(&agent, &temporary)
        .and_then(|()| std::fs::rename(&temporary, &link));
    if let Err(error) = linked {
        let _ = std::fs::remove_file(&temporary);
        log::debug!("could not link the forwarded agent: {error}");
    }
}

struct SshOutput {
    stdout: Vec<u8>,
}

/// A command on the remote host, over `control` when there is one.
///
/// A session on a shared connection also drops the forwards the user's
/// configuration would add: the login that owns them already has them, and a
/// second request for the same local port would only fail.
fn command_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    remote_command: String,
) -> Vec<String> {
    let mut arguments = vec!["-T".to_owned()];
    if let Some(control) = control {
        arguments.extend([
            "-S".to_owned(),
            control.display().to_string(),
            "-o".to_owned(),
            "ControlMaster=no".to_owned(),
            "-o".to_owned(),
            "ClearAllForwardings=yes".to_owned(),
        ]);
    }
    push_target_options(&mut arguments, target);
    arguments.extend([target.destination.clone(), remote_command]);
    arguments
}

#[cfg(any(unix, test))]
fn endpoint_arguments(target: &RemoteTarget, control: Option<&Path>) -> Vec<String> {
    command_arguments(target, control, remote_endpoint_command())
}

fn program_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let command = match platform {
        HostPlatform::Posix => remote_program_command(),
        HostPlatform::Windows => remote_host::program_command(),
    };
    command_arguments(target, control, command)
}

fn profiles_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let command = match platform {
        HostPlatform::Posix => REMOTE_PROFILES_COMMAND.to_owned(),
        HostPlatform::Windows => remote_host::profiles_command(),
    };
    command_arguments(target, control, command)
}

fn start_daemon_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    program: &Path,
    platform: HostPlatform,
) -> Vec<String> {
    if platform == HostPlatform::Windows {
        return command_arguments(target, control, remote_host::start_daemon_command(program));
    }
    let program = shell_escape_double_quoted(&program.to_string_lossy());
    let command = format!(
        r#"/bin/sh -c 'if command -v zmux >/dev/null 2>&1; then nohup "{program}" --daemon >/dev/null 2>&1 </dev/null & exit; fi; exec "${{SHELL:-/bin/sh}}" -lic "nohup \"{program}\" --daemon >/dev/null 2>&1 </dev/null &"'"#
    );
    command_arguments(target, control, command)
}

/// The login every other Unix command shares. It runs nothing itself (`-N`);
/// `ControlPersist=no` keeps it in the foreground as this process's child, so
/// it ends when the transport does rather than lingering on its own.
#[cfg(any(unix, test))]
fn master_arguments(target: &RemoteTarget, control_path: &Path) -> Vec<String> {
    let mut arguments = vec![
        "-T".to_owned(),
        "-N".to_owned(),
        "-M".to_owned(),
        "-S".to_owned(),
        control_path.display().to_string(),
        "-o".to_owned(),
        "ControlPersist=no".to_owned(),
        "-o".to_owned(),
        "ExitOnForwardFailure=yes".to_owned(),
    ];
    push_target_options(&mut arguments, target);
    arguments.push(target.destination.clone());
    arguments
}

/// Adds (`forward`) or removes (`cancel`) a stream-local forward on the
/// master. The master does the work locally; nothing crosses the network.
#[cfg(any(unix, test))]
fn forward_request_arguments(
    target: &RemoteTarget,
    control_path: &Path,
    operation: &str,
    forwarding: &str,
) -> Vec<String> {
    vec![
        "-S".to_owned(),
        control_path.display().to_string(),
        "-O".to_owned(),
        operation.to_owned(),
        "-L".to_owned(),
        forwarding.to_owned(),
        target.destination.clone(),
    ]
}

/// A session that keeps the forwarded agent linked where daemon-owned shells
/// look for it. On a shared connection this asks the master to forward the
/// agent for this session, which it can because it was started with `-A`.
#[cfg(any(unix, test))]
fn agent_holder_arguments(
    target: &RemoteTarget,
    control_path: &Path,
    socket: &Path,
) -> Vec<String> {
    command_arguments(target, Some(control_path), agent_holder_command(socket))
}

/// The multiplexed bridge: a Windows client's one login, or a session on a
/// Unix client's master.
fn bridge_arguments(
    target: &RemoteTarget,
    control: Option<&Path>,
    platform: HostPlatform,
) -> Vec<String> {
    let link_agent = target.forward_agent == Some(true);
    let command = match platform {
        HostPlatform::Posix => remote_bridge_command(link_agent),
        HostPlatform::Windows => remote_host::bridge_command(link_agent),
    };
    let mut arguments = command_arguments(target, control, command);
    if control.is_none() {
        // A login of its own drops the configured forwards too: the bridge is
        // the only thing it carries.
        arguments.splice(
            1..1,
            ["-o".to_owned(), "ClearAllForwardings=yes".to_owned()],
        );
    }
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
fn agent_holder_command(socket: &Path) -> String {
    let socket = shell_quote(&socket.to_string_lossy());
    let script = format!(
        "umask 077; link={socket}; temporary=\"$link.$$\"; if test -n \"$SSH_AUTH_SOCK\"; then rm -f \"$temporary\"; ln -s \"$SSH_AUTH_SOCK\" \"$temporary\" && mv -f \"$temporary\" \"$link\"; fi; exec sleep 2147483647"
    );
    format!("/bin/sh -c {}", shell_quote(&script))
}

#[cfg(any(unix, test))]
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
/// connection attempt must therefore explicitly reap SSH, otherwise repeated
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

fn parse_remote_program_path(output: &[u8], platform: HostPlatform) -> Result<PathBuf> {
    let text = std::str::from_utf8(output)
        .context("remote zmux path was not UTF-8")?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    anyhow::ensure!(
        !text.is_empty(),
        "the remote host has no zmux on the path its shell resolves"
    );
    if platform == HostPlatform::Windows {
        return remote_host::parse_program_path(text);
    }
    // This path belongs to the remote POSIX host. Windows Path::is_absolute
    // rejects /home/... even though it is absolute for the host running zmux.
    anyhow::ensure!(
        text.starts_with('/'),
        "the remote host resolved zmux to {text}, which is not an absolute path"
    );
    Ok(PathBuf::from(text))
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
                return Err(error).context("checking SSH command status");
            }
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            let _ = stdout_thread.join();
            let stderr = stderr_thread
                .join()
                .map_err(|_| anyhow::anyhow!("SSH stderr reader panicked"))??;
            let detail = String::from_utf8_lossy(&stderr);
            anyhow::bail!(
                "SSH command timed out after {timeout:?}{}",
                ssh_detail(detail.trim())
            );
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
        "SSH command failed with {status}: {}",
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
        "SSH output exceeded {MAX_SSH_OUTPUT_BYTES} bytes"
    );
    Ok(bytes)
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("restricting SSH control directory {}", path.display()))
}

#[cfg(test)]
#[path = "tests/remote.rs"]
mod tests;
