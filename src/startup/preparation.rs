//! Resolves launch configuration while Windows initializes its graphics platform.
//! Configuration and profile discovery need no GPUI state; platform creation
//! stays on the caller's thread because it initializes the Windows OLE apartment.

use anyhow::{Context as _, Result, anyhow};
use std::thread;

pub(super) fn prepare_in_parallel<P, L: Send>(
    prepare: impl FnOnce() -> Result<L> + Send,
    platform: impl FnOnce() -> P,
) -> Result<(P, L)> {
    thread::scope(|scope| {
        let pending = thread::Builder::new()
            .name("launch preparation".to_owned())
            .spawn_scoped(scope, prepare)
            .context("could not start launch preparation")?;
        let platform = platform();
        let launch = pending
            .join()
            .map_err(|_| anyhow!("launch preparation panicked"))??;
        Ok((platform, launch))
    })
}

#[cfg(test)]
#[path = "../tests/startup/preparation.rs"]
mod tests;
