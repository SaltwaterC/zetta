//! Telling a Windows host's daemon which agent a pane's relay carries.
//!
//! The Windows counterpart of `agent_link.rs`. A named pipe cannot be
//! symlinked, so the daemon serves each pane's stable agent pipe itself and
//! relays every connection to the pipe named in a small file; this writes that
//! file for as long as the relay lives. Compiled on every platform under test,
//! because nothing here but the caller is Windows-specific.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

/// A published target file naming this relay's private Zosh agent pipe.
///
/// Replacement is a rename, so the daemon never reads a half-written name.
/// Cleanup removes the file only while it still names this relay's pipe; an
/// overlapping newer relay's target is left alone. With no file, the daemon
/// relays the pane's agent to what the pane would have used without Zosh.
pub(super) struct ForwardedAgentTarget {
    path: PathBuf,
    target: String,
}

impl ForwardedAgentTarget {
    #[cfg_attr(
        not(windows),
        allow(
            dead_code,
            reason = "only a Windows relay installs one; tests on other hosts publish directly"
        )
    )]
    pub(super) fn install(pane_id: u64) -> Result<Option<Self>> {
        let Some(target) = std::env::var_os("SSH_AUTH_SOCK") else {
            return Ok(None);
        };
        Self::publish(
            target.to_string_lossy().into_owned(),
            crate::paths::pane_forwarded_agent_target(pane_id),
        )
    }

    /// Publishes `target` at `path`, unless it is not a local named pipe or is
    /// a daemon's own pane pipe — relaying a pane's agent to itself would loop.
    pub(super) fn publish(target: String, path: PathBuf) -> Result<Option<Self>> {
        if !target.starts_with(r"\\.\pipe\")
            || target.starts_with(crate::paths::PANE_AGENT_PIPE_PREFIX)
        {
            return Ok(None);
        }
        replace_file(&path, &target).context("publishing the forwarded-agent target")?;
        Ok(Some(Self { path, target }))
    }
}

impl Drop for ForwardedAgentTarget {
    fn drop(&mut self) {
        if fs::read_to_string(&self.path).is_ok_and(|current| current == self.target) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn replace_file(path: &Path, contents: &str) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, contents)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
