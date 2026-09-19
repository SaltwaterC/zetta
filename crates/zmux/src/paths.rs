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
/// with `0700` by [`crate::catalog::create_private_dir`] before anything is
/// written into it.
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
///
/// The current directory is not an acceptable substitute: this directory holds
/// the process control token and the session catalogs, and a working directory
/// can be one another user may write to. A per-user path under the system
/// temporary directory keeps that ownership, and
/// [`crate::catalog::create_private_dir`] restricts it once it is created.
pub(crate) fn private_fallback_dir() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: geteuid only reads the calling process's effective user ID
        // and cannot fail.
        env::temp_dir().join(format!("zetta-{}", unsafe { libc::geteuid() }))
    }
    #[cfg(not(unix))]
    env::temp_dir().join("zetta")
}

#[cfg(test)]
#[path = "tests/paths.rs"]
mod tests;
