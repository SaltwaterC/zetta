//! TFTP CLI proxies and the shared engine used by server log panes.
use anyhow::{Context as _, Result};
use std::ffi::OsString;

#[cfg(feature = "tftp-server")]
pub(crate) use ztftp::{OpenTftpServer, start_server};

#[cfg(feature = "tftp-client")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TftpCommand {
    pub(crate) args: Vec<OsString>,
}

#[cfg(feature = "tftp-client")]
pub(crate) fn tftp_help() -> String {
    ztftp::tftp_help().replace("ztftp", "zetta tftp")
}

#[cfg(feature = "tftp-client")]
pub(crate) fn parse_tftp_args(args: impl IntoIterator<Item = OsString>) -> Result<TftpCommand> {
    let args: Vec<_> = args.into_iter().collect();
    ztftp::parse_tftp_args(args.iter().cloned())?;
    Ok(TftpCommand { args })
}

#[cfg(feature = "tftp-client")]
impl TftpCommand {
    pub(crate) fn run(&self) -> Result<()> {
        run_tftp_proxy(&self.args)
    }
}

pub(crate) fn run_tftp_proxy(args: &[OsString]) -> Result<()> {
    let executable = std::env::current_exe().context("locating the zetta executable")?;
    let path = executable.with_file_name(format!("ztftp{}", std::env::consts::EXE_SUFFIX));
    let status = std::process::Command::new(&path)
        .args(args)
        .status()
        .with_context(|| {
            format!(
                "starting ztftp at {}; install ztftp beside zetta",
                path.display()
            )
        })?;
    anyhow::ensure!(status.success(), "ztftp failed with {status}");
    Ok(())
}
