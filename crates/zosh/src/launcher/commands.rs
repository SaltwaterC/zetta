//! The command lines the launcher builds: the `ssh` invocation, the remote
//! server's arguments and environment, and the shell quoting between them.

use super::*;

pub(super) fn ssh_base_command(
    command: &MoshCommand,
    forward_agent: bool,
) -> (String, Vec<String>) {
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
    if forward_agent {
        ssh.push("-A".to_owned());
    }
    (program, ssh)
}

pub(super) fn without_native_agent_forwarding(arguments: Vec<String>) -> Vec<String> {
    let mut filtered = Vec::with_capacity(arguments.len());
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "-A"
            || argument.eq_ignore_ascii_case("-oforwardagent=yes")
            || argument.eq_ignore_ascii_case("-oforwardagent=no")
        {
            index += 1;
            continue;
        }
        if argument == "-o"
            && arguments.get(index + 1).is_some_and(|value| {
                value.eq_ignore_ascii_case("ForwardAgent=yes")
                    || value.eq_ignore_ascii_case("ForwardAgent=no")
            })
        {
            index += 2;
            continue;
        }
        filtered.push(argument.clone());
        index += 1;
    }
    filtered
}

#[cfg(test)]
pub(super) fn ssh_bootstrap_command(command: &MoshCommand, target: &str) -> (String, Vec<String>) {
    ssh_bootstrap_command_with_colors(command, target, terminal::color_count())
        .expect("test Mosh command has a valid server command")
}

pub(super) fn ssh_bootstrap_command_with_colors(
    command: &MoshCommand,
    target: &str,
    colors: u16,
) -> Result<(String, Vec<String>)> {
    // Native forwarding is temporary: zosh-server opens the inherited socket
    // once so OpenSSH emits its forwarding session binding. Subsequent agent
    // traffic travels over the authenticated Zosh state instead.
    let (program, arguments) = ssh_base_command(command, false);
    let mut arguments = if command.forward_agent {
        without_native_agent_forwarding(arguments)
    } else {
        arguments
    };
    if command.forward_agent {
        arguments.push("-A".to_owned());
    }
    if let Some(control_path) = &command.control_path {
        // A session on the embedder's login. Forwards the user's
        // configuration adds belong to that login already.
        arguments.extend([
            "-S".to_owned(),
            control_path.display().to_string(),
            "-o".to_owned(),
            "ControlMaster=no".to_owned(),
            "-o".to_owned(),
            "ClearAllForwardings=yes".to_owned(),
        ]);
    } else if command.remote_ip == RemoteIpMode::Proxy {
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

pub(super) fn remote_server_command(command: &MoshCommand, colors: u16) -> Result<String> {
    let server = remote_server_invocation(command, colors)?;
    if command.remote_ip == RemoteIpMode::Remote {
        let marker = shell_quote(
            "[ -n \"$SSH_CONNECTION\" ] && printf '\\nMOSH SSH_CONNECTION %s\\n' \"$SSH_CONNECTION\"",
        );
        Ok(format!("sh -c {marker} ; {server}"))
    } else {
        Ok(server)
    }
}

pub(super) fn remote_server_invocation(command: &MoshCommand, colors: u16) -> Result<String> {
    if !command.server_explicit && command.server == DEFAULT_SERVER {
        let zosh = server_invocation_for(command, DEFAULT_SERVER, colors);
        let stock = server_invocation_for(command, STOCK_SERVER, colors);
        return Ok(format!(
            "if command -v {DEFAULT_SERVER} >/dev/null 2>&1; then {zosh}; else {stock}; fi"
        ));
    }
    Ok(format!(
        "{}{}",
        server_environment(command),
        shell_quote_words(&server_arguments_with_colors(command, colors)?)
    ))
}

/// The environment assignment a server invocation is prefixed with, if any.
/// A prefix rather than an `export`, so it reaches only the server and the
/// remote shell's syntax is the same as for the invocation itself.
pub(super) fn server_environment(command: &MoshCommand) -> String {
    command
        .server_network_timeout
        .map(|timeout| format!("MOSH_SERVER_NETWORK_TMOUT={} ", timeout.as_secs().max(1)))
        .unwrap_or_default()
}

pub(super) fn server_invocation_for(command: &MoshCommand, server: &str, colors: u16) -> String {
    let mut arguments = vec![server.to_owned()];
    arguments.extend(server_options_with_colors(
        command,
        colors,
        server == DEFAULT_SERVER,
    ));
    format!(
        "{}{}",
        server_environment(command),
        shell_quote_words(&arguments)
    )
}

pub(super) fn proxy_command(command: &MoshCommand) -> String {
    let executable = command
        .proxy_program
        .clone()
        .or_else(|| env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "zosh.exe" } else { "zosh" }));
    format!(
        "ProxyCommand={} --fake-proxy --family={} -- %h %p",
        proxy_executable_quote(&executable.to_string_lossy()),
        family_name(command.family)
    )
}

pub(super) fn proxy_executable_quote(executable: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", executable.replace('\"', "\\\""))
    } else {
        shell_quote(executable)
    }
}

pub(super) fn family_name(family: AddressFamily) -> &'static str {
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
pub(super) fn server_arguments(command: &MoshCommand) -> Vec<String> {
    server_arguments_with_colors(command, terminal::color_count())
        .expect("test Mosh command has a valid server command")
}

pub(super) fn server_arguments_with_colors(
    command: &MoshCommand,
    colors: u16,
) -> Result<Vec<String>> {
    let mut arguments = parse_server_command(&command.server)?;
    let zosh_server = is_zosh_server_command(&arguments);
    arguments.extend(server_options_with_colors(command, colors, zosh_server));
    Ok(arguments)
}

pub(super) fn is_zosh_server_command(arguments: &[String]) -> bool {
    arguments.iter().any(|argument| {
        let name = argument.rsplit(['/', '\\']).next().unwrap_or(argument);
        name.eq_ignore_ascii_case(DEFAULT_SERVER) || name.eq_ignore_ascii_case("zosh-server.exe")
    })
}

pub(super) fn server_options_with_colors(
    command: &MoshCommand,
    colors: u16,
    zosh_server: bool,
) -> Vec<String> {
    let mut arguments = vec!["new".to_owned()];
    arguments.extend(["-c".to_owned(), colors.to_string()]);
    // The server already inherits the environment that sshd established for
    // the remote command. Leave locale selection to OpenSSH's
    // SendEnv/AcceptEnv policy and the remote host's own defaults instead of
    // overriding it with locale names that may exist only on the client.
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
    if zosh_server && command.forward_agent {
        arguments.push("--forward-agent".to_owned());
    }
    if !command.remote_command.is_empty() {
        arguments.push("--".to_owned());
        arguments.extend(command.remote_command.iter().cloned());
    }
    arguments
}

pub(super) fn local_server_arguments(command: &MoshCommand, colors: u16) -> Vec<String> {
    let zosh_server = parse_server_command(&command.server)
        .is_ok_and(|arguments| is_zosh_server_command(&arguments));
    server_options_with_colors(command, colors, zosh_server)
}

pub(super) fn parse_server_command(value: &str) -> Result<Vec<String>> {
    let words = shlex::split(value).context("--server must contain a shell-quoted command")?;
    anyhow::ensure!(!words.is_empty(), "--server cannot be empty");
    Ok(words)
}

pub(super) fn target_host(target: &str) -> Result<String> {
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

pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(super) fn shell_quote_words(values: &[String]) -> String {
    values
        .iter()
        .map(|value| shell_quote(value))
        .collect::<Vec<_>>()
        .join(" ")
}
