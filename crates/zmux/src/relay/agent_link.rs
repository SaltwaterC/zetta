//! Pointing a pane's stable forwarded-agent name at this relay's agent.
//!
//! Unix-only: the name is a symlink to an `SSH_AUTH_SOCK` socket, and a
//! Windows host's shells find their agent some other way.

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

/// A stable daemon path pointing at this relay's private Zosh agent socket.
///
/// Replacement is atomic, so a reconnect never exposes a half-written link.
/// Cleanup restores the pane's native-SSH fallback only when this relay still
/// owns the link; an overlapping newer relay is left alone.
pub(super) struct ForwardedAgentLink {
    path: PathBuf,
    target: PathBuf,
    fallback: PathBuf,
}

impl ForwardedAgentLink {
    pub(super) fn install(pane_id: u64) -> Result<Option<Self>> {
        let Some(target) = std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from) else {
            return Ok(None);
        };
        Self::install_from(
            target,
            crate::paths::pane_forwarded_agent_socket(pane_id),
            crate::paths::pane_forwarded_agent_fallback(pane_id),
        )
    }

    pub(super) fn install_from(
        target: PathBuf,
        path: PathBuf,
        fallback: PathBuf,
    ) -> Result<Option<Self>> {
        use std::os::unix::fs::FileTypeExt as _;

        if target == path
            || !fs::metadata(&target)
                .map(|metadata| metadata.file_type().is_socket())
                .unwrap_or(false)
        {
            return Ok(None);
        }
        Self::publish(target, path, fallback).map(Some)
    }

    pub(super) fn publish(target: PathBuf, path: PathBuf, fallback: PathBuf) -> Result<Self> {
        replace_link(&target, &path).context("publishing the forwarded-agent link")?;
        Ok(Self {
            path,
            target,
            fallback,
        })
    }
}

impl Drop for ForwardedAgentLink {
    fn drop(&mut self) {
        if fs::read_link(&self.path).is_ok_and(|target| target == self.target) {
            let _ = replace_link(&self.fallback, &self.path);
        }
    }
}

fn replace_link(target: &Path, path: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    let mut temporary: OsString = path.as_os_str().to_owned();
    temporary.push(format!(".{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    match fs::remove_file(&temporary) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    symlink(target, &temporary)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
