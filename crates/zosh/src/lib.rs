//! `zosh`, Zetta's standalone cross-platform Zosh client.
//!
//! The crate owns both halves of the Mosh launcher contract: it bootstraps a
//! remote Mosh server over SSH and speaks the Mosh State Synchronization
//! Protocol over UDP with its bundled terminal frontend.
//!
//! It is also a library, for an application that wants a Mosh session without
//! a terminal to run it in: [`bootstrap_pane_endpoint`] performs the SSH half
//! in front of an arbitrary remote command, and [`PaneSession`] speaks the
//! protocol and renders it into a byte stream the caller feeds to its own
//! emulator. Zetta uses the pair to carry a remote pane's output over Mosh
//! while its control traffic stays on SSH.

mod client;
mod display;
mod escape;
mod frame;
mod launcher;
#[cfg(unix)]
mod locale;
mod notification;
mod scrollback;
mod stream;
mod terminal;

pub use client::ClientArgs;
pub use escape::{EscapeAction, EscapeKey, EscapeState};
pub use launcher::{
    PaneBootstrapOutcome, PaneBootstrapRequest, PaneEndpoint, bootstrap_pane_endpoint,
};
pub use mosh_rs::{Base64Key, DisplayPreference};
pub use stream::{PaneReader, PaneSession, PaneSessionSettings, PaneWriter};

/// The keep-alive interval `-k` asks for when it is given no value.
pub const KEEP_ALIVE_DEFAULT_MS: u64 = mosh_rs::sender::KEEP_ALIVE_DEFAULT_MS;
/// The smallest interval a keep-alive can be held to: below Mosh's own
/// minimum frame interval, one cannot go out any sooner.
pub const KEEP_ALIVE_MIN_MS: u64 = client::KEEP_ALIVE_MIN_MS;
/// The largest: above Mosh's unassisted heartbeat there is nothing left to
/// ask for.
pub const KEEP_ALIVE_MAX_MS: u64 = client::KEEP_ALIVE_MAX_MS;

/// Parses a `--keep-alive=MS` value, in milliseconds.
///
/// This is the one place the bounds are enforced, so every command and
/// configuration surface that carries the setting rejects the same values.
pub fn parse_keep_alive_interval(value: &str) -> anyhow::Result<u64> {
    client::parse_keep_alive_interval(value)
}

/// Run the standalone Zosh launcher.
pub fn run(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> anyhow::Result<()> {
    launcher::run(arguments)
}

/// Entry point used by the standalone `zosh` binary.
pub fn standalone_main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let result = if arguments
        .first()
        .is_some_and(|argument| argument == "--endpoint")
    {
        client::run_endpoint(arguments.into_iter().skip(1))
    } else if arguments.len() == 1 && arguments[0] == "-c" {
        // Keep the mosh-client colour probe available to integrations and
        // older callers even though the normal zosh command is the full
        // launcher.
        client::run_endpoint(arguments)
    } else if endpoint_shape(&arguments) && std::env::var_os("MOSH_KEY").is_some() {
        // Preserve the old bundled-endpoint invocation for integrations that
        // already have a MOSH CONNECT response and key.
        client::run_endpoint(arguments)
    } else {
        run(arguments)
    };
    if let Err(error) = result {
        eprintln!("zosh failed: {error:#}");
        std::process::exit(1);
    }
}

fn endpoint_shape(arguments: &[std::ffi::OsString]) -> bool {
    arguments.len() == 2
        && !arguments[0].to_string_lossy().starts_with('-')
        && arguments[1]
            .to_string_lossy()
            .parse::<u16>()
            .is_ok_and(|port| port != 0)
}

#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;

#[cfg(all(test, unix))]
#[path = "tests/interop.rs"]
mod interop_tests;
