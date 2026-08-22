//! The standalone Windows pseudoconsole host.

#[cfg(windows)]
fn main() {
    if let Some(code) = zmux::pty_host::run_palette_bootstrap_from_env() {
        std::process::exit(code);
    }
    if let Some(code) = zmux::pty_host::run_palette_probe_from_env() {
        std::process::exit(code);
    }
    if let Some(code) = zmux::pty_host::run_palette_helper_from_env() {
        std::process::exit(code);
    }
    if let Err(error) = zmux::pty_host::run() {
        eprintln!("zmux-pty: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("zmux-pty is only supported on Windows");
    std::process::exit(1);
}
