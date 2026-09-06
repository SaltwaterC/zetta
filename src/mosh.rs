//! The `zetta mosh` compatibility proxy.
//!
//! The full Mosh command belongs to the bundled `zosh` executable. Keeping
//! this process as a transparent handoff is important: `zosh` must see the
//! same option spelling, target, and remote command that a user supplied to
//! `zetta mosh`.

use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result};
use task::ShellKind;

const DEFAULT_SERVER: &str = "mosh-server";
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

#[cfg(test)]
#[path = "tests/mosh.rs"]
mod tests;
