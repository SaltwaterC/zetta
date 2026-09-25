//! Host clipboard I/O for requests arriving from an interactive child.
//!
//! The sibling helpers own platform-specific clipboard behavior. The guard
//! keeps them from probing their own terminal while serving a request.

use anyhow::{Context as _, Result, ensure};
use std::{
    io::Write as _,
    process::{Command, Stdio},
};

fn sibling(name: &str) -> Result<std::path::PathBuf> {
    let mut path = std::env::current_exe().context("locating Zetta executable")?;
    path.set_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    Ok(path)
}

pub(super) fn copy(text: &str) -> Result<()> {
    let mut child = Command::new(sibling("zcopy")?)
        .env("ZCLIP_HOST_BACKEND", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("starting host zcopy")?;
    child
        .stdin
        .take()
        .context("host zcopy has no standard input")?
        .write_all(text.as_bytes())
        .context("sending text to host zcopy")?;
    let status = child.wait().context("waiting for host zcopy")?;
    ensure!(status.success(), "host zcopy failed: {status}");
    Ok(())
}

pub(super) fn paste() -> Result<Option<String>> {
    let output = Command::new(sibling("zpaste")?)
        .env("ZCLIP_HOST_BACKEND", "1")
        .output()
        .context("running host zpaste")?;
    ensure!(
        output.status.success(),
        "host zpaste failed: {}",
        output.status
    );
    let text = String::from_utf8(output.stdout).context("host clipboard is not UTF-8 text")?;
    Ok(Some(text))
}
