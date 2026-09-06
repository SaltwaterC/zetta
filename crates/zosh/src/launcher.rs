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
};

use anyhow::{Context as _, Result};
use mosh_rs::{Base64Key, DisplayPreference};

use crate::{
    client::{self, SessionSettings},
    terminal,
};

const DEFAULT_SERVER: &str = "mosh-server";
const DEFAULT_SSH: &str = "ssh";
const LOCALE_VARIABLES: [&str; 15] = [
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_ADDRESS",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_IDENTIFICATION",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_MEASUREMENT",
    "LC_NAME",
    "LC_PAPER",
    "LC_TELEPHONE",
    "LC_TIME",
];

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
    prediction: PredictionMode,
    prediction_explicit: bool,
    predict_overwrite: bool,
    predict_overwrite_explicit: bool,
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
            prediction: PredictionMode::default(),
            prediction_explicit: false,
            predict_overwrite: false,
            predict_overwrite_explicit: false,
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
    let bootstrap = run_ssh_bootstrap(&command, target, colors)?;
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
    let output = Command::new(&server[0])
        .args(&server[1..])
        .args(local_server_arguments(&local_command, colors))
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("starting local Mosh server {:?}", local_command.server))?;
    let bootstrap = BootstrapOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        status: output.status,
    };
    ensure_success(bootstrap.status, "local Mosh server")?;
    let mut endpoint = parse_bootstrap_output(&bootstrap.combined())?;
    endpoint.diagnostics.retain(|line| !line.is_empty());
    forward_diagnostics(&endpoint.diagnostics.join("\n"));
    let host = endpoint.ip.as_deref().unwrap_or(&fallback_host);
    launch_endpoint(command, host, &endpoint)
}

fn run_proxy(command: &MoshCommand, proxy: &ProxyRequest) -> Result<()> {
    let addresses = (proxy.host.as_str(), proxy.port)
        .to_socket_addrs()
        .with_context(|| format!("resolving proxy target {:?}", proxy.host))?
        .collect::<Vec<_>>();
    let candidates = ordered_socket_addresses(&addresses, command.family)
        .context("proxy target did not resolve to the requested address family")?;
    let (address, stream) = connect_proxy(candidates)?;
    eprintln!("MOSH IP {}", address.ip());
    #[cfg(unix)]
    {
        run_proxy_unix(stream)
    }
    #[cfg(not(unix))]
    {
        run_proxy_threaded(stream)
    }
}

fn copy_proxy_output<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut stdout = io::stdout();
    copy_proxy_output_to(reader, &mut stdout)
}

fn copy_proxy_output_to<R: Read, W: Write>(reader: &mut R, writer: &mut W) -> io::Result<u64> {
    let mut buffer = [0_u8; 16 * 1024];
    let mut copied = 0;
    loop {
        let length = reader.read(&mut buffer)?;
        if length == 0 {
            return Ok(copied);
        }
        writer.write_all(&buffer[..length])?;
        writer.flush()?;
        copied += length as u64;
    }
}

#[cfg(unix)]
fn run_proxy_unix(mut stream: TcpStream) -> Result<()> {
    // The stock Mosh proxy forks so the output side can close its stdout as
    // soon as the TCP peer closes. That EOF is how OpenSSH learns that its
    // ProxyCommand has ended; keeping both directions in threads leaves the
    // parent waiting on stdin forever after a remote disconnect.
    //
    // No threads have been created by the proxy path before this point, so
    // the child can safely perform the small amount of I/O below before it
    // exits with `_exit`.
    // SAFETY: this path has not created any threads, and the child only uses
    // inherited file descriptors before terminating with `_exit`.
    let child = unsafe { libc::fork() };
    if child == -1 {
        return Err(io::Error::last_os_error()).context("forking the Mosh proxy");
    }
    if child == 0 {
        // The output child must not keep the proxy input pipe open: doing so
        // prevents OpenSSH from observing EOF when the remote socket closes.
        // SAFETY: fd 0 is the proxy command's stdin and is valid or already
        // closed; either result is harmless in this terminating child.
        unsafe { libc::close(libc::STDIN_FILENO) };
        let result = copy_proxy_output(&mut stream);
        let _ = stream.shutdown(Shutdown::Read);
        // SAFETY: this is the fork child and must not run the parent's Rust
        // destructors or flush unrelated process state.
        unsafe { libc::_exit(i32::from(result.is_err())) };
    }

    // Match the stock proxy: a controlling-terminal hangup must not kill the
    // input half while OpenSSH is still using this process as its transport.
    // SAFETY: changing this process-local signal disposition is safe before
    // the proxy returns; the proxy is a short-lived helper process.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::close(libc::STDOUT_FILENO);
    }
    let mut stdin = io::stdin();
    let input_result = io::copy(&mut stdin, &mut stream);
    let _ = stream.shutdown(Shutdown::Write);

    let mut status = 0;
    let wait_result = loop {
        // SAFETY: `child` is the PID returned by `fork`, `status` is a valid
        // out-parameter, and waiting for this direct child is intentional.
        let result = unsafe { libc::waitpid(child, &mut status, 0) };
        if result == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break result;
    };
    input_result.context("forwarding Mosh proxy input")?;
    anyhow::ensure!(
        wait_result == child,
        "waiting for the Mosh proxy output child"
    );
    anyhow::ensure!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "Mosh proxy output failed"
    );
    Ok(())
}

#[cfg(not(unix))]
fn run_proxy_threaded(mut stream: TcpStream) -> Result<()> {
    let mut remote = stream
        .try_clone()
        .context("cloning the Mosh proxy stream")?;
    let reader = thread::spawn(move || {
        let result = copy_proxy_output(&mut remote);
        if let Err(error) = &result {
            eprintln!("zosh: Mosh proxy output failed: {error}");
        }
        // On platforms without fork, terminating the helper when the output
        // half closes is the equivalent of the stock proxy child exiting and
        // releasing OpenSSH's stdout pipe.
        std::process::exit(i32::from(result.is_err()));
    });
    let mut stdin = io::stdin();
    let input_result = io::copy(&mut stdin, &mut stream);
    let _ = stream.shutdown(Shutdown::Write);
    let _ = reader.join();
    input_result.context("forwarding Mosh proxy input")?;
    Ok(())
}

fn connect_proxy(candidates: Vec<SocketAddr>) -> Result<(SocketAddr, TcpStream)> {
    let mut last_error = None;
    for address in candidates {
        match TcpStream::connect(address) {
            Ok(stream) => return Ok((address, stream)),
            Err(error) => last_error = Some(error),
        }
    }
    Err(anyhow::anyhow!(
        "could not connect the Mosh proxy{}",
        last_error
            .map(|error| format!(": {error}"))
            .unwrap_or_default()
    ))
}

fn run_ssh_bootstrap(command: &MoshCommand, target: &str, colors: u16) -> Result<BootstrapResult> {
    let (program, arguments) = ssh_bootstrap_command_with_colors(command, target, colors)?;
    let mut ssh = Command::new(&program);
    // Keep the controlling terminal attached while OpenSSH creates the
    // remote PTY so its initial window size is the user's real size. `-n`
    // below still makes the bootstrap non-consuming; redirecting stdin here
    // makes OpenSSH fall back to its default 80x24 PTY dimensions.
    ssh.args(&arguments).stdin(Stdio::inherit());
    if command.remote_ip == RemoteIpMode::Proxy {
        ssh.env("SHELL", "/bin/sh");
    }
    let mut child = ssh
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting SSH bootstrap command {program:?}"))?;
    let stdout = child
        .stdout
        .take()
        .context("capturing SSH bootstrap stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("capturing SSH bootstrap stderr")?;
    let (sender, receiver) = mpsc::channel();
    let readers = vec![
        spawn_bootstrap_reader(stdout, sender.clone()),
        spawn_bootstrap_reader(stderr, sender),
    ];
    let mut combined = String::new();
    while let Ok(line) = receiver.recv() {
        combined.push_str(&line);
        if let Some(result) = bootstrap_endpoint_ready(command, &combined, &line) {
            match result {
                Ok(endpoint) => {
                    // OpenSSH changes the shared controlling terminal while
                    // a PTY-backed bootstrap is alive. Wait for it to restore
                    // that terminal before the endpoint client captures raw
                    // mode; starting Zosh as soon as MOSH CONNECT arrives
                    // races SSH and drops TUI input into the wrong process.
                    wait_bootstrap_child(child, readers)?;
                    return Ok(BootstrapResult::Endpoint(endpoint));
                }
                Err(error) if line.starts_with("MOSH CONNECT ") => {
                    stop_bootstrap_child(&mut child, readers);
                    return Err(error);
                }
                Err(_) => {}
            }
        }
    }
    finish_ssh_bootstrap(command, child, readers, combined)
}

fn bootstrap_endpoint_ready(
    command: &MoshCommand,
    combined: &str,
    line: &str,
) -> Option<Result<BootstrapEndpoint>> {
    if !line.starts_with("MOSH CONNECT ") && !combined.contains("MOSH CONNECT ") {
        return None;
    }
    let result = parse_bootstrap_output(combined);
    if command.remote_ip != RemoteIpMode::Proxy
        || result
            .as_ref()
            .ok()
            .and_then(|endpoint| endpoint.ip.as_deref())
            .is_some_and(|ip| !ip.is_empty())
    {
        return Some(result);
    }
    None
}

fn finish_ssh_bootstrap(
    command: &MoshCommand,
    mut child: Child,
    readers: Vec<thread::JoinHandle<()>>,
    combined: String,
) -> Result<BootstrapResult> {
    let status = child
        .wait()
        .context("waiting for the SSH bootstrap command")?;
    for reader in readers {
        let _ = reader.join();
    }
    if !status.success() {
        if is_unsupported_server_output_for(&combined, &command.server) {
            return Ok(BootstrapResult::UnsupportedServer {
                output: combined,
                status: status.code(),
            });
        }
        return Err(anyhow::anyhow!(
            "SSH bootstrap failed with {}{}",
            status,
            format_diagnostics(&combined)
        ));
    }
    forward_diagnostics(&combined);
    Err(anyhow::anyhow!("SSH bootstrap did not print MOSH CONNECT"))
}

fn spawn_bootstrap_reader<R>(reader: R, sender: mpsc::Sender<String>) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else {
                break;
            };
            if sender.send(format!("{line}\n")).is_err() {
                break;
            }
        }
    })
}

fn stop_bootstrap_child(child: &mut Child, readers: Vec<thread::JoinHandle<()>>) {
    let _ = child.kill();
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
}

fn wait_bootstrap_child(mut child: Child, readers: Vec<thread::JoinHandle<()>>) -> Result<()> {
    child
        .wait()
        .context("waiting for the SSH bootstrap command")?;
    for reader in readers {
        let _ = reader.join();
    }
    Ok(())
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
        SessionSettings {
            prediction: command.prediction.display_preference(),
            predict_overwrite: command.predict_overwrite,
            initialize_terminal: command.init,
        },
    )
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
    client
        .map(|path| client_color_count(Path::new(path)))
        .unwrap_or_else(terminal::color_count)
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
    eprintln!("zosh: remote mosh-server is unavailable; falling back to SSH");
    let (program, mut arguments) = ssh_base_command(command);
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

fn ssh_base_command(command: &MoshCommand) -> (String, Vec<String>) {
    let mut ssh = command.ssh.clone();
    let program = ssh
        .drain(..1)
        .next()
        .unwrap_or_else(|| DEFAULT_SSH.to_owned());
    if let Some(flag) = command.family.ssh_flag() {
        ssh.push(flag.to_owned());
    }
    ssh.push(if command.ssh_pty {
        "-tt".to_owned()
    } else {
        "-T".to_owned()
    });
    (program, ssh)
}

#[cfg(test)]
fn ssh_bootstrap_command(command: &MoshCommand, target: &str) -> (String, Vec<String>) {
    ssh_bootstrap_command_with_colors(command, target, terminal::color_count())
        .expect("test Mosh command has a valid server command")
}

fn ssh_bootstrap_command_with_colors(
    command: &MoshCommand,
    target: &str,
    colors: u16,
) -> Result<(String, Vec<String>)> {
    let (program, mut arguments) = ssh_base_command(command);
    if command.remote_ip == RemoteIpMode::Proxy {
        arguments.extend([
            "-S".to_owned(),
            "none".to_owned(),
            "-o".to_owned(),
            proxy_command(command),
        ]);
    }
    arguments.push("-n".to_owned());
    arguments.push(target.to_owned());
    // OpenSSH consumes this separator; it is not part of the remote command.
    // Stock Mosh uses it so a target or SSH option cannot be mistaken for a
    // remote command argument.
    arguments.push("--".to_owned());
    arguments.push(remote_server_command(command, colors)?);
    Ok((program, arguments))
}

fn remote_server_command(command: &MoshCommand, colors: u16) -> Result<String> {
    let server = shell_quote_words(&server_arguments_with_colors(command, colors)?);
    if command.remote_ip == RemoteIpMode::Remote {
        let marker = shell_quote(
            "[ -n \"$SSH_CONNECTION\" ] && printf '\\nMOSH SSH_CONNECTION %s\\n' \"$SSH_CONNECTION\"",
        );
        Ok(format!("sh -c {marker} ; {server}"))
    } else {
        Ok(server)
    }
}

fn proxy_command(command: &MoshCommand) -> String {
    let executable = env::current_exe()
        .unwrap_or_else(|_| PathBuf::from(if cfg!(windows) { "zosh.exe" } else { "zosh" }));
    format!(
        "ProxyCommand={} --fake-proxy --family={} -- %h %p",
        proxy_executable_quote(&executable.to_string_lossy()),
        family_name(command.family)
    )
}

fn proxy_executable_quote(executable: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", executable.replace('\"', "\\\""))
    } else {
        shell_quote(executable)
    }
}

fn family_name(family: AddressFamily) -> &'static str {
    match family {
        AddressFamily::PreferInet => "prefer-inet",
        AddressFamily::Inet => "inet",
        AddressFamily::Inet6 => "inet6",
        AddressFamily::Auto => "auto",
        AddressFamily::All => "all",
        AddressFamily::PreferInet6 => "prefer-inet6",
    }
}

#[cfg(test)]
fn server_arguments(command: &MoshCommand) -> Vec<String> {
    server_arguments_with_colors(command, terminal::color_count())
        .expect("test Mosh command has a valid server command")
}

fn server_arguments_with_colors(command: &MoshCommand, colors: u16) -> Result<Vec<String>> {
    let mut arguments = parse_server_command(&command.server)?;
    arguments.extend(server_options_with_colors(command, colors));
    Ok(arguments)
}

fn server_options_with_colors(command: &MoshCommand, colors: u16) -> Vec<String> {
    let mut arguments = vec!["new".to_owned()];
    arguments.extend(["-c".to_owned(), colors.to_string()]);
    for variable in LOCALE_VARIABLES {
        if let Ok(value) = env::var(variable)
            && !value.is_empty()
        {
            arguments.extend(["-l".to_owned(), format!("{variable}={value}")]);
        }
    }
    match &command.bind_server {
        BindServer::Ssh => arguments.push("-s".to_owned()),
        BindServer::Any => {}
        BindServer::Address(address) => {
            arguments.extend(["-i".to_owned(), address.clone()]);
        }
    }
    if let Some(port) = &command.port {
        arguments.extend(["-p".to_owned(), port.as_argument()]);
    }
    if !command.remote_command.is_empty() {
        arguments.push("--".to_owned());
        arguments.extend(command.remote_command.iter().cloned());
    }
    arguments
}

fn local_server_arguments(command: &MoshCommand, colors: u16) -> Vec<String> {
    server_options_with_colors(command, colors)
}

fn parse_server_command(value: &str) -> Result<Vec<String>> {
    let words = shlex::split(value).context("--server must contain a shell-quoted command")?;
    anyhow::ensure!(!words.is_empty(), "--server cannot be empty");
    Ok(words)
}

fn target_host(target: &str) -> Result<String> {
    let without_user = target.rsplit_once('@').map_or(target, |(_, host)| host);
    let host = if without_user.starts_with('[') {
        without_user
            .strip_prefix('[')
            .and_then(|value| value.split_once(']').map(|(host, _)| host))
            .unwrap_or(without_user)
    } else {
        without_user
            .rsplit_once(':')
            .filter(|(_, port)| {
                without_user.matches(':').count() == 1
                    && port.chars().all(|character| character.is_ascii_digit())
            })
            .map_or(without_user, |(host, _)| host)
    };
    anyhow::ensure!(!host.is_empty(), "Mosh target has an empty host");
    Ok(host.to_owned())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_quote_words(values: &[String]) -> String {
    values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn parse_port_request(value: &str) -> Result<PortRequest> {
    let (start, end) = match value.split_once(':') {
        Some((start, end)) => (start, Some(end)),
        None => (value, None),
    };
    let start = start
        .parse::<u16>()
        .with_context(|| format!("invalid Mosh port {start:?}"))?;
    let end = end
        .map(|value| {
            value
                .parse::<u16>()
                .with_context(|| format!("invalid Mosh port {value:?}"))
        })
        .transpose()?;
    if let Some(end) = end {
        anyhow::ensure!(
            start != 0 && end != 0 && end >= start,
            "Mosh port range is invalid"
        );
    }
    Ok(PortRequest { start, end })
}

pub(crate) fn parse_family(value: &str) -> Result<AddressFamily> {
    match value.to_ascii_lowercase().as_str() {
        "inet" | "ipv4" | "4" => Ok(AddressFamily::Inet),
        "inet6" | "ipv6" | "6" => Ok(AddressFamily::Inet6),
        "prefer-inet" => Ok(AddressFamily::PreferInet),
        "prefer-inet6" => Ok(AddressFamily::PreferInet6),
        "auto" => Ok(AddressFamily::Auto),
        "all" | "any" => Ok(AddressFamily::All),
        _ => anyhow::bail!(
            "invalid address family {value:?}; expected inet, inet6, auto, all, prefer-inet, or prefer-inet6"
        ),
    }
}

pub(crate) fn parse_remote_ip(value: &str) -> Result<RemoteIpMode> {
    match value {
        "local" => Ok(RemoteIpMode::Local),
        "remote" => Ok(RemoteIpMode::Remote),
        "proxy" => Ok(RemoteIpMode::Proxy),
        _ => anyhow::bail!("invalid remote IP mode {value:?}; expected local, remote, or proxy"),
    }
}

fn parse_ssh_command(value: &str) -> Result<Vec<String>> {
    let words = shlex::split(value).context("--ssh must contain a shell-quoted command")?;
    anyhow::ensure!(!words.is_empty(), "--ssh cannot be empty");
    Ok(words)
}

fn parse_prediction(value: &str) -> Result<PredictionMode> {
    match value {
        "adaptive" => Ok(PredictionMode::Adaptive),
        "always" => Ok(PredictionMode::Always),
        "never" => Ok(PredictionMode::Never),
        "experimental" => Ok(PredictionMode::Experimental),
        _ => anyhow::bail!("invalid prediction mode {value:?}"),
    }
}

fn parse_bind_server(value: &str) -> Result<BindServer> {
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "ssh" => Ok(BindServer::Ssh),
        "any" => Ok(BindServer::Any),
        _ => Ok(BindServer::Address(value.to_owned())),
    }
}

#[derive(Default)]
struct SeenOptions {
    client: bool,
    server: bool,
    prediction: bool,
    overwrite: bool,
    family: bool,
    port: bool,
    bind_server: bool,
    ssh: bool,
    ssh_pty: bool,
    init: bool,
    remote_ip: bool,
}

fn parse_args(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<MoshCommand> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut command = MoshCommand {
        original_arguments: arguments.clone(),
        ..MoshCommand::default()
    };
    let mut index = 0;
    let mut delimiter = false;
    let mut fake_proxy = false;
    let mut seen = SeenOptions::default();
    while index < arguments.len() {
        let value = arguments[index].to_string_lossy().into_owned();
        if delimiter || !value.starts_with('-') {
            return finish_target(&mut command, &arguments, index, fake_proxy);
        }
        if value == "--" {
            delimiter = true;
            index += 1;
            continue;
        }
        if parse_flag(&value, &mut command, &mut seen, &mut fake_proxy)? {
            index += 1;
            continue;
        }
        if let Some((name, attached)) = value.split_once('=') {
            parse_attached_value(name, attached, &mut command, &mut seen, &mut fake_proxy)?;
            index += 1;
            continue;
        }
        if value == "-p" || value == "--port" {
            let next = required_value(&arguments, &mut index, &value)?;
            set_port(&mut command, &mut seen, &next)?;
            continue;
        }
        if takes_value(&value) {
            let next = required_value(&arguments, &mut index, &value)?;
            set_named_value(&mut command, &mut seen, &value, &next)?;
            continue;
        }
        if let Some(port) = value.strip_prefix("-p") {
            anyhow::ensure!(!port.is_empty(), "missing value for -p");
            set_port(&mut command, &mut seen, port)?;
            index += 1;
            continue;
        }
        anyhow::bail!("unknown zosh option {value:?}");
    }
    anyhow::ensure!(!delimiter, "zosh requires a target after --");
    finish_without_target(command, fake_proxy)
}

fn parse_flag(
    value: &str,
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    fake_proxy: &mut bool,
) -> Result<bool> {
    match value {
        "-a" => set_prediction(command, seen, PredictionMode::Always)?,
        "-n" => set_prediction(command, seen, PredictionMode::Never)?,
        "--predict-overwrite" | "-o" => {
            anyhow::ensure!(!seen.overwrite, "duplicate --predict-overwrite");
            seen.overwrite = true;
            command.predict_overwrite = true;
            command.predict_overwrite_explicit = true;
        }
        "--no-predict-overwrite" => {
            anyhow::ensure!(!seen.overwrite, "duplicate --predict-overwrite");
            seen.overwrite = true;
            command.predict_overwrite = false;
            command.predict_overwrite_explicit = true;
        }
        "-4" => set_family(command, seen, AddressFamily::Inet)?,
        "-6" => set_family(command, seen, AddressFamily::Inet6)?,
        "--ssh-pty" => {
            anyhow::ensure!(!seen.ssh_pty, "duplicate SSH PTY option");
            seen.ssh_pty = true;
            command.ssh_pty = true;
        }
        "--no-ssh-pty" => {
            anyhow::ensure!(!seen.ssh_pty, "duplicate SSH PTY option");
            seen.ssh_pty = true;
            command.ssh_pty = false;
        }
        "--init" => {
            anyhow::ensure!(!seen.init, "duplicate terminal initialization option");
            seen.init = true;
            command.init = true;
            command.init_explicit = true;
        }
        "--no-init" => {
            anyhow::ensure!(!seen.init, "duplicate terminal initialization option");
            seen.init = true;
            command.init = false;
            command.init_explicit = true;
        }
        "--local" => {
            anyhow::ensure!(!command.local, "duplicate --local");
            command.local = true;
        }
        "--help" | "-h" => {
            anyhow::ensure!(!command.help, "duplicate --help");
            command.help = true;
        }
        "--version" | "-V" => {
            anyhow::ensure!(!command.version, "duplicate --version");
            command.version = true;
        }
        "--fake-proxy" => *fake_proxy = true,
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_attached_value(
    name: &str,
    value: &str,
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    _fake_proxy: &mut bool,
) -> Result<()> {
    anyhow::ensure!(!value.is_empty(), "missing value for {name}");
    match name {
        "--client" => set_once(&mut seen.client, "--client", || {
            command.client = Some(value.to_owned());
            Ok(())
        }),
        "--server" => set_once(&mut seen.server, "--server", || {
            parse_server_command(value)?;
            command.server = value.to_owned();
            Ok(())
        }),
        "--predict" => set_prediction(command, seen, parse_prediction(value)?),
        "-p" | "--port" => set_port(command, seen, value),
        "--family" => set_family(command, seen, parse_family(value)?),
        "--bind-server" => set_once(&mut seen.bind_server, "--bind-server", || {
            command.bind_server = parse_bind_server(value)?;
            Ok(())
        }),
        "--ssh" => set_once(&mut seen.ssh, "--ssh", || {
            command.ssh = parse_ssh_command(value)?;
            Ok(())
        }),
        "--experimental-remote-ip" => {
            anyhow::ensure!(!seen.remote_ip, "duplicate remote IP option");
            seen.remote_ip = true;
            command.remote_ip = parse_remote_ip(value)?;
            Ok(())
        }
        "--fake-proxy" => anyhow::bail!("--fake-proxy does not accept a value"),
        _ => anyhow::bail!("unknown zosh option {name:?}"),
    }
}

fn takes_value(value: &str) -> bool {
    matches!(
        value,
        "--client"
            | "--server"
            | "--predict"
            | "--family"
            | "--bind-server"
            | "--ssh"
            | "--experimental-remote-ip"
    )
}

fn required_value(
    arguments: &[std::ffi::OsString],
    index: &mut usize,
    option: &str,
) -> Result<String> {
    let next = arguments
        .get(*index + 1)
        .with_context(|| format!("missing value for {option}"))?
        .to_string_lossy()
        .into_owned();
    anyhow::ensure!(
        !next.starts_with('-') || option == "--bind-server",
        "missing value for {option}"
    );
    anyhow::ensure!(!next.is_empty(), "missing value for {option}");
    *index += 2;
    Ok(next)
}

fn set_named_value(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    name: &str,
    value: &str,
) -> Result<()> {
    let mut fake_proxy = false;
    parse_attached_value(name, value, command, seen, &mut fake_proxy)
}

fn set_once<T>(seen: &mut bool, name: &str, apply: impl FnOnce() -> Result<T>) -> Result<()> {
    anyhow::ensure!(!*seen, "duplicate {name}");
    *seen = true;
    apply().map(|_| ())
}

fn set_prediction(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    prediction: PredictionMode,
) -> Result<()> {
    anyhow::ensure!(!seen.prediction, "duplicate prediction option");
    seen.prediction = true;
    command.prediction = prediction;
    command.prediction_explicit = true;
    Ok(())
}

fn set_family(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    family: AddressFamily,
) -> Result<()> {
    anyhow::ensure!(!seen.family, "duplicate address-family option");
    seen.family = true;
    command.family = family;
    Ok(())
}

fn set_port(command: &mut MoshCommand, seen: &mut SeenOptions, value: &str) -> Result<()> {
    anyhow::ensure!(!seen.port, "duplicate --port");
    seen.port = true;
    command.port = Some(parse_port_request(value)?);
    Ok(())
}

fn finish_target(
    command: &mut MoshCommand,
    arguments: &[std::ffi::OsString],
    index: usize,
    fake_proxy: bool,
) -> Result<MoshCommand> {
    anyhow::ensure!(command.target.is_none(), "duplicate Mosh target");
    command.target = Some(arguments[index].to_string_lossy().into_owned());
    command.remote_command = arguments[index + 1..]
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    if fake_proxy {
        anyhow::ensure!(
            command.remote_command.len() == 1,
            "--fake-proxy requires HOST -- PORT"
        );
        let port = command.remote_command[0]
            .parse::<u16>()
            .context("invalid proxy port")?;
        anyhow::ensure!(port != 0, "proxy port must be between 1 and 65535");
        command.proxy = Some(ProxyRequest {
            host: command.target.take().unwrap_or_default(),
            port,
        });
        command.remote_command.clear();
    }
    Ok(command.clone())
}

fn finish_without_target(command: MoshCommand, fake_proxy: bool) -> Result<MoshCommand> {
    if fake_proxy {
        anyhow::bail!("--fake-proxy requires a target and port");
    }
    if command.help || command.version {
        return Ok(command);
    }
    anyhow::bail!("zosh requires a target such as user@example.com")
}

pub(crate) fn parse_bootstrap_output(output: &str) -> Result<BootstrapEndpoint> {
    let mut connect = None;
    let mut ip = None;
    let mut diagnostics = Vec::new();
    for line in output.lines().map(str::trim_end) {
        if let Some(value) = line.strip_prefix("MOSH CONNECT ") {
            anyhow::ensure!(
                connect.is_none(),
                "Mosh bootstrap printed more than one MOSH CONNECT line"
            );
            let fields = value.split_whitespace().collect::<Vec<_>>();
            anyhow::ensure!(
                fields.len() == 2,
                "malformed MOSH CONNECT line; expected port and key"
            );
            let port = fields[0]
                .parse::<u16>()
                .context("malformed MOSH CONNECT port")?;
            anyhow::ensure!(port != 0, "Mosh CONNECT port must be between 1 and 65535");
            anyhow::ensure!(valid_mosh_key(fields[1]), "malformed MOSH CONNECT key");
            connect = Some((port, fields[1].to_owned()));
        } else if let Some(value) = line.strip_prefix("MOSH IP ") {
            anyhow::ensure!(
                ip.is_none(),
                "Mosh bootstrap printed more than one server address"
            );
            let value = value.trim();
            anyhow::ensure!(
                !value.is_empty(),
                "Mosh bootstrap printed an empty server address"
            );
            ip = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("MOSH SSH_CONNECTION ") {
            anyhow::ensure!(
                ip.is_none(),
                "Mosh bootstrap printed more than one server address"
            );
            let fields = value.split_whitespace().collect::<Vec<_>>();
            anyhow::ensure!(
                fields.len() == 4,
                "malformed MOSH SSH_CONNECTION line; expected four fields"
            );
            ip = Some(fields[2].to_owned());
        } else if !line.trim().is_empty() {
            diagnostics.push(line.to_owned());
        }
    }
    let (port, key) = connect.context("SSH bootstrap did not print MOSH CONNECT")?;
    Ok(BootstrapEndpoint {
        port,
        key,
        ip,
        diagnostics,
    })
}

fn valid_mosh_key(value: &str) -> bool {
    Base64Key::from_printable(value).is_ok()
}

#[cfg(test)]
pub(crate) fn is_unsupported_server_output(output: &str) -> bool {
    is_unsupported_server_output_for(output, DEFAULT_SERVER)
}

fn is_unsupported_server_output_for(output: &str, server: &str) -> bool {
    let server = server.to_ascii_lowercase();
    let server_command = shlex::split(&server).unwrap_or_else(|| vec![server.clone()]);
    let server_name = server_command.last().map(String::as_str).unwrap_or(&server);
    let server_name = Path::new(server_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&server);
    output.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        let names_server = line.contains(&server) || line.contains(server_name);
        names_server
            && [
                "command not found",
                "not found",
                "not installed",
                "no such file",
                "unknown command",
                "illegal option",
                "unknown option",
                "unrecognized option",
                "unsupported option",
                "does not support",
                "unsupported",
                "invalid option",
            ]
            .iter()
            .any(|phrase| line.contains(phrase))
    })
}

fn forward_diagnostics(output: &str) {
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        eprintln!("mosh: {line}");
    }
}

fn format_diagnostics(output: &str) -> String {
    let lines = output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        String::new()
    } else {
        format!(": {}", lines.join(" | "))
    }
}

pub(crate) fn help_text() -> &'static str {
    r#"Usage: zosh [options] [--] [user@]host [command...]
        --client=PATH        mosh client on local machine
                                (default: bundled zosh endpoint)
        --server=COMMAND     mosh server on remote machine
                                (default: "mosh-server")

        --predict=adaptive      local echo for slower links [default]
-a      --predict=always        use local echo even on fast links
-n      --predict=never         never use local echo
        --predict=experimental  aggressively echo even when incorrect

-o      --predict-overwrite     prediction overwrites instead of inserting

-4      --family=inet           use IPv4 only
-6      --family=inet6           use IPv6 only
        --family=auto           autodetect network type for single-family hosts only
        --family=all            try all network types
        --family=prefer-inet    use all network types, but try IPv4 first [default]
        --family=prefer-inet6   use all network types, but try IPv6 first

-p PORT[:PORT2]
        --port=PORT[:PORT2]     server-side UDP port or range
                                (No effect on server-side SSH port)
        --bind-server={ssh|any|IP}  ask the server to reply from an IP address
                                       (default: "ssh")

        --ssh=COMMAND           ssh command to run when setting up session
                                (example: "ssh -p 2222")
                                (default: "ssh")

        --ssh-pty               allocate a pseudo tty on ssh connection
        --no-ssh-pty            do not allocate a pseudo tty on ssh connection

        --init                  initialize the local terminal
        --no-init               do not send terminal initialization string [default]

        --local                 run mosh-server locally without using ssh

        --experimental-remote-ip=(local|remote|proxy)  select the method for
                             discovering the remote IP address to use for mosh
                             (default: "proxy")

        --help                  this message
        --version               version information

"#
}

#[cfg(test)]
#[path = "tests/launcher.rs"]
mod tests;
