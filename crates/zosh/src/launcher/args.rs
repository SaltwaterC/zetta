//! Parsing the launcher's command line: Mosh's own options, the ones Zosh
//! adds, and the help text that documents both.

use super::*;

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

pub(super) fn parse_ssh_command(value: &str) -> Result<Vec<String>> {
    let words = shlex::split(value).context("--ssh must contain a shell-quoted command")?;
    anyhow::ensure!(!words.is_empty(), "--ssh cannot be empty");
    Ok(words)
}

pub(super) fn parse_prediction(value: &str) -> Result<PredictionMode> {
    match value {
        "adaptive" => Ok(PredictionMode::Adaptive),
        "always" => Ok(PredictionMode::Always),
        "never" => Ok(PredictionMode::Never),
        "experimental" => Ok(PredictionMode::Experimental),
        _ => anyhow::bail!("invalid prediction mode {value:?}"),
    }
}

pub(super) fn parse_bind_server(value: &str) -> Result<BindServer> {
    let normalized = value.to_ascii_lowercase();
    match normalized.as_str() {
        "ssh" => Ok(BindServer::Ssh),
        "any" => Ok(BindServer::Any),
        _ => Ok(BindServer::Address(value.to_owned())),
    }
}

#[derive(Default)]
pub(super) struct SeenOptions {
    pub(super) client: bool,
    pub(super) server: bool,
    pub(super) prediction: bool,
    pub(super) overwrite: bool,
    pub(super) family: bool,
    pub(super) port: bool,
    pub(super) bind_server: bool,
    pub(super) ssh: bool,
    pub(super) ssh_pty: bool,
    pub(super) init: bool,
    pub(super) remote_ip: bool,
    pub(super) keep_alive: bool,
    pub(super) scrollback: bool,
    pub(super) forward_agent: bool,
}

/// The parts of a `zosh` command line that a program embedding it reads back:
/// `zetta mosh` validates its arguments before handing them over unchanged,
/// and Zetta's image paste recovers the SSH invocation of a running `zosh`.
/// Parsing with the launcher's own parser is what keeps both in step with
/// every option it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherCommand {
    pub ssh: Vec<String>,
    pub target: Option<String>,
    pub remote_command: Vec<String>,
    /// `--local`: no SSH connection is opened.
    pub local: bool,
    /// `--fake-proxy`: the launcher re-entering itself as an SSH
    /// `ProxyCommand`, not a session anyone is typing into.
    pub proxy: bool,
    pub help: bool,
    pub version: bool,
}

pub fn parse_launcher_args(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<LauncherCommand> {
    let command = parse_args(arguments)?;
    Ok(LauncherCommand {
        ssh: command.ssh,
        target: command.target,
        remote_command: command.remote_command,
        local: command.local,
        proxy: command.proxy.is_some(),
        help: command.help,
        version: command.version,
    })
}

pub(super) fn parse_args(
    arguments: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<MoshCommand> {
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

pub(super) fn parse_flag(
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
        "--keep-alive" | "-k" => set_keep_alive(command, seen, KEEP_ALIVE_DEFAULT_MS)?,
        "--scrollback" | "-s" => {
            set_scrollback(command, seen, client::SCROLLBACK_DEFAULT_KIB)?;
        }
        "--no-scrollback" => set_scrollback(command, seen, 0)?,
        "--forward-agent" => set_forward_agent(command, seen, true)?,
        "--no-forward-agent" => set_forward_agent(command, seen, false)?,
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

pub(super) fn parse_attached_value(
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
            command.server_explicit = true;
            Ok(())
        }),
        "--predict" => set_prediction(command, seen, parse_prediction(value)?),
        "--keep-alive" | "-k" => {
            set_keep_alive(command, seen, client::parse_keep_alive_interval(value)?)
        }
        "--scrollback" | "-s" => {
            set_scrollback(command, seen, client::parse_scrollback_kib(value)?)
        }
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

pub(super) fn takes_value(value: &str) -> bool {
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

pub(super) fn required_value(
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

pub(super) fn set_named_value(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    name: &str,
    value: &str,
) -> Result<()> {
    let mut fake_proxy = false;
    parse_attached_value(name, value, command, seen, &mut fake_proxy)
}

pub(super) fn set_once<T>(
    seen: &mut bool,
    name: &str,
    apply: impl FnOnce() -> Result<T>,
) -> Result<()> {
    anyhow::ensure!(!*seen, "duplicate {name}");
    *seen = true;
    apply().map(|_| ())
}

pub(super) fn set_prediction(
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

pub(super) fn set_scrollback(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    kib: u32,
) -> Result<()> {
    anyhow::ensure!(!seen.scrollback, "duplicate --scrollback");
    seen.scrollback = true;
    command.scrollback_kib = kib;
    Ok(())
}

pub(super) fn set_keep_alive(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    interval: u64,
) -> Result<()> {
    anyhow::ensure!(!seen.keep_alive, "duplicate --keep-alive");
    seen.keep_alive = true;
    command.keep_alive = Some(interval);
    Ok(())
}

pub(super) fn set_forward_agent(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    enabled: bool,
) -> Result<()> {
    anyhow::ensure!(!seen.forward_agent, "duplicate agent-forwarding option");
    seen.forward_agent = true;
    command.forward_agent = enabled;
    Ok(())
}

pub(super) fn set_family(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    family: AddressFamily,
) -> Result<()> {
    anyhow::ensure!(!seen.family, "duplicate address-family option");
    seen.family = true;
    command.family = family;
    Ok(())
}

pub(super) fn set_port(
    command: &mut MoshCommand,
    seen: &mut SeenOptions,
    value: &str,
) -> Result<()> {
    anyhow::ensure!(!seen.port, "duplicate --port");
    seen.port = true;
    command.port = Some(parse_port_request(value)?);
    Ok(())
}

pub(super) fn finish_target(
    command: &mut MoshCommand,
    arguments: &[std::ffi::OsString],
    index: usize,
    fake_proxy: bool,
) -> Result<MoshCommand> {
    anyhow::ensure!(command.target.is_none(), "duplicate Mosh target");
    command.target = Some(arguments[index].to_string_lossy().into_owned());
    // `HOST -- COMMAND` is Mosh's form, and the separator is not part of the
    // command: the server invocation adds its own before it. Kept, it made the
    // server run a program named `--`, which failed only after the client had
    // associated, so the session hung on "Last contact" instead of erroring.
    let rest = &arguments[index + 1..];
    let rest = match rest.first() {
        Some(first) if first == "--" => &rest[1..],
        _ => rest,
    };
    command.remote_command = rest
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

pub(super) fn finish_without_target(command: MoshCommand, fake_proxy: bool) -> Result<MoshCommand> {
    if fake_proxy {
        anyhow::bail!("--fake-proxy requires a target and port");
    }
    if command.help || command.version {
        return Ok(command);
    }
    anyhow::bail!("zosh requires a target such as user@example.com")
}

pub(crate) fn help_text() -> &'static str {
    r#"Usage: zosh [options] [--] [user@]host [command...]
        --client=PATH        mosh client on local machine
                                (default: bundled zosh endpoint)
        --server=COMMAND     mosh server on remote machine
                                (default: "zosh-server", then "mosh-server")

        --predict=adaptive      local echo for slower links [default]
-a      --predict=always        use local echo even on fast links
-n      --predict=never         never use local echo
        --predict=experimental  aggressively echo even when incorrect

-o      --predict-overwrite     prediction overwrites instead of inserting

-k      --keep-alive            hold the session to a packet every 500 ms,
                                so aggressive WiFi power management cannot
                                park the radio between Mosh's 3 s heartbeats
        --keep-alive=MS         use MS milliseconds instead (20-3000)

-s      --scrollback            keep the output that scrolls off the screen,
                                which Mosh alone cannot: up to 1 MiB of it in
                                flight, and the program waits rather than the
                                history being lost [default]
        --scrollback=KIB        hold KIB kibibytes in flight instead (16-2048)
        --no-scrollback         keep only what is on the screen, as stock
                                Mosh does

        --forward-agent         forward SSH-agent connections when both ends
                                are Zosh (disabled by default)
        --no-forward-agent      disable SSH-agent forwarding

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

        --local                 run zosh-server locally without using ssh

        --experimental-remote-ip=(local|remote|proxy)  select the method for
                             discovering the remote IP address to use for mosh
                             (default: "proxy")

        --help                  this message
        --version               version information

"#
}
