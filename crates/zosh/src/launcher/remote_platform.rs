//! Chooses the remote SSH command dialect before launching a Mosh server.
//!
//! The client can run on a different OS from the host. The probe therefore
//! observes the host's default shell instead of using this process's `cfg`.

use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RemotePlatform {
    Posix,
    Windows,
}

const PLATFORM_PROBE: &str = "echo ZOSH_OS_A%OS% ZOSH_OS_B$env:OS";

pub(super) fn probe(command: &MoshCommand, target: &str) -> Result<RemotePlatform> {
    let mut ssh = command.ssh.clone();
    let program = ssh
        .drain(..1)
        .next()
        .unwrap_or_else(|| DEFAULT_SSH.to_owned());
    let mut arguments = without_native_agent_forwarding(ssh);
    if let Some(flag) = command.family.ssh_flag() {
        arguments.push(flag.to_owned());
    }
    arguments.extend(["-T".to_owned(), "-a".to_owned()]);
    if command.remote_ip == RemoteIpMode::Proxy {
        arguments.extend([
            "-S".to_owned(),
            "none".to_owned(),
            "-o".to_owned(),
            proxy_command(command),
        ]);
    }
    arguments.extend([
        "-n".to_owned(),
        target.to_owned(),
        "--".to_owned(),
        PLATFORM_PROBE.to_owned(),
    ]);
    let output = Command::new(&program)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("probing the remote shell through {program:?}"))?;
    anyhow::ensure!(
        output.status.success(),
        "remote shell probe failed with {}{}",
        output.status,
        format_diagnostics(&String::from_utf8_lossy(&output.stderr))
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(classify_probe(&stdout))
}

fn classify_probe(output: &str) -> RemotePlatform {
    if output
        .lines()
        .any(|line| line.contains("ZOSH_OS_AWindows_NT") || line.contains("ZOSH_OS_BWindows_NT"))
    {
        RemotePlatform::Windows
    } else {
        RemotePlatform::Posix
    }
}

pub(super) fn windows_bootstrap_command(
    command: &MoshCommand,
    target: &str,
    colors: u16,
) -> Result<(String, Vec<String>)> {
    let (program, mut arguments) = ssh_base_command(command, false);
    // Win32 OpenSSH's PTY path can swallow the bootstrap line. The server
    // creates its own ConPTY after SSH exits, so the bootstrap needs no PTY.
    *arguments
        .last_mut()
        .expect("ssh_base_command adds PTY mode") = "-T".to_owned();
    let mut arguments = if command.forward_agent {
        without_native_agent_forwarding(arguments)
    } else {
        arguments
    };
    if command.forward_agent {
        arguments.push("-A".to_owned());
    }
    if command.remote_ip == RemoteIpMode::Proxy {
        arguments.extend([
            "-S".to_owned(),
            "none".to_owned(),
            "-o".to_owned(),
            proxy_command(command),
        ]);
    }
    arguments.extend([
        target.to_owned(),
        "--".to_owned(),
        windows_remote_command(command, colors)?,
    ]);
    Ok((program, arguments))
}

fn windows_remote_command(command: &MoshCommand, colors: u16) -> Result<String> {
    let script = windows_remote_script(command, colors)?;
    let mut utf16 = Vec::with_capacity(script.len() * 2);
    for unit in script.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        STANDARD.encode(utf16)
    ))
}

fn windows_remote_script(command: &MoshCommand, colors: u16) -> Result<String> {
    let address = if command.remote_ip == RemoteIpMode::Remote {
        "if ($env:SSH_CONNECTION) { [Console]::Out.WriteLine('MOSH SSH_CONNECTION ' + $env:SSH_CONNECTION) }; "
    } else {
        ""
    };
    let (selection, arguments) = if !command.server_explicit && command.server == DEFAULT_SERVER {
        let selection = "$server = (Get-Command zosh-server.exe -ErrorAction SilentlyContinue).Source; \
            if ($server) { $arguments = @ZOSH@ } else { \
            $server = (Get-Command mosh-server.exe -ErrorAction SilentlyContinue).Source; \
            $arguments = @STOCK@ }; ";
        let selection = selection
            .replace(
                "@ZOSH@",
                &powershell_array(&server_options_with_colors(command, colors, true)),
            )
            .replace(
                "@STOCK@",
                &powershell_array(&server_options_with_colors(command, colors, false)),
            );
        (selection, String::new())
    } else {
        let mut words = windows_server_words(&command.server)?;
        let server = words.remove(0);
        let zosh_server = is_zosh_server_command(std::slice::from_ref(&server));
        words.extend(server_options_with_colors(command, colors, zosh_server));
        (
            format!("$server = {}; ", powershell_quote(&server)),
            powershell_array(&words),
        )
    };
    let args = if arguments.is_empty() {
        String::new()
    } else {
        format!("$arguments = {arguments}; ")
    };
    Ok(format!(
        "$ErrorActionPreference = 'Stop'; {address}{selection}{args}\
         if (-not $server) {{ [Console]::Error.WriteLine('zosh-server not found'); exit 127 }}; \
         & $server @arguments; exit $LASTEXITCODE"
    ))
}

fn windows_server_words(value: &str) -> Result<Vec<String>> {
    let value = value.trim();
    let unquoted = value.trim_matches('"').trim_matches('\'');
    if unquoted.to_ascii_lowercase().ends_with(".exe")
        && (unquoted.contains(':') || unquoted.starts_with("\\\\"))
    {
        return Ok(vec![unquoted.to_owned()]);
    }
    parse_server_command(value)
}

fn powershell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell_array(values: &[String]) -> String {
    format!(
        "@({})",
        values
            .iter()
            .map(|value| powershell_quote(value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
#[path = "../tests/launcher/remote_platform.rs"]
mod tests;
