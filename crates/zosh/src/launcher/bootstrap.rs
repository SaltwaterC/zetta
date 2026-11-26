//! The SSH bootstrap: starting the remote server over `ssh`, relaying the
//! agent it may need, and reading the endpoint (or the failure) it prints.

use super::*;

/// Where the bootstrap's SSH process gets its standard input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BootstrapStdin {
    /// Keep the controlling terminal attached while OpenSSH creates the
    /// remote PTY, so its initial window size is the user's real size.
    Terminal,
    /// An embedder has no terminal to lend, and does not want one consumed on
    /// its behalf. OpenSSH falls back to its default 80x24 PTY, which the
    /// first resize replaces.
    None,
}

pub(super) fn create_bootstrap_agent_relay(
    command: &MoshCommand,
    target: Option<&str>,
) -> (Option<BootstrapAgentRelay>, Option<PathBuf>) {
    if !command.forward_agent {
        return (None, None);
    }
    let agent_path = match target {
        Some(target) => {
            resolve_ssh_agent_path(command, target).unwrap_or_else(|_| environment_agent_path())
        }
        None => environment_agent_path(),
    }
    .map(normalize_agent_path);
    let relay = match agent_path.as_ref() {
        Some(path) => BootstrapAgentRelay::for_agent_path(path.clone()),
        None => Ok(None),
    };
    let relay = match relay {
        Ok(relay) => relay,
        Err(error) => {
            eprintln!("zosh: native agent bootstrap relay unavailable: {error}");
            None
        }
    };
    (relay, agent_path)
}

pub(super) fn resolve_ssh_agent_path(
    command: &MoshCommand,
    target: &str,
) -> Result<Option<PathBuf>> {
    let config = ssh_config(command, target)?;
    Ok(parse_forward_agent(&config)
        .or_else(|| parse_identity_agent(&config))
        .or_else(environment_agent_path))
}

/// What OpenSSH's configuration resolves for `target` (`ssh -G`), without
/// connecting to anything.
pub(super) fn ssh_config(command: &MoshCommand, target: &str) -> Result<String> {
    let mut ssh = command.ssh.clone();
    let program = ssh
        .drain(..1)
        .next()
        .unwrap_or_else(|| DEFAULT_SSH.to_owned());
    if let Some(flag) = command.family.ssh_flag() {
        ssh.push(flag.to_owned());
    }
    let output = Command::new(&program)
        .args(&ssh)
        .arg("-G")
        .arg(target)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("querying {program:?} for its configuration"))?;
    anyhow::ensure!(
        output.status.success(),
        "{program:?} could not resolve its configuration for {target:?}"
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn parse_identity_agent(config: &str) -> Option<PathBuf> {
    parse_agent_config_value(config, "identityagent")
}

pub(super) fn parse_forward_agent(config: &str) -> Option<PathBuf> {
    parse_agent_config_value(config, "forwardagent")
}

pub(super) fn parse_agent_config_value(config: &str, key: &str) -> Option<PathBuf> {
    let value = config.lines().find_map(|line| {
        let (name, value) = line.split_once(char::is_whitespace)?;
        name.eq_ignore_ascii_case(key).then_some(value.trim())
    })?;
    if ["none", "yes", "no"]
        .iter()
        .any(|word| value.eq_ignore_ascii_case(word))
    {
        return None;
    }
    if value == "SSH_AUTH_SOCK" || value == "$SSH_AUTH_SOCK" {
        return std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from);
    }
    if let Some(name) = value.strip_prefix('$') {
        return std::env::var_os(name).map(PathBuf::from);
    }
    Some(PathBuf::from(value))
}

pub(super) fn run_ssh_bootstrap(
    command: &MoshCommand,
    target: &str,
    colors: u16,
    stdin: BootstrapStdin,
) -> Result<BootstrapResult> {
    let platform = if command.embedded && command.windows_host {
        remote_platform::RemotePlatform::Windows
    } else if command.embedded {
        remote_platform::RemotePlatform::Posix
    } else {
        remote_platform::probe(command, target)?
    };
    let (program, arguments) = match platform {
        remote_platform::RemotePlatform::Posix => {
            ssh_bootstrap_command_with_colors(command, target, colors)?
        }
        remote_platform::RemotePlatform::Windows => {
            remote_platform::windows_bootstrap_command(command, target, colors)?
        }
    };
    let (relay, agent_path) = create_bootstrap_agent_relay(command, Some(target));
    let mut ssh = Command::new(&program);
    // `-n` already makes the bootstrap non-consuming; this only decides whose
    // terminal, if any, sizes the remote PTY.
    if let Some(relay) = relay.as_ref() {
        // Keep login authentication on the configured IdentityAgent. Only the
        // forwarded channel passes through the capture relay; 1Password can
        // reject login authentication when it is proxied by another process.
        // Command-line values are applied before ssh_config values, so put the
        // socket path before the later `-A` shorthand.
        ssh.arg("-o")
            .arg(format!("ForwardAgent={}", relay.path().display()));
    }
    ssh.args(&arguments).stdin(match stdin {
        BootstrapStdin::Terminal => Stdio::inherit(),
        BootstrapStdin::None => Stdio::null(),
    });
    // Preserve SSH_AUTH_SOCK: IdentityAgent may refer to it, and agent
    // applications can use it while authorizing the login. ForwardAgent
    // already selects the capture relay without changing authentication.
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
                    let mut endpoint = endpoint;
                    endpoint.agent_binding = relay.as_ref().and_then(BootstrapAgentRelay::binding);
                    endpoint.agent_path = agent_path;
                    return Ok(BootstrapResult::Endpoint(endpoint));
                }
                Err(error)
                    if line
                        .split('\r')
                        .any(|part| part.trim_start().starts_with("MOSH CONNECT ")) =>
                {
                    stop_bootstrap_child(&mut child, readers);
                    return Err(error);
                }
                Err(_) => {}
            }
        }
    }
    finish_ssh_bootstrap(command, child, readers, combined)
}

pub(super) fn bootstrap_endpoint_ready(
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

pub(super) fn finish_ssh_bootstrap(
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
        if unsupported_server_output_for_command(&combined, command) {
            return Ok(BootstrapResult::UnsupportedServer {
                output: combined,
                status: status.code(),
            });
        }
        return Err(ssh_bootstrap_failure(command, status, &combined));
    }
    if combined.contains("MOSH CONNECT ") {
        let endpoint = parse_bootstrap_output(&combined)?;
        if command.remote_ip == RemoteIpMode::Proxy && endpoint.ip.is_none() {
            anyhow::bail!(
                "SSH bootstrap printed MOSH CONNECT but the proxy did not report MOSH IP"
            );
        }
        anyhow::bail!("SSH bootstrap printed MOSH CONNECT but the endpoint was not accepted");
    }
    forward_diagnostics(&combined);
    Err(anyhow::anyhow!("SSH bootstrap did not print MOSH CONNECT"))
}

/// Explain authentication failures without burying the remedy under repeated
/// password attempts and address-discovery output.
pub(super) fn ssh_bootstrap_failure(
    command: &MoshCommand,
    status: ExitStatus,
    output: &str,
) -> anyhow::Error {
    let Some(denial) = output
        .lines()
        .find(|line| line.contains("Permission denied ("))
    else {
        return anyhow::anyhow!(
            "SSH bootstrap failed with {status}{}",
            format_diagnostics(output)
        );
    };
    let mut explanation = String::from("SSH authentication failed for the Zosh startup login. ");
    if command.embedded && command.forward_agent {
        explanation.push_str(
            "Agent forwarding requires a separate SSH login, even when the session's existing \
             SSH connection works. ",
        );
    }
    explanation.push_str(
        "Unlock your SSH agent (such as 1Password) and authorize the new login, then reconnect. ",
    );
    if output
        .lines()
        .any(|line| line.contains("ssh_askpass:") && line.contains("No such file or directory"))
    {
        explanation.push_str(
            "SSH also could not prompt for a password because its local askpass helper is missing. ",
        );
    }
    anyhow::anyhow!("{explanation}{}", denial.trim())
}

pub(super) fn spawn_bootstrap_reader<R>(
    reader: R,
    sender: mpsc::Sender<String>,
) -> thread::JoinHandle<()>
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

pub(super) fn stop_bootstrap_child(child: &mut Child, readers: Vec<thread::JoinHandle<()>>) {
    let _ = child.kill();
    let _ = child.wait();
    for reader in readers {
        let _ = reader.join();
    }
}

pub(super) fn wait_bootstrap_child(
    mut child: Child,
    readers: Vec<thread::JoinHandle<()>>,
) -> Result<()> {
    child
        .wait()
        .context("waiting for the SSH bootstrap command")?;
    for reader in readers {
        let _ = reader.join();
    }
    Ok(())
}

pub(crate) fn parse_bootstrap_output(output: &str) -> Result<BootstrapEndpoint> {
    let mut connect = None;
    let mut ip = None;
    let mut diagnostics = Vec::new();
    // A PTY can put a carriage return before a protocol line, including on
    // Windows where SSH's output uses CRLF. Treat CR as a line boundary.
    for line in output.split(['\r', '\n']).map(str::trim) {
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
        agent_binding: None,
        agent_path: None,
    })
}

pub(super) fn valid_mosh_key(value: &str) -> bool {
    Base64Key::from_printable(value).is_ok()
}

#[cfg(test)]
pub(crate) fn is_unsupported_server_output(output: &str) -> bool {
    is_unsupported_server_output_for(output, DEFAULT_SERVER)
        || is_unsupported_server_output_for(output, STOCK_SERVER)
}

pub(super) fn unsupported_server_output_for_command(output: &str, command: &MoshCommand) -> bool {
    if !command.server_explicit && command.server == DEFAULT_SERVER {
        return is_unsupported_server_output_for(output, DEFAULT_SERVER)
            || is_unsupported_server_output_for(output, STOCK_SERVER);
    }
    is_unsupported_server_output_for(output, &command.server)
}

pub(super) fn is_unsupported_server_output_for(output: &str, server: &str) -> bool {
    let server = server.to_ascii_lowercase();
    let server_command = shlex::split(&server).unwrap_or_else(|| vec![server.clone()]);
    let server_name = server_command
        .last()
        .map_or(server.as_str(), String::as_str);
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

pub(super) fn forward_diagnostics(output: &str) {
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        eprintln!("mosh: {line}");
    }
}

pub(super) fn format_diagnostics(output: &str) -> String {
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
