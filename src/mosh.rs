//! The `zetta mosh` compatibility proxy and the Mosh launcher's option table.
//!
//! The full Mosh command belongs to the bundled `zosh` executable. Keeping
//! this process as a transparent handoff is important: `zosh` must see the
//! same option spelling, target, and remote command that a user supplied to
//! `zetta mosh`.
//!
//! [`parse_mosh_args`] lives here rather than beside the other subcommand
//! parsers because two callers need it and only one of them is a command line:
//! `startup::arg_parsing::mosh` builds a `StartupMode` from it, and
//! `ssh_image_paste` parses the *foreground process* of a `zosh` pane with it
//! to recover the SSH invocation a clipboard image has to be uploaded through.

use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result};
use task::ShellKind;

const DEFAULT_SERVER: &str = "zosh-server";
const DEFAULT_SSH: &str = "ssh";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PredictionMode {
    Adaptive,
    Always,
    Never,
    Experimental,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddressFamily {
    PreferInet,
    Inet,
    Inet6,
    Auto,
    All,
    PreferInet6,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BindServer {
    Ssh,
    Any,
    Address(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteIpMode {
    Local,
    Remote,
    Proxy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortRequest {
    pub(crate) start: u16,
    pub(crate) end: Option<u16>,
}

impl PortRequest {
    #[cfg(test)]
    pub(crate) fn as_argument(&self) -> String {
        match self.end {
            Some(end) => format!("{}:{}", self.start, end),
            None => self.start.to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProxyRequest {
    pub(crate) host: String,
    pub(crate) port: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoshCommand {
    pub(crate) client: Option<String>,
    pub(crate) server: String,
    pub(crate) prediction: PredictionMode,
    pub(crate) prediction_explicit: bool,
    pub(crate) predict_overwrite: bool,
    /// `-k`/`--keep-alive`, in milliseconds. Validated here and forwarded
    /// verbatim; `zosh` owns what it does with it.
    pub(crate) keep_alive: Option<u64>,
    pub(crate) family: AddressFamily,
    pub(crate) port: Option<PortRequest>,
    pub(crate) bind_server: BindServer,
    pub(crate) ssh: Vec<String>,
    pub(crate) ssh_pty: bool,
    pub(crate) init: bool,
    pub(crate) init_explicit: bool,
    pub(crate) local: bool,
    pub(crate) remote_ip: RemoteIpMode,
    pub(crate) target: Option<String>,
    pub(crate) remote_command: Vec<String>,
    pub(crate) help: bool,
    pub(crate) version: bool,
    pub(crate) proxy: Option<ProxyRequest>,
    /// The exact argument vector after the `mosh` subcommand. Startup parsing
    /// still validates it for Zetta's completion and diagnostics, but the
    /// runtime forwards this untouched to `zosh`.
    pub(crate) raw_arguments: Vec<OsString>,
}

impl Default for MoshCommand {
    fn default() -> Self {
        Self {
            client: None,
            server: DEFAULT_SERVER.to_owned(),
            prediction: PredictionMode::Adaptive,
            prediction_explicit: false,
            predict_overwrite: false,
            keep_alive: None,
            family: AddressFamily::PreferInet,
            port: None,
            bind_server: BindServer::Ssh,
            ssh: vec![DEFAULT_SSH.to_owned()],
            ssh_pty: true,
            init: true,
            init_explicit: false,
            local: false,
            remote_ip: RemoteIpMode::Proxy,
            target: None,
            remote_command: Vec::new(),
            help: false,
            version: false,
            proxy: None,
            raw_arguments: Vec::new(),
        }
    }
}

/// Forward `zetta mosh` to the bundled `zosh` process without changing the
/// Mosh command's semantics or target handling.
pub(crate) fn run(command: &MoshCommand) -> Result<()> {
    let arguments = forwarded_arguments(command);
    let zosh = resolve_zosh()?;
    let mut process = Command::new(&zosh);
    process
        .args(&arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = process.exec();
        Err(error).with_context(|| format!("starting bundled zosh {}", zosh.display()))
    }
    #[cfg(not(unix))]
    {
        let status = process
            .status()
            .with_context(|| format!("starting bundled zosh {}", zosh.display()))?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("zosh exited with {status}")
        }
    }
}

fn forwarded_arguments(command: &MoshCommand) -> Vec<OsString> {
    if !command.raw_arguments.is_empty() {
        return command.raw_arguments.clone();
    }
    if command.help {
        return vec![OsString::from("--help")];
    }
    if command.version {
        return vec![OsString::from("--version")];
    }
    command
        .target
        .as_deref()
        .map(|target| {
            let mut arguments = vec![OsString::from(target)];
            arguments.extend(command.remote_command.iter().map(OsString::from));
            arguments
        })
        .unwrap_or_default()
}

fn resolve_zosh() -> Result<PathBuf> {
    let current = env::current_exe().context("locating the running zetta executable")?;
    if let Some(parent) = current.parent() {
        let sibling = parent.join(if cfg!(windows) { "zosh.exe" } else { "zosh" });
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from(if cfg!(windows) {
        "zosh.exe"
    } else {
        "zosh"
    }))
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

/// Bounds on `--keep-alive=MS`, matching `zosh`'s own.  Below Mosh's
/// minimum frame interval a keep-alive cannot go out any sooner; above
/// its unassisted heartbeat there is nothing left to ask for.
pub(crate) const KEEP_ALIVE_DEFAULT_MS: u64 = 500;
const KEEP_ALIVE_MIN_MS: u64 = 20;
const KEEP_ALIVE_MAX_MS: u64 = 3000;

pub(crate) fn parse_keep_alive_interval(value: &str) -> Result<u64> {
    let interval = value
        .parse::<u64>()
        .with_context(|| format!("invalid keep-alive interval {value:?}"))?;
    anyhow::ensure!(
        (KEEP_ALIVE_MIN_MS..=KEEP_ALIVE_MAX_MS).contains(&interval),
        "keep-alive interval must be between {KEEP_ALIVE_MIN_MS} and {KEEP_ALIVE_MAX_MS} milliseconds"
    );
    Ok(interval)
}

pub(crate) fn parse_remote_ip(value: &str) -> Result<RemoteIpMode> {
    match value {
        "local" => Ok(RemoteIpMode::Local),
        "remote" => Ok(RemoteIpMode::Remote),
        "proxy" => Ok(RemoteIpMode::Proxy),
        _ => anyhow::bail!("invalid remote IP mode {value:?}; expected local, remote, or proxy"),
    }
}

pub(crate) fn parse_ssh_command(value: &str) -> Result<Vec<String>> {
    let words = ShellKind::Posix
        .split(value)
        .context("--ssh must contain a shell-quoted command")?;
    anyhow::ensure!(!words.is_empty(), "--ssh cannot be empty");
    Ok(words)
}

/// Parses a Mosh launcher command line — everything after `zetta mosh`, or the
/// arguments of a running `zosh`/`mosh` process.
pub(crate) fn parse_mosh_args(arguments: &[OsString]) -> Result<MoshCommand> {
    let mut command = MoshCommand::default();
    let mut index = 0;
    let mut delimiter = false;
    let mut fake_proxy = false;
    let mut seen = SeenOptions::default();

    while index < arguments.len() {
        let value = arguments[index].to_string_lossy().into_owned();
        if delimiter {
            return finish_target(&mut command, arguments, index, fake_proxy);
        }
        if value == "--" {
            delimiter = true;
            index += 1;
            continue;
        }
        if !value.starts_with('-') {
            return finish_target(&mut command, arguments, index, fake_proxy);
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
            let next = required_value(arguments, &mut index, &value)?;
            set_port(&mut command, &mut seen, &next)?;
            continue;
        }
        if takes_value(&value) {
            let next = required_value(arguments, &mut index, &value)?;
            set_named_value(&mut command, &mut seen, &value, &next)?;
            continue;
        }
        if let Some(port) = value.strip_prefix("-p") {
            anyhow::ensure!(!port.is_empty(), "missing value for -p");
            set_port(&mut command, &mut seen, port)?;
            index += 1;
            continue;
        }
        anyhow::bail!("unknown zetta mosh option {value:?}");
    }

    anyhow::ensure!(!delimiter, "zetta mosh requires a target after --");
    finish_without_target(command, fake_proxy)
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
    keep_alive: bool,
}

fn parse_flag(
    value: &str,
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    fake_proxy: &mut bool,
) -> Result<bool> {
    match value {
        "-a" => {
            set_prediction(command, seen, PredictionMode::Always)?;
        }
        "-n" => {
            set_prediction(command, seen, PredictionMode::Never)?;
        }
        "--predict-overwrite" | "-o" => {
            anyhow::ensure!(!seen.overwrite, "duplicate --predict-overwrite");
            seen.overwrite = true;
            command.predict_overwrite = true;
        }
        "--no-predict-overwrite" => {
            anyhow::ensure!(!seen.overwrite, "duplicate --predict-overwrite");
            seen.overwrite = true;
            command.predict_overwrite = false;
        }
        "--keep-alive" | "-k" => {
            set_keep_alive(command, seen, KEEP_ALIVE_DEFAULT_MS)?;
        }
        "-4" => {
            set_family(command, seen, AddressFamily::Inet)?;
        }
        "-6" => {
            set_family(command, seen, AddressFamily::Inet6)?;
        }
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
    fake_proxy: &mut bool,
) -> Result<()> {
    anyhow::ensure!(!value.is_empty(), "missing value for {name}");
    match name {
        "-p" => set_port(command, seen, value),
        "--client" => set_once(&mut seen.client, "--client", || {
            command.client = Some(value.to_owned());
            Ok(())
        }),
        "--server" => set_once(&mut seen.server, "--server", || {
            command.server = value.to_owned();
            Ok(())
        }),
        "--predict" => set_prediction(command, seen, parse_prediction(value)?),
        "--keep-alive" | "-k" => set_keep_alive(command, seen, parse_keep_alive_interval(value)?),
        "--family" => set_family(command, seen, parse_family(value)?),
        "--port" => set_port(command, seen, value),
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
        "--fake-proxy" => {
            anyhow::ensure!(value == "true", "--fake-proxy does not take a value");
            *fake_proxy = true;
            Ok(())
        }
        _ => anyhow::bail!("unknown zetta mosh option {name:?}"),
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

fn required_value(arguments: &[OsString], index: &mut usize, option: &str) -> Result<String> {
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
    parse_attached_value(name, value, command, seen, &mut fake_proxy)?;
    anyhow::ensure!(!fake_proxy, "{name} does not accept an attached value");
    Ok(())
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

fn set_keep_alive(command: &mut MoshCommand, seen: &mut SeenOptions, interval: u64) -> Result<()> {
    anyhow::ensure!(!seen.keep_alive, "duplicate --keep-alive");
    seen.keep_alive = true;
    command.keep_alive = Some(interval);
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

fn finish_target(
    command: &mut MoshCommand,
    arguments: &[OsString],
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
    anyhow::bail!("zetta mosh requires a target such as user@example.com")
}

#[cfg(test)]
#[path = "tests/mosh.rs"]
mod tests;
