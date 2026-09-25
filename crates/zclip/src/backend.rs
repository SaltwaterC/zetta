//! System clipboard I/O for the two standalone programs.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::CLIPBOARD_DAEMON_FLAG;
use anyhow::{Context as _, Result};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::env;
use std::io::{self, Read as _, Write as _};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn spawn_clipboard_copy_daemon(text: String) -> Result<()> {
    use std::os::unix::process::CommandExt as _;
    use std::process::Stdio;

    let executable = env::current_exe().context("locating the zcopy executable")?;
    let mut command = std::process::Command::new(executable);
    command
        .arg(CLIPBOARD_DAEMON_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir("/");
    // SAFETY: setsid(2) is async-signal-safe and is the only call made in the forked child
    // before it execs; detaching into its own session keeps the clipboard daemon alive after
    // this shell's session, and its controlling terminal, goes away.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut daemon = command.spawn().context("spawning the clipboard daemon")?;
    daemon
        .stdin
        .take()
        .context("the clipboard daemon did not provide a standard input pipe")?
        .write_all(text.as_bytes())
        .context("sending clipboard contents to the daemon")?;
    Ok(())
}

fn run_clipboard_copy_daemon() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut input)
        .context("reading standard input")?;
    let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use arboard::SetExtLinux as _;
        clipboard
            .set()
            .wait()
            .text(input)
            .context("serving the system clipboard")?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    clipboard
        .set_text(input)
        .context("writing to the system clipboard")?;
    Ok(())
}

fn run_copy() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_to_string(&mut input)
        .context("reading standard input")?;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        spawn_clipboard_copy_daemon(input)
    }
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    {
        let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
        clipboard
            .set_text(input)
            .context("writing to the system clipboard")?;
        Ok(())
    }
}

fn run_paste() -> Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("opening the system clipboard")?;
    if let Some(text) = clipboard_text(clipboard.get_text())? {
        io::stdout()
            .write_all(text.as_bytes())
            .context("writing the clipboard contents to standard output")?;
    }
    Ok(())
}

pub fn copy(daemon: bool) -> Result<()> {
    if daemon {
        run_clipboard_copy_daemon()
    } else {
        run_copy()
    }
}

pub fn paste() -> Result<()> {
    run_paste()
}

fn clipboard_text(result: std::result::Result<String, arboard::Error>) -> Result<Option<String>> {
    match result {
        Ok(text) => Ok(Some(text)),
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(error) => Err(error).context("reading the system clipboard"),
    }
}

#[cfg(test)]
#[path = "tests/backend.rs"]
mod tests;
