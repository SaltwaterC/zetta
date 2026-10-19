//! HTTP CLI proxy and the shared engine used by server log panes.
use std::ffi::OsString;

use anyhow::{Context as _, Result};

pub(crate) use zhttp::{
    HttpServerOptions, OpenHttpServer, http_server_limits_help, start_http_server,
};

pub(crate) fn run_http_proxy(args: &[OsString]) -> Result<()> {
    let executable = std::env::current_exe().context("locating the zetta executable")?;
    let path = executable.with_file_name(format!("zhttp{}", std::env::consts::EXE_SUFFIX));
    let status = std::process::Command::new(&path)
        .args(args)
        .status()
        .with_context(|| {
            format!(
                "starting zhttp at {}; install zhttp beside zetta",
                path.display()
            )
        })?;
    anyhow::ensure!(status.success(), "zhttp failed with {status}");
    Ok(())
}
