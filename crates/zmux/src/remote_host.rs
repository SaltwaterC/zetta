//! Which command dialect a remote host's SSH server speaks, and what is sent
//! to a Windows one.
//!
//! OpenSSH runs a remote command through the account's shell. On a Windows
//! host that is PowerShell or cmd, where every `/bin/sh -c` wrapper in
//! `remote.rs` fails. A Windows host cannot be reached by stream-local
//! forwarding either — Win32-OpenSSH's server refuses `direct-streamlocal`
//! channels just as its client cannot open them — so it is always reached
//! through the `proxy-mux` bridge, whichever OS the client runs.
//!
//! The dialect is learned rather than configured, and only once a POSIX
//! command has already failed, so a POSIX host never pays for the probe. What
//! is learned is kept per destination for the life of the process.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use anyhow::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::remote::RemoteTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostPlatform {
    Posix,
    Windows,
}

/// Expands to `Windows_NT` in one half under cmd and in the other under
/// PowerShell; a POSIX shell expands neither.
pub(crate) const PLATFORM_PROBE: &str = "echo ZMUX_OS_A%OS% ZMUX_OS_B$env:OS";

pub(crate) fn classify_probe(output: &[u8]) -> HostPlatform {
    let output = String::from_utf8_lossy(output);
    if output.contains("ZMUX_OS_AWindows_NT") || output.contains("ZMUX_OS_BWindows_NT") {
        HostPlatform::Windows
    } else {
        HostPlatform::Posix
    }
}

/// Keyed by where the login goes, not by how: agent forwarding does not change
/// which shell answers.
type HostKey = (String, Option<u16>);

fn learned_platforms() -> &'static Mutex<HashMap<HostKey, HostPlatform>> {
    static LEARNED: OnceLock<Mutex<HashMap<HostKey, HostPlatform>>> = OnceLock::new();
    LEARNED.get_or_init(Mutex::default)
}

fn host_key(target: &RemoteTarget) -> HostKey {
    (target.destination().to_owned(), target.port())
}

pub(crate) fn learned(target: &RemoteTarget) -> Option<HostPlatform> {
    learned_platforms()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&host_key(target))
        .copied()
}

pub(crate) fn learn(target: &RemoteTarget, platform: HostPlatform) {
    learned_platforms()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(host_key(target), platform);
}

/// Finds `zmux` on the account's `PATH`. Windows keeps that in the registry
/// rather than in a profile script, so unlike the POSIX wrapper there is no
/// login shell to fall back to — and `-NoProfile` keeps a profile's output out
/// of what is parsed.
const RESOLVE_ZMUX: &str = "$zmux = (Get-Command zmux.exe -ErrorAction SilentlyContinue | \
     Select-Object -First 1).Source; if (-not $zmux) { \
     [Console]::Error.WriteLine('zmux was not found on this host'); exit 127 }; ";

/// Prints where `zmux` is, as the POSIX program query does.
pub(crate) fn program_command() -> String {
    encoded(&format!("{RESOLVE_ZMUX}[Console]::Out.WriteLine($zmux)"))
}

pub(crate) fn profiles_command() -> String {
    encoded(&format!(
        "{RESOLVE_ZMUX}& $zmux profiles --json; exit $LASTEXITCODE"
    ))
}

/// The bridge. PowerShell hands a native command its own standard handles
/// when nothing captures its output, so the binary link passes through
/// untouched in both directions.
pub(crate) fn bridge_command(link_agent: bool) -> String {
    let arguments = if link_agent {
        "proxy-mux --forward-agent"
    } else {
        "proxy-mux"
    };
    encoded(&format!(
        "{RESOLVE_ZMUX}& $zmux {arguments}; exit $LASTEXITCODE"
    ))
}

/// Starts the daemon so that it outlives this SSH session: `--detach` makes
/// `zmux` start it outside the job sshd ends the session's processes with.
pub(crate) fn start_daemon_command(program: &Path) -> String {
    encoded(&format!(
        "& {} --daemon --detach; exit $LASTEXITCODE",
        quote(&program.to_string_lossy())
    ))
}

/// A path a Windows host printed for its `zmux`: absolute, with a drive or a
/// UNC share, because it is later run from somewhere without this `PATH`.
pub(crate) fn parse_program_path(text: &str) -> Result<PathBuf> {
    let bytes = text.as_bytes();
    let drive = bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    anyhow::ensure!(
        drive || text.starts_with("\\\\"),
        "the remote host resolved zmux to {text}, which is not an absolute path"
    );
    Ok(PathBuf::from(text))
}

/// Encoded, because whether the account's shell is PowerShell or cmd, and
/// however it splits the command line OpenSSH joined, base64 has nothing in it
/// for either to reinterpret.
pub(crate) fn encoded(script: &str) -> String {
    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    format!(
        "powershell.exe -NoProfile -NonInteractive -EncodedCommand {}",
        STANDARD.encode(utf16)
    )
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Decodes what [`encoded`] produced, for tests that check the script itself.
#[cfg(test)]
pub(crate) fn decoded(command: &str) -> Result<String> {
    use anyhow::Context as _;

    let encoded = command
        .rsplit_once(' ')
        .context("the command has no encoded script")?
        .1;
    let bytes = STANDARD.decode(encoded)?;
    let units = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    Ok(String::from_utf16(&units)?)
}

#[cfg(test)]
#[path = "tests/remote_host.rs"]
mod tests;
