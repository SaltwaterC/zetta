//! Where session state lives.
//!
//! The daemon and the application have to agree on this without one importing
//! the other's configuration layer, so the resolution lives here and
//! `zetta`'s `config` module delegates to it.

use std::{env, path::PathBuf};

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
pub fn session_catalog_dir() -> PathBuf {
    platform_config_dir().join("sessions")
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
