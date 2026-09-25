//! Validated `zetta copy` and `zetta paste` proxies for the standalone clipboard tools.
use anyhow::{Context as _, Result};
use std::ffi::OsString;
use std::process::Command;

use super::CliServiceCommand;

pub(crate) fn copy_help() -> String {
    zclip::copy_help().replace("Usage: zcopy", "Usage: zetta copy")
}

pub(crate) fn paste_help() -> String {
    zclip::paste_help().replace("Usage: zpaste", "Usage: zetta paste")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CopyCommand {
    args: Vec<OsString>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PasteCommand {
    args: Vec<OsString>,
}

pub(crate) fn parse_copy_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args: Vec<_> = args.into_iter().collect();
    zclip::parse_copy_args(args.iter().cloned())?;
    Ok(CliServiceCommand::Copy(CopyCommand { args }))
}

pub(crate) fn parse_paste_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<CliServiceCommand> {
    let args: Vec<_> = args.into_iter().collect();
    zclip::parse_paste_args(args.iter().cloned())?;
    Ok(CliServiceCommand::Paste(PasteCommand { args }))
}

fn run_helper(name: &str, args: &[OsString]) -> Result<()> {
    let executable = std::env::current_exe().context("locating the zetta executable")?;
    let path = executable.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    run_helper_at(&path, name, args)
}

fn run_helper_at(path: &std::path::Path, name: &str, args: &[OsString]) -> Result<()> {
    let status = Command::new(path).args(args).status().with_context(|| {
        format!(
            "starting {name} at {}; install the clipboard helpers beside zetta",
            path.display()
        )
    })?;
    anyhow::ensure!(status.success(), "{name} failed with {status}");
    Ok(())
}

impl CopyCommand {
    pub(super) fn run(&self) -> Result<()> {
        run_helper("zcopy", &self.args)
    }
}
impl PasteCommand {
    pub(super) fn run(&self) -> Result<()> {
        run_helper("zpaste", &self.args)
    }
}

#[cfg(test)]
#[path = "../tests/cli_services/clipboard.rs"]
mod tests;
