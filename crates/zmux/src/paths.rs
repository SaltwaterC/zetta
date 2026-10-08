//! Where session state lives.
//!
//! The daemon and the application have to agree on this without one importing
//! the other's configuration layer, so the resolution lives here and
//! `zetta`'s `config` module delegates to it.

use std::{
    env,
    path::{Path, PathBuf},
};

const SESSION_DIRECTORY_PREFIX: &str = "sessions-debug-v";
const SESSION_DIRECTORY_NAME: &str = "sessions";

pub fn platform_config_dir() -> PathBuf {
    #[cfg(windows)]
    return windows_config_dir(
        env::var_os("APPDATA").map(PathBuf::from),
        &private_fallback_dir(),
    );
    #[cfg(not(windows))]
    unix_config_dir(
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
        &private_fallback_dir(),
    )
}

/// The directory holding session catalogs and the control endpoint. Created
/// and checked by [`crate::private_fs::create_private_dir`] before anything is
/// written into it, and checked by [`crate::private_fs::validate_private_dir`]
/// before anything read out of it is trusted.
///
/// Binaries physically under Cargo's `target/debug` directory use a
/// protocol-scoped directory so a development build can run beside an
/// installed release. This is important while the wire protocol is still
/// changing: a debug client must not connect to an older daemon that can
/// accept the socket but cannot understand its framing. A debug-profile binary
/// installed elsewhere is treated as the installed application and uses the
/// normal session directory.
pub fn session_catalog_dir() -> PathBuf {
    let development_binary = cfg!(debug_assertions)
        && env::current_exe()
            .ok()
            .is_some_and(|path| is_target_debug_binary(&path));
    let name = if development_binary {
        format!(
            "{SESSION_DIRECTORY_PREFIX}{}",
            crate::messages::PROTOCOL_VERSION
        )
    } else {
        SESSION_DIRECTORY_NAME.to_owned()
    };
    platform_config_dir().join(name)
}

/// Stable socket name inherited by daemon-owned shells for a forwarded agent.
///
/// The link may not exist when the shell starts. A native SSH control
/// connection installs it for its lifetime; per-pane aliases normally point
/// here, while Zosh temporarily redirects its pane's alias to a private socket.
pub fn forwarded_agent_socket() -> PathBuf {
    session_catalog_dir().join("forwarded-agent.sock")
}

/// Stable agent name inherited by one daemon-owned pane.
///
/// It normally points at [`forwarded_agent_socket`], which native SSH owns.
/// A Zosh relay replaces only its pane's link with that link's private agent,
/// so concurrent panes cannot steal the socket name from one another.
#[cfg(unix)]
pub fn pane_forwarded_agent_socket(pane_id: u64) -> PathBuf {
    session_catalog_dir().join(format!("forwarded-agent-{pane_id}.sock"))
}

/// The immutable fallback behind one pane's stable agent name.
///
/// Local panes point this at the agent supplied by their creating window;
/// remote panes point it at [`forwarded_agent_socket`]. A Zosh relay can then
/// restore this name without needing to know which kind of pane it serves.
#[cfg(unix)]
pub fn pane_forwarded_agent_fallback(pane_id: u64) -> PathBuf {
    session_catalog_dir().join(format!("forwarded-agent-{pane_id}.fallback"))
}

/// Where a Zosh relay publishes its private agent pipe for one pane.
///
/// A Windows host cannot point a stable name at a named pipe the way a Unix
/// host symlinks a socket, so the daemon serves the stable name itself (see
/// [`pane_forwarded_agent_pipe`]) and reads this file on every connection to
/// learn where to relay it. The file holds a single pipe path; its absence
/// means no relay is carrying an agent for the pane right now.
#[cfg(any(windows, test))]
pub fn pane_forwarded_agent_target(pane_id: u64) -> PathBuf {
    session_catalog_dir().join(format!("forwarded-agent-{pane_id}.target"))
}

/// A fresh named pipe for a daemon-owned pane on Windows to be given as its
/// `SSH_AUTH_SOCK`, stable for the pane's lifetime.
///
/// The pipe namespace is machine-wide and any account may create a name in it.
/// A pipe created first keeps its creator's DACL, so a name another account
/// can predict is one it can claim before the daemon does and serve to the
/// pane itself — becoming the pane's agent, and receiving every key `ssh-add`
/// hands it. The random component keeps the name out of reach until it exists.
/// From then on the listener holds an instance for as long as the pane lives
/// (`server/agent_pipe/listener.rs`), including across an upgrade, because any
/// account can list existing pipe names: randomness covers the interval before
/// creation, never a gap after it.
///
/// The session-directory scope keeps a development daemon's pipes apart from an
/// installed one's in a pipe listing.
#[cfg(any(windows, test))]
pub fn new_pane_forwarded_agent_pipe(pane_id: u64) -> anyhow::Result<PathBuf> {
    let nonce = crate::transport::random_hex(16)?;
    Ok(pane_forwarded_agent_pipe_in(
        &session_catalog_dir(),
        pane_id,
        Some(&nonce),
    ))
}

/// The predictable name a daemon from before [`new_pane_forwarded_agent_pipe`]
/// gave a pane, which adopting such a daemon's handover still has to serve.
#[cfg(any(windows, test))]
pub fn legacy_pane_forwarded_agent_pipe(pane_id: u64) -> PathBuf {
    pane_forwarded_agent_pipe_in(&session_catalog_dir(), pane_id, None)
}

#[cfg(any(windows, test))]
pub(crate) const PANE_AGENT_PIPE_PREFIX: &str = r"\\.\pipe\zmux-agent-";

#[cfg(any(windows, test))]
fn pane_forwarded_agent_pipe_in(session_dir: &Path, pane_id: u64, nonce: Option<&str>) -> PathBuf {
    // FNV-1a over the lowercased path: Windows paths compare case-insensitively,
    // and this only has to be stable, not secret.
    let scope = session_dir
        .to_string_lossy()
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    match nonce {
        Some(nonce) => PathBuf::from(format!(
            "{PANE_AGENT_PIPE_PREFIX}{scope:016x}-{pane_id}-{nonce}"
        )),
        None => PathBuf::from(format!("{PANE_AGENT_PIPE_PREFIX}{scope:016x}-{pane_id}")),
    }
}

fn is_target_debug_binary(path: &Path) -> bool {
    let mut saw_target = false;
    for component in path.components() {
        if saw_target && component.as_os_str() == "debug" {
            return true;
        }
        saw_target = component.as_os_str() == "target";
    }
    false
}

#[cfg(any(not(windows), test))]
pub(crate) fn unix_config_dir(
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
    fallback: &std::path::Path,
) -> PathBuf {
    if let Some(xdg) = xdg.filter(|path| !path.as_os_str().is_empty()) {
        return xdg.join("zetta");
    }
    home.filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| fallback.join("zetta"), |home| home.join(".config/zetta"))
}

#[cfg(any(windows, test))]
pub(crate) fn windows_config_dir(app_data: Option<PathBuf>, fallback: &std::path::Path) -> PathBuf {
    app_data
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| fallback.to_path_buf())
        .join("Zetta")
}

/// Where configuration lives when the platform's per-user location is unknown.
/// [`crate::private_fs::private_fallback_dir`] says why it is safe to use.
pub(crate) fn private_fallback_dir() -> PathBuf {
    crate::private_fs::private_fallback_dir()
}

#[cfg(test)]
#[path = "tests/paths.rs"]
mod tests;
