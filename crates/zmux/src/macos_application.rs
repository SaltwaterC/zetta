//! Hosts a packaged Mac daemon in the application's main executable.
//!
//! A standalone helper inherited the GUI's privacy identity. Once that GUI
//! exited and its image was replaced, macOS could no longer resolve it and
//! denied LAN traffic from surviving shells. `zetta mux --daemon` has the
//! application's bundle identity and establishes its own responsibility, so
//! its identity remains live for as long as its shells do. Unbundled remote
//! installations continue to use the small standalone daemon.

use std::{
    ffi::OsString,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

pub(crate) fn executable(current: &Path) -> Option<PathBuf> {
    let directory = current.parent()?;
    let contents = directory.parent()?;
    let application = directory.join("zetta");
    (directory.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && contents.parent()?.extension()? == "app"
        && contents.join("Info.plist").is_file()
        && application.is_file())
    .then_some(application)
}

pub(crate) fn is_application(path: &Path) -> bool {
    executable(path).is_some_and(|application| application == path)
}

/// Also migrates an older standalone daemon's exec handover without changing
/// its PID or closing the PTYs and handover descriptors it inherited.
pub(crate) fn host_daemon(arguments: &[OsString]) -> Result<()> {
    if !arguments.iter().any(|argument| argument == "--daemon") {
        return Ok(());
    }
    let current = std::env::current_exe()?;
    let Some(application) = executable(&current).filter(|application| application != &current)
    else {
        return Ok(());
    };
    let error = std::process::Command::new(&application)
        .arg("mux")
        .args(arguments)
        .exec();
    Err(error).with_context(|| format!("hosting the multiplexer in {}", application.display()))
}

#[cfg(test)]
#[path = "tests/macos_application.rs"]
mod tests;
