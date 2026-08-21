//! The standalone Windows pseudoconsole host.

#[cfg(windows)]
fn main() {
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
