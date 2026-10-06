//! `wslx.exe`: `wsl.exe`, with the Windows SSH agent carried into the
//! distribution.
//!
//! A Zosh pane on a Windows host is given its forwarded agent as a named pipe
//! in `SSH_AUTH_SOCK`. `wsl.exe` cannot hand that to a WSL2 distribution: the
//! distribution is a virtual machine, which cannot open a Windows pipe, and
//! `ssh` inside it only speaks to a Unix socket, which Windows cannot create
//! there. `wslx.exe` closes that gap and otherwise stays out of the way — every
//! argument reaches `wsl.exe` untouched, and its exit status is `wsl.exe`'s.
//!
//! When the arguments start a session (`args.rs`) and `SSH_AUTH_SOCK` names a
//! pipe (`environment.rs`), `wslx.exe` first starts a second, windowless
//! `wsl.exe` in the same distribution as the same user. That one runs
//! `bootstrap.rs`'s script, which execs the Linux relay (`relay.rs`) — copied
//! into the user's cache over the same stdio the first time a build is used.
//! The relay listens on a private Unix socket and carries each agent
//! connection back over its stdio (`protocol.rs`) to `wslx.exe`, which answers
//! it from the pipe (`bridge.rs`). Only once the socket exists is the real
//! `wsl.exe` started, with `SSH_AUTH_SOCK` pointing at it and listed in
//! `WSLENV` so WSL passes it through.
//!
//! Anything that keeps the agent from being carried is reported on stderr and
//! the session starts without it: `wslx.exe` must never be the reason `wsl.exe`
//! did not run.

pub mod args;
pub mod bootstrap;
pub mod bridge;
pub mod environment;
pub mod help;
pub mod protocol;
#[cfg(unix)]
pub mod relay;

#[cfg(windows)]
mod launcher;
#[cfg(windows)]
pub use launcher::run;

use std::sync::{Mutex, MutexGuard};

/// Every mutex here guards either a writer, whose half-written message the
/// peer rejects on its own, or a map of connections, which a panicking
/// connection thread cannot leave inconsistent; so a poisoned lock is simply
/// taken.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "tests/support.rs"]
mod test_support;
