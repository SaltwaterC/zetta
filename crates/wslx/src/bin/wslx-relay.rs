//! The relay `wslx.exe` starts inside a WSL distribution; see `wslx::relay`.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    wslx::relay::main()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("wslx-relay runs inside a WSL distribution and is built for Linux");
    std::process::exit(1);
}
