//! The `zetta mosh` compatibility proxy.
//!
//! The full Mosh command belongs to the bundled `zosh` executable. Keeping
//! this process as a transparent handoff is important: `zosh` must see the
//! same option spelling, target, and remote command that a user supplied to
//! `zetta mosh`.
//!
//! The arguments are checked with `zosh`'s own parser before the handoff, so
//! a mistake is reported by `zetta` and every option `zosh` accepts is
//! accepted here. A copy of that parser used to live in this module and fell
//! behind it: `--scrollback` was refused by `zetta mosh` while `zosh` took it.

use std::{
    env,
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Context as _, Result};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MoshCommand {
    /// The exact argument vector after the `mosh` subcommand, forwarded
    /// untouched to `zosh`.
    pub(crate) raw_arguments: Vec<OsString>,
}

/// Checks a `zetta mosh` command line the way `zosh` will read it.
pub(crate) fn parse_mosh_args(arguments: &[OsString]) -> Result<MoshCommand> {
    // Without the client there is no parser to check against, and nothing
    // bundled to forward to either; `run` reports the missing executable.
    #[cfg(feature = "zosh-client")]
    zosh::parse_launcher_args(arguments.iter().cloned())?;
    Ok(MoshCommand {
        raw_arguments: arguments.to_vec(),
    })
}

/// Forward `zetta mosh` to the bundled `zosh` process without changing the
/// Mosh command's semantics or target handling.
pub(crate) fn run(command: &MoshCommand) -> Result<()> {
    let zosh = resolve_zosh()?;
    let mut process = Command::new(&zosh);
    process
        .args(&command.raw_arguments)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        let error = process.exec();
        Err(error).with_context(|| format!("starting bundled zosh {}", zosh.display()))
    }
    #[cfg(not(unix))]
    {
        let status = process
            .status()
            .with_context(|| format!("starting bundled zosh {}", zosh.display()))?;
        if status.success() {
            Ok(())
        } else {
            anyhow::bail!("zosh exited with {status}")
        }
    }
}

fn resolve_zosh() -> Result<PathBuf> {
    let current = env::current_exe().context("locating the running zetta executable")?;
    if let Some(parent) = current.parent() {
        let sibling = parent.join(if cfg!(windows) { "zosh.exe" } else { "zosh" });
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from(if cfg!(windows) {
        "zosh.exe"
    } else {
        "zosh"
    }))
}

#[cfg(test)]
#[path = "tests/mosh.rs"]
mod tests;
