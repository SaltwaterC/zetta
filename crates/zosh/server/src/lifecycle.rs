#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::io::{BufRead, BufReader, Write};

#[cfg(unix)]
pub fn detach_after_connect_line() -> Result<()> {
    use std::ffi::CString;

    // This runs before any worker thread or PTY child exists. fork(2) is
    // therefore used in the conventional single-threaded daemonization window.
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            bail!("fork failed: {}", std::io::Error::last_os_error());
        }
        if pid > 0 {
            libc::_exit(0);
        }

        if libc::setsid() < 0 {
            bail!("setsid failed: {}", std::io::Error::last_os_error());
        }

        // A second fork prevents the long-lived server from becoming a session
        // leader that could accidentally acquire a controlling terminal.
        let pid = libc::fork();
        if pid < 0 {
            bail!("second fork failed: {}", std::io::Error::last_os_error());
        }
        if pid > 0 {
            libc::_exit(0);
        }

        let devnull = CString::new("/dev/null").unwrap();
        let fd = libc::open(devnull.as_ptr(), libc::O_RDWR);
        if fd < 0 {
            bail!(
                "open(/dev/null) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            if libc::dup2(fd, target) < 0 {
                let err = std::io::Error::last_os_error();
                libc::close(fd);
                bail!("dup2(/dev/null) failed: {err}");
            }
        }
        if fd > libc::STDERR_FILENO {
            libc::close(fd);
        }
    }
    Ok(())
}

/// On Windows, stock mosh bootstrap needs the SSH-side helper to terminate
/// after printing MOSH CONNECT while the actual UDP server remains alive.
/// Re-exec the server using detached process creation and relay its first line.
#[cfg(windows)]
pub fn windows_parent_bootstrap(raw_args: &[OsString]) -> Result<bool> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    if has_lifecycle_flag(raw_args, "--foreground")
        || has_lifecycle_flag(raw_args, "--no-detach")
        || has_lifecycle_flag(raw_args, "--internal-child")
    {
        return Ok(false);
    }

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

    let exe = std::env::current_exe().context("locating current mosh-server executable")?;

    let spawn = |flags: u32| -> std::io::Result<std::process::Child> {
        let mut cmd = Command::new(&exe);
        cmd.arg("--internal-child")
            .args(raw_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        cmd.creation_flags(flags);
        cmd.spawn()
    };

    // OpenSSH for Windows commonly uses a job object for session cleanup.
    // Prefer breaking away so closing SSH does not kill the Mosh UDP session.
    // Fall back for environments where breakaway is disallowed (manual launch,
    // service managers, and some sshd configurations).
    let mut child =
        match spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB) {
            Ok(child) => child,
            Err(_) => spawn(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
                .context("spawning detached Windows mosh-server child")?,
        };

    let stdout = child
        .stdout
        .take()
        .context("detached child did not expose bootstrap stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .context("reading MOSH CONNECT from detached child")?;
    if n == 0 || !line.starts_with("MOSH CONNECT ") {
        let _ = child.kill();
        bail!("detached mosh-server child exited before emitting MOSH CONNECT");
    }

    print!("{line}");
    std::io::stdout().flush().context("flushing MOSH CONNECT")?;
    // Dropping Child does not terminate the child process. The child owns the
    // UDP session; this bootstrap process can now exit and let SSH close.
    Ok(true)
}

#[cfg(windows)]
fn has_lifecycle_flag(args: &[OsString], needle: &str) -> bool {
    let mut command = false;
    for arg in args {
        if !command && arg == "--" {
            command = true;
            continue;
        }
        if !command && arg == needle {
            return true;
        }
    }
    false
}
