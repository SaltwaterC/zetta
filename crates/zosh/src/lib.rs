//! `zosh`, Zetta's standalone cross-platform Mosh client.
//!
//! The crate owns both halves of the Mosh launcher contract: it bootstraps a
//! remote Mosh server over SSH and speaks the Mosh State Synchronization
//! Protocol over UDP with its bundled terminal frontend.

mod client;
mod display;
mod escape;
mod launcher;
#[cfg(unix)]
mod locale;
mod notification;
mod scrollback;
mod terminal;

pub use client::ClientArgs;
pub use escape::{EscapeAction, EscapeKey, EscapeState};

/// Run the standalone Mosh-compatible launcher.
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
