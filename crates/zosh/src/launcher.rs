//! The complete Mosh launcher: SSH bootstrap, endpoint selection, and the
//! bundled terminal client.
//!
//! Keeping this in `zosh` is intentional. `zosh` is the user-facing Mosh
//! command, while `zetta mosh` is only a compatibility proxy that forwards
//! its original argument vector to this executable.

use std::{
    env,
    ffi::OsString,
    io::{self, BufRead as _, BufReader, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};
use mosh_rs::{Base64Key, DisplayPreference, sender::KEEP_ALIVE_DEFAULT_MS};

use crate::{
    agent::{BootstrapAgentRelay, environment_agent_path, normalize_agent_path},
    client::{self, SessionSettings},
    terminal,
};

mod remote_platform;

mod args;
pub use args::*;
mod bootstrap;
use bootstrap::*;
mod commands;
use commands::*;
mod proxy;
use proxy::*;

const DEFAULT_SERVER: &str = "zosh-server";
const STOCK_SERVER: &str = "mosh-server";
const DEFAULT_SSH: &str = "ssh";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PredictionMode {
    #[default]
    Adaptive,
    Always,
    Never,
    Experimental,
}

impl PredictionMode {
    fn as_env(self) -> &'static str {
        match self {
            Self::Adaptive => "adaptive",
            Self::Always => "always",
            Self::Never => "never",
            Self::Experimental => "experimental",
        }
    }

    fn display_preference(self) -> DisplayPreference {
        match self {
            Self::Adaptive => DisplayPreference::Adaptive,
            Self::Always => DisplayPreference::Always,
            Self::Never => DisplayPreference::Never,
            Self::Experimental => DisplayPreference::Experimental,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum AddressFamily {
    #[default]
    PreferInet,
    Inet,
    Inet6,
    Auto,
    All,
    PreferInet6,
}

impl AddressFamily {
    fn ssh_flag(self) -> Option<&'static str> {
        match self {
            Self::PreferInet | Self::Auto | Self::All | Self::PreferInet6 => None,
            Self::Inet => Some("-4"),
            Self::Inet6 => Some("-6"),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum BindServer {
    #[default]
    Ssh,
    Any,
    Address(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RemoteIpMode {
    Local,
    Remote,
    #[default]
    Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortRequest {
    pub(crate) start: u16,
    pub(crate) end: Option<u16>,
}

impl PortRequest {
    pub(crate) fn as_argument(&self) -> String {
        match self.end {
            Some(end) => format!("{}:{}", self.start, end),
            None => self.start.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProxyRequest {
    host: String,
    port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MoshCommand {
    client: Option<String>,
    server: String,
    server_explicit: bool,
    prediction: PredictionMode,
    prediction_explicit: bool,
    predict_overwrite: bool,
    predict_overwrite_explicit: bool,
    /// `-k`/`--keep-alive`: how long the session may go without sending
    /// before it emits a keep-alive, or `None` for Mosh's own heartbeat.
    keep_alive: Option<u64>,
    /// `-s`/`--scrollback`: KiB of scrolled-off history to ask the server to
    /// carry, or zero for none. See `PROTOCOL.md`.
    scrollback_kib: u32,
    /// Opt in to the authenticated Zosh SSH-agent forwarding extension.
    forward_agent: bool,
    /// An embedder already knows the host's platform, so no SSH probe is spent
    /// on every pane of a shared session: see [`Self::windows_host`].
    embedded: bool,
    /// For an embedded command, the host is Windows: bootstrap its server
    /// through PowerShell rather than a POSIX shell.
    windows_host: bool,
    family: AddressFamily,
    port: Option<PortRequest>,
    bind_server: BindServer,
    ssh: Vec<String>,
    ssh_pty: bool,
    init: bool,
    local: bool,
    remote_ip: RemoteIpMode,
    init_explicit: bool,
    target: Option<String>,
    remote_command: Vec<String>,
    /// The executable OpenSSH is pointed at for `--experimental-remote-ip=proxy`.
    /// `None` means this process, which is what the `zosh` command wants; an
    /// embedder is some other program and has to name the bundled `zosh`.
    proxy_program: Option<PathBuf>,
    /// An OpenSSH control socket already logged in to the target. The
    /// bootstrap then runs as a session on that connection instead of
    /// logging in again; see [`PaneBootstrapRequest::control_path`].
    control_path: Option<PathBuf>,
    /// How long the remote server may go without hearing from its client
    /// before it ends the session; see
    /// [`PaneBootstrapRequest::server_network_timeout`].
    server_network_timeout: Option<Duration>,
    original_arguments: Vec<std::ffi::OsString>,
    help: bool,
    version: bool,
    proxy: Option<ProxyRequest>,
}

impl Default for MoshCommand {
    fn default() -> Self {
        Self {
            client: None,
            server: DEFAULT_SERVER.to_owned(),
            server_explicit: false,
            prediction: PredictionMode::default(),
            prediction_explicit: false,
            predict_overwrite: false,
            predict_overwrite_explicit: false,
            keep_alive: None,
            scrollback_kib: client::SCROLLBACK_DEFAULT_KIB,
            forward_agent: false,
            embedded: false,
            windows_host: false,
            family: AddressFamily::default(),
            port: None,
            bind_server: BindServer::default(),
            ssh: vec![DEFAULT_SSH.to_owned()],
            ssh_pty: true,
            // Zosh deliberately stays on the user's normal screen by
            // default. Pass --init to opt into stock Mosh's alternate screen.
            init: false,
            local: false,
            remote_ip: RemoteIpMode::default(),
            init_explicit: false,
            target: None,
            remote_command: Vec::new(),
            proxy_program: None,
            control_path: None,
            server_network_timeout: None,
            original_arguments: Vec::new(),
            help: false,
            version: false,
            proxy: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BootstrapEndpoint {
    pub(crate) port: u16,
    pub(crate) key: String,
    pub(crate) ip: Option<String>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) agent_binding: Option<Vec<u8>>,
    pub(crate) agent_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BootstrapResult {
    Endpoint(BootstrapEndpoint),
    UnsupportedServer { output: String, status: Option<i32> },
}

#[derive(Debug)]
struct BootstrapOutput {
    stdout: String,
    stderr: String,
    status: ExitStatus,
}

impl BootstrapOutput {
    fn combined(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// What an embedder asks a host for: a Mosh endpoint in front of one command.
///
/// This is the launcher's bootstrap half without its command line. Everything
/// it does not name is what `zosh` itself would do — the same SSH invocation,
/// the same `zosh-server`-or-`mosh-server` probe, and the same address
/// discovery, so an alias, a `ProxyJump` or an agent in `~/.ssh/config`
/// behaves for a pane exactly as it does for the command.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneBootstrapRequest {
    /// An OpenSSH destination, passed to `ssh` as one argument.
    pub target: String,
    /// An SSH port, when the destination's configuration does not supply one.
    pub ssh_port: Option<u16>,
    /// What the remote server runs instead of a login shell.
    pub remote_command: Vec<String>,
    /// The keep-alive interval in milliseconds, or `None` for Mosh's own
    /// three-second heartbeat.
    pub keep_alive: Option<u64>,
    /// Request SSH-agent forwarding for this embedded Zosh endpoint.
    pub forward_agent: bool,
    /// The `zosh` executable OpenSSH should use to discover the server's
    /// address. An embedder is not `zosh`, so it has to say where that is;
    /// without it the address discovery falls back to resolving the
    /// destination locally, which an alias may not answer.
    pub proxy_program: Option<PathBuf>,
    /// An OpenSSH control socket the embedder already holds for `target`.
    ///
    /// With one, the bootstrap is a session on that login — a couple of round
    /// trips rather than a whole new login per pane. OpenSSH does not run a
    /// `ProxyCommand` for such a session, so the server's address is instead
    /// what `ssh -G` says the login connects to, resolved here, which is what
    /// the proxy would have reported. Where that cannot be known — the target
    /// is reached through a configured `ProxyCommand` or `ProxyJump` — or
    /// where agent forwarding needs a login of its own to capture its
    /// binding, the socket is not used, and a failed shared bootstrap is tried
    /// again with a login of its own.
    pub control_path: Option<PathBuf>,
    /// How long the remote server may go without hearing from its client
    /// before it ends the session (`MOSH_SERVER_NETWORK_TMOUT`, which both
    /// `zosh-server` and `mosh-server` honour). `None` is Mosh's own
    /// behaviour: wait for the client for ever.
    ///
    /// For the standalone command that is right — the client is a person's
    /// terminal, and roaming back to it after any absence is the point. An
    /// embedder whose pane lives on elsewhere is different: a client that went
    /// without a shutdown (a crash, a lost machine) leaves a server, and
    /// whatever it runs, waiting for good.
    pub server_network_timeout: Option<Duration>,
    /// The host is Windows, whose account shell is PowerShell or cmd, so its
    /// server is started with the Windows bootstrap. An embedder learns this
    /// from the connection it already has, which spares every pane the probe
    /// the standalone command makes.
    pub windows_host: bool,
}

/// A Mosh endpoint, ready for [`crate::PaneSession::connect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneEndpoint {
    pub host: String,
    pub port: u16,
    pub key: String,
    /// The native SSH forwarding binding captured while the endpoint was
    /// bootstrapped, when agent forwarding was requested and available.
    pub agent_binding: Option<Vec<u8>>,
    /// The local agent selected by this destination's `IdentityAgent` setting.
    pub agent_path: Option<PathBuf>,
    /// Anything the remote side printed alongside the endpoint. `zosh` writes
    /// these to its stderr; an embedder has somewhere else to put them.
    pub diagnostics: Vec<String>,
}

/// What a host answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneBootstrapOutcome {
    Endpoint(PaneEndpoint),
    /// The host has no usable Mosh server. `zosh` falls back to plain SSH
    /// here; an embedder is expected to fall back to whatever carried the
    /// same content before, rather than to fail.
    UnsupportedServer {
        output: String,
        status: Option<i32>,
    },
}

/// Bootstraps a Mosh endpoint for one remote command.
///
/// Blocking: this starts SSH and waits for the remote server's `MOSH CONNECT`
/// line, so it belongs on a background thread, never on a thread that draws.
pub fn bootstrap_pane_endpoint(request: &PaneBootstrapRequest) -> Result<PaneBootstrapOutcome> {
    let command = embedded_command(request)?;
    let mut shared_failure = None;
    if let Some(shared) = shared_connection_command(&command, request) {
        match bootstrap_embedded(&shared.command, &request.target, Some(&shared.host)) {
            Ok(outcome) => return Ok(outcome),
            Err(error) => shared_failure = Some(error),
        }
    }
    let mut outcome = bootstrap_embedded(&command, &request.target, None)?;
    if let (Some(error), PaneBootstrapOutcome::Endpoint(endpoint)) = (shared_failure, &mut outcome)
    {
        endpoint.diagnostics.insert(
            0,
            format!("the shared SSH connection could not start this pane, so it logged in again: {error:#}"),
        );
    }
    Ok(outcome)
}

/// A bootstrap command that runs over the embedder's SSH login, and the
/// address its Mosh server will be reachable at.
struct SharedConnectionCommand {
    command: MoshCommand,
    host: String,
}

/// Whether this request can run over its control socket, and how.
fn shared_connection_command(
    command: &MoshCommand,
    request: &PaneBootstrapRequest,
) -> Option<SharedConnectionCommand> {
    let control_path = request.control_path.as_ref()?;
    // Native agent forwarding is captured from the bootstrap's own SSH
    // process, which a session on another login never talks to the agent as.
    if request.forward_agent {
        return None;
    }
    let config = ssh_config(command, &request.target).ok()?;
    let host = shared_connection_host(&config, command.family)?;
    let mut shared = command.clone();
    shared.control_path = Some(control_path.clone());
    shared.remote_ip = RemoteIpMode::Local;
    Some(SharedConnectionCommand {
        command: shared,
        host,
    })
}

/// The address a login described by `ssh -G` output connects to, or `None`
/// when a configured proxy stands between, so that only the proxy's own
/// report would be the truth.
fn shared_connection_host(config: &str, family: AddressFamily) -> Option<String> {
    let value = |key: &str| {
        config.lines().find_map(|line| {
            let (name, value) = line.split_once(char::is_whitespace)?;
            name.eq_ignore_ascii_case(key).then_some(value.trim())
        })
    };
    let configured =
        |value: Option<&str>| value.is_some_and(|value| !value.eq_ignore_ascii_case("none"));
    if configured(value("proxycommand")) || configured(value("proxyjump")) {
        return None;
    }
    let host = value("hostname")?;
    let port = value("port")
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or(22);
    let addresses = (host, port).to_socket_addrs().ok()?.collect::<Vec<_>>();
    Some(select_socket_address(&addresses, family)?.ip().to_string())
}

/// One embedded bootstrap: over the command's control socket with `host`
/// already known, or as a login of its own that discovers it.
fn bootstrap_embedded(
    command: &MoshCommand,
    target: &str,
    host: Option<&str>,
) -> Result<PaneBootstrapOutcome> {
    // The pane's emulator is not a terminal that can be asked, and there is no
    // terminal behind it that a colour probe would reach either.
    let colors = EMBEDDED_COLOR_COUNT;
    match run_ssh_bootstrap(command, target, colors, BootstrapStdin::None)? {
        BootstrapResult::UnsupportedServer { output, status } => {
            Ok(PaneBootstrapOutcome::UnsupportedServer { output, status })
        }
        BootstrapResult::Endpoint(endpoint) => {
            let host = match host {
                Some(host) => host.to_owned(),
                None => select_endpoint_host(command, target, endpoint.ip.as_deref())?,
            };
            Ok(PaneBootstrapOutcome::Endpoint(PaneEndpoint {
                host,
                port: endpoint.port,
                key: endpoint.key,
                agent_binding: endpoint.agent_binding,
                agent_path: endpoint.agent_path,
                diagnostics: endpoint.diagnostics,
            }))
        }
    }
}

/// The Mosh command an embedder's request amounts to.
///
/// Separate from the bootstrap itself so that what is sent — the SSH options,
/// the remote command, and how the server's address will be discovered — can
/// be pinned without logging in to anything.
fn embedded_command(request: &PaneBootstrapRequest) -> Result<MoshCommand> {
    anyhow::ensure!(
        !request.target.trim().is_empty(),
        "a Mosh bootstrap needs an SSH destination"
    );
    anyhow::ensure!(
        !request.remote_command.is_empty(),
        "a Mosh bootstrap for an embedder needs a remote command to run"
    );
    if let Some(interval) = request.keep_alive {
        crate::client::validate_keep_alive_interval(interval)?;
    }
    let mut command = MoshCommand {
        keep_alive: request.keep_alive,
        forward_agent: request.forward_agent,
        embedded: true,
        windows_host: request.windows_host,
        remote_command: request.remote_command.clone(),
        proxy_program: request.proxy_program.clone(),
        server_network_timeout: request.server_network_timeout,
        ..MoshCommand::default()
    };
    if let Some(port) = request.ssh_port {
        anyhow::ensure!(port != 0, "SSH port must be between 1 and 65535");
        command.ssh.extend(["-p".to_owned(), port.to_string()]);
    }
    // Without a proxy to learn the connected address from, ask the remote SSH
    // server which address it saw instead of resolving the destination here:
    // an embedder's alias may have no meaning in DNS.
    if command.proxy_program.is_none() {
        command.remote_ip = RemoteIpMode::Remote;
    }
    Ok(command)
}

/// What an embedded pane tells the remote server its terminal can do.
///
/// The standalone client asks the terminal it is running in. A pane has no
/// such terminal to ask, and every emulator an embedder would be rendering
/// into handles 256 colours.
const EMBEDDED_COLOR_COUNT: u16 = 256;

/// Run the full Mosh-compatible command line.
pub(crate) fn run(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<()> {
    let mut command = parse_args(arguments)?;
    if command.help {
        print!("{}", help_text());
        return Ok(());
    }
    if command.version {
        println!("zosh {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    command.apply_environment()?;
    if let Some(proxy) = &command.proxy {
        return run_proxy(&command, proxy);
    }

    let target = command
        .target
        .as_deref()
        .context("zosh requires a target such as user@example.com")?;
    let colors = endpoint_color_count(command.client.as_deref());
    if command.local {
        return run_local(&command, target, colors);
    }
    let bootstrap = run_ssh_bootstrap(&command, target, colors, BootstrapStdin::Terminal)?;
    let endpoint = match bootstrap {
        BootstrapResult::Endpoint(endpoint) => endpoint,
        BootstrapResult::UnsupportedServer { output, status } => {
            forward_diagnostics(&output);
            return run_plain_ssh(&command, target, status);
        }
    };
    forward_diagnostics(&endpoint.diagnostics.join("\n"));
    let host = select_endpoint_host(&command, target, endpoint.ip.as_deref())?;
    launch_endpoint(&command, &host, &endpoint)
}

impl MoshCommand {
    fn apply_environment(&mut self) -> Result<()> {
        if !self.prediction_explicit
            && let Some(value) = env::var_os("MOSH_PREDICTION_DISPLAY")
        {
            let value = value.to_string_lossy();
            self.prediction = parse_prediction(&value)
                .with_context(|| format!("invalid MOSH_PREDICTION_DISPLAY value {value:?}"))?;
        }
        if !self.predict_overwrite_explicit {
            self.predict_overwrite =
                env::var("MOSH_PREDICTION_OVERWRITE").is_ok_and(|value| value == "yes");
        }
        if !self.init_explicit && env::var_os("MOSH_NO_TERM_INIT").is_some() {
            self.init = false;
        }
        Ok(())
    }
}

fn run_local(command: &MoshCommand, target: &str, colors: u16) -> Result<()> {
    let fallback_host = local_target_host(target, command.family)
        .or_else(|_| target_host(target))
        .context("resolving the local Mosh server address")?;
    let mut local_command = command.clone();
    if matches!(&local_command.bind_server, BindServer::Ssh) {
        local_command.bind_server = BindServer::Address(fallback_host.clone());
    }
    let server = parse_server_command(&local_command.server)?;
    let (relay, agent_path) = create_bootstrap_agent_relay(&local_command, None);
    let mut server_process = Command::new(&server[0]);
    server_process
        .args(&server[1..])
        .args(local_server_arguments(&local_command, colors))
        .stdin(Stdio::null());
    if let Some(relay) = relay.as_ref() {
        server_process.env("SSH_AUTH_SOCK", relay.path());
    }
    let output = server_process
        .output()
        .with_context(|| format!("starting local Mosh server {:?}", local_command.server))?;
    let bootstrap = BootstrapOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status,
    };
    ensure_success(bootstrap.status, "local Mosh server")?;
    let mut endpoint = parse_bootstrap_output(&bootstrap.combined())?;
    endpoint.agent_binding = relay.as_ref().and_then(BootstrapAgentRelay::binding);
    endpoint.agent_path = agent_path;
    endpoint.diagnostics.retain(|line| !line.is_empty());
    forward_diagnostics(&endpoint.diagnostics.join("\n"));
    let host = endpoint.ip.as_deref().unwrap_or(&fallback_host);
    launch_endpoint(command, host, &endpoint)
}

fn launch_endpoint(command: &MoshCommand, host: &str, endpoint: &BootstrapEndpoint) -> Result<()> {
    if let Some(client_path) = command.client.as_deref() {
        return launch_external_client(command, Path::new(client_path), host, endpoint);
    }
    let key = Base64Key::from_printable(&endpoint.key).context("invalid MOSH CONNECT key")?;
    client::run_session_with_settings(
        host,
        endpoint.port,
        &key,
        endpoint_settings_with_agent(
            command,
            endpoint.agent_binding.clone(),
            endpoint.agent_path.clone(),
        ),
    )
}

/// The launcher options the bundled endpoint client consumes. Named so
/// it can be asserted without opening a socket.
#[cfg(test)]
fn endpoint_settings(command: &MoshCommand) -> SessionSettings {
    endpoint_settings_with_agent(command, None, None)
}

fn endpoint_settings_with_agent(
    command: &MoshCommand,
    agent_binding: Option<Vec<u8>>,
    agent_path: Option<PathBuf>,
) -> SessionSettings {
    SessionSettings {
        prediction: command.prediction.display_preference(),
        predict_overwrite: command.predict_overwrite,
        initialize_terminal: command.init,
        keep_alive: command.keep_alive,
        scrollback_kib: command.scrollback_kib,
        forward_agent: command.forward_agent,
        agent_binding,
        agent_path,
    }
}

fn launch_external_client(
    command: &MoshCommand,
    client: &Path,
    host: &str,
    endpoint: &BootstrapEndpoint,
) -> Result<()> {
    let mut process = Command::new(client);
    process
        .args(external_client_arguments(command, host, endpoint.port))
        .env("MOSH_KEY", &endpoint.key)
        .env("MOSH_PREDICTION_DISPLAY", command.prediction.as_env());
    if command.predict_overwrite {
        process.env("MOSH_PREDICTION_OVERWRITE", "yes");
    } else {
        process.env_remove("MOSH_PREDICTION_OVERWRITE");
    }
    // An external client is not necessarily Zosh, so the keep-alive
    // travels the way Mosh's own settings do rather than as an argument
    // stock `mosh-client` would reject.
    if let Some(interval) = command.keep_alive {
        process.env(client::KEEP_ALIVE_ENV, interval.to_string());
    } else {
        process.env_remove(client::KEEP_ALIVE_ENV);
    }
    // Likewise: an external client may be stock `mosh-client`, which would
    // reject the argument but ignores an environment variable it has never
    // heard of.
    process.env(client::SCROLLBACK_ENV, command.scrollback_kib.to_string());
    if command.forward_agent {
        process.env(client::FORWARD_AGENT_ENV, "yes");
    } else {
        process.env_remove(client::FORWARD_AGENT_ENV);
    }
    if !command.init {
        process.env("MOSH_NO_TERM_INIT", "1");
    } else {
        process.env_remove("MOSH_NO_TERM_INIT");
    }
    process
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = process.exec();
        Err(error).with_context(|| format!("starting Mosh endpoint client {}", client.display()))
    }
    #[cfg(not(unix))]
    {
        let status = process
            .status()
            .with_context(|| format!("starting Mosh endpoint client {}", client.display()))?;
        ensure_success(status, "Mosh endpoint client")
    }
}

fn external_client_arguments(command: &MoshCommand, host: &str, port: u16) -> Vec<OsString> {
    let mut arguments = Vec::new();
    if !command.original_arguments.is_empty() {
        let original = command
            .original_arguments
            .iter()
            .map(|argument| shell_quote(&argument.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(" ");
        arguments.extend([
            OsString::from("-#"),
            OsString::from(format!("{original} |")),
        ]);
    }
    arguments.extend([OsString::from(host), OsString::from(port.to_string())]);
    arguments
}

fn endpoint_color_count(client: Option<&str>) -> u16 {
    client.map_or_else(terminal::color_count, |path| {
        client_color_count(Path::new(path))
    })
}

fn client_color_count(client: &Path) -> u16 {
    let Ok(output) = Command::new(client).arg("-c").output() else {
        return 0;
    };
    if !output.status.success() {
        return 0;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u16>()
        .unwrap_or(0)
}

fn run_plain_ssh(command: &MoshCommand, target: &str, bootstrap_status: Option<i32>) -> Result<()> {
    eprintln!("zosh: no usable remote Mosh server; falling back to SSH");
    let (program, mut arguments) = ssh_base_command(command, command.forward_agent);
    arguments.push(target.to_owned());
    if !command.remote_command.is_empty() {
        arguments.push(shell_quote_words(&command.remote_command));
    }
    let status = Command::new(program)
        .args(arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("starting the SSH fallback")?;
    if status.success() {
        return Ok(());
    }
    let code = status.code().or(bootstrap_status).unwrap_or(1);
    anyhow::bail!("SSH fallback exited with status {code}")
}

fn ensure_success(status: ExitStatus, description: &str) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{description} exited with {status}")
    }
}

fn select_endpoint_host(
    command: &MoshCommand,
    target: &str,
    reported_ip: Option<&str>,
) -> Result<String> {
    match command.remote_ip {
        RemoteIpMode::Local => local_target_host(target, command.family),
        RemoteIpMode::Remote => reported_ip
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .context("the remote SSH connection did not report a Mosh server address"),
        RemoteIpMode::Proxy => reported_ip
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .context("could not determine the Mosh server address"),
    }
}

fn local_target_host(target: &str, family: AddressFamily) -> Result<String> {
    let host = target_host(target)?;
    let addresses = (host.as_str(), 22)
        .to_socket_addrs()
        .with_context(|| format!("resolving Mosh target {host:?}"))?
        .collect::<Vec<_>>();
    let selected = select_socket_address(&addresses, family)
        .context("Mosh target did not resolve to an IP address")?;
    Ok(selected.ip().to_string())
}

fn select_socket_address(addresses: &[SocketAddr], family: AddressFamily) -> Option<SocketAddr> {
    ordered_socket_addresses(addresses, family)?
        .into_iter()
        .next()
}

fn ordered_socket_addresses(
    addresses: &[SocketAddr],
    family: AddressFamily,
) -> Option<Vec<SocketAddr>> {
    let mut ordered = Vec::with_capacity(addresses.len());
    match family {
        AddressFamily::Inet => {
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv4));
        }
        AddressFamily::Inet6 => {
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv6));
        }
        AddressFamily::PreferInet => {
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv4));
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv6));
        }
        AddressFamily::PreferInet6 => {
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv6));
            ordered.extend(addresses.iter().copied().filter(SocketAddr::is_ipv4));
        }
        AddressFamily::All => ordered.extend(addresses.iter().copied()),
        AddressFamily::Auto => {
            let first = addresses.first().copied()?;
            if addresses
                .iter()
                .all(|address| address.is_ipv4() == first.is_ipv4())
            {
                ordered.extend(addresses.iter().copied());
            }
        }
    }
    (!ordered.is_empty()).then_some(ordered)
}

#[cfg(test)]
#[path = "tests/launcher.rs"]
mod tests;
