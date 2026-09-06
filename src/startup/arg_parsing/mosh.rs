use super::*;

use crate::mosh::{
    AddressFamily, BindServer, MoshCommand, PredictionMode, parse_family, parse_port_request,
    parse_remote_ip, parse_ssh_command,
};

pub(crate) fn parse_mosh_subcommand(arguments: &[OsString]) -> Result<StartupArgs> {
    let mut command = parse_mosh_args(arguments)?;
    command.raw_arguments = arguments.to_vec();
    Ok(StartupArgs::for_mode(StartupMode::Mosh(command)))
}

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
        command.proxy = Some(crate::mosh::ProxyRequest {
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
