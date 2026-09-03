fn main() {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let Err(error) = zmux::run_with_defaults(&arguments, client_defaults()) {
        eprintln!("zmux: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(feature = "session-persistence")]
fn client_defaults() -> zmux::ClientDefaults {
    zmux::ClientDefaults {
        identity_paths: zmux::persistence::default_identity_path()
            .into_iter()
            .collect(),
    }
}

#[cfg(not(feature = "session-persistence"))]
fn client_defaults() -> zmux::ClientDefaults {
    zmux::ClientDefaults::default()
}
