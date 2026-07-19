#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
use std::{
    env, io,
    os::windows::process::CommandExt as _,
    path::{Path, PathBuf},
    process::Command,
};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[cfg(windows)]
fn cli_executable(gui_executable: &Path) -> PathBuf {
    gui_executable.with_file_name("zetta.exe")
}

#[cfg(windows)]
fn launch() -> io::Result<i32> {
    let gui_executable = env::current_exe()?;
    let status = Command::new(cli_executable(&gui_executable))
        .args(env::args_os().skip(1))
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    Ok(status.code().unwrap_or(1))
}

#[cfg(windows)]
fn main() {
    std::process::exit(launch().unwrap_or(1));
}

#[cfg(not(windows))]
fn main() {}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn cli_executable_is_next_to_the_gui_launcher() {
        assert_eq!(
            cli_executable(Path::new(r"C:\Program Files\Zetta\zetta-gui.exe")),
            Path::new(r"C:\Program Files\Zetta\zetta.exe")
        );
    }
}
