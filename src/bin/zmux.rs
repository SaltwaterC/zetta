//! The multiplexer executable built alongside `zetta`.
//!
//! `zetta mux` dispatches to the same [`zmux::run_with_defaults`] with the same
//! defaults, so the two entry points cannot drift apart — including in what they
//! resolve from Zetta's configuration, which `zmux` itself deliberately knows
//! nothing about.

#[cfg(feature = "session-persistence")]
#[path = "../mux_identity.rs"]
mod mux_identity;

fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Err(error) = zmux::run_with_defaults(&arguments, client_defaults(&arguments)) {
        eprintln!("zmux: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(feature = "session-persistence")]
fn client_defaults(arguments: &[std::ffi::OsString]) -> zmux::ClientDefaults {
    if !mux_identity::command_uses_an_identity(arguments) {
        return zmux::ClientDefaults::default();
    }
    zmux::ClientDefaults {
        identity_paths: mux_identity::configured_identity_paths(None),
    }
}

#[cfg(not(feature = "session-persistence"))]
fn client_defaults(_: &[std::ffi::OsString]) -> zmux::ClientDefaults {
    zmux::ClientDefaults::default()
}
