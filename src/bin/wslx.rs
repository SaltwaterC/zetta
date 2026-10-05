//! `wslx.exe`: `wsl.exe`, with the SSH agent of the pane it runs in carried
//! into the distribution. See `crates/wslx`.

#[cfg(windows)]
fn main() {
    std::process::exit(wslx::run());
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wslx is only supported on Windows");
    std::process::exit(1);
}
