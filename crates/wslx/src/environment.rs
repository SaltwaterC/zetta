//! What `wslx.exe` reads from its environment, and what it hands `wsl.exe`.

use std::ffi::OsStr;

/// The variable that carries the agent on both sides of the boundary.
pub const AGENT_VARIABLE: &str = "SSH_AUTH_SOCK";

/// The named pipe `value` names, in its canonical `\\.\pipe\` form.
///
/// Only a pipe is carried. Anything else in `SSH_AUTH_SOCK` on Windows is
/// either unset or a path nothing inside the distribution could use, and in
/// both cases `wsl.exe` runs exactly as it would have.
pub fn agent_pipe(value: &OsStr) -> Option<String> {
    let value = value.to_str()?.trim();
    let name = ["\\\\.\\pipe\\", "//./pipe/"].iter().find_map(|prefix| {
        value
            .get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .map(|_| &value[prefix.len()..])
    })?;
    (!name.is_empty()).then(|| format!("\\\\.\\pipe\\{name}"))
}

/// `WSLENV` with `SSH_AUTH_SOCK` passed through as is.
///
/// An existing entry for it is dropped rather than kept: with a `/p` or `/l`
/// flag WSL would translate the Linux socket path as though it were a Windows
/// one.
pub fn wslenv_with_agent(existing: Option<&str>) -> String {
    let mut entries: Vec<&str> = existing
        .unwrap_or_default()
        .split(':')
        .filter(|entry| {
            let name = entry.split('/').next().unwrap_or_default();
            !name.is_empty() && !name.eq_ignore_ascii_case(AGENT_VARIABLE)
        })
        .collect();
    entries.push(AGENT_VARIABLE);
    entries.join(":")
}

#[cfg(test)]
#[path = "tests/environment.rs"]
mod tests;
