#[cfg(windows)]
use anyhow::Context;
use anyhow::{Result, bail};
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::io::{BufRead, BufReader, Write};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use std::os::windows::io::FromRawHandle;

#[cfg(unix)]
pub fn detach_after_connect_line() -> Result<()> {
    use std::ffi::CString;

    // This runs before any worker thread or PTY child exists. fork(2) is
    // therefore used in the conventional single-threaded daemonization window.
    unsafe {
        // The parent exits the moment it has forked, and it is the session
        // leader SSH started the bootstrap as: the kernel hangs up that
        // session's process group as it goes. Until the child below has
        // called setsid it is still in that group, and a hangup there killed
        // it a few times in every hundred — after MOSH CONNECT had already
        // gone out, so the client sent its datagrams to a port nobody held
        // and the pane it was for stayed blank. Ignored across the window and
        // restored once this process has a session of its own, because an
        // ignored disposition survives exec and the pane's programs must not
        // inherit it.
        let hangup = libc::signal(libc::SIGHUP, libc::SIG_IGN);
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
        if hangup != libc::SIG_ERR {
            libc::signal(libc::SIGHUP, hangup);
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
    if has_lifecycle_flag(raw_args, "--foreground")
        || has_lifecycle_flag(raw_args, "--no-detach")
        || has_lifecycle_flag(raw_args, "--internal-child")
    {
        return Ok(false);
    }

    let exe = std::env::current_exe().context("locating current zosh-server executable")?;
    let child_args = std::iter::once(OsString::from("--internal-child"))
        .chain(raw_args.iter().cloned())
        .collect::<Vec<_>>();
    let (stdout, child) = spawn_detached_process(&exe, &child_args)?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let n = match reader.read_line(&mut line) {
        Ok(n) => n,
        Err(error) => {
            child.terminate();
            return Err(error).context("reading MOSH CONNECT from detached child");
        }
    };
    if n == 0 || !line.starts_with("MOSH CONNECT ") {
        child.terminate();
        bail!("detached zosh-server child exited before emitting MOSH CONNECT");
    }

    print!("{line}");
    std::io::stdout().flush().context("flushing MOSH CONNECT")?;
    // Dropping Child does not terminate the child process. The child owns the
    // UDP session; this bootstrap process can now exit and let SSH close.
    Ok(true)
}

#[cfg(windows)]
struct DetachedChild {
    process: windows::Win32::Foundation::HANDLE,
    thread: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl DetachedChild {
    fn terminate(&self) {
        use windows::Win32::System::Threading::TerminateProcess;
        let _ = unsafe { TerminateProcess(self.process, 1) };
    }
}

#[cfg(windows)]
impl Drop for DetachedChild {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        let _ = unsafe { CloseHandle(self.process) };
        let _ = unsafe { CloseHandle(self.thread) };
    }
}

#[cfg(windows)]
fn spawn_detached_process(
    exe: &std::path::Path,
    args: &[OsString],
) -> Result<(std::fs::File, DetachedChild)> {
    use windows::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
    use windows::Win32::Security::SECURITY_ATTRIBUTES;
    use windows::Win32::System::Pipes::CreatePipe;
    use windows::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
        STARTUPINFOEXW, UpdateProcThreadAttribute,
    };
    use windows::core::{PCWSTR, PWSTR};

    let mut pipe_read = HANDLE::default();
    let mut pipe_write = HANDLE::default();
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: true.into(),
        ..SECURITY_ATTRIBUTES::default()
    };
    unsafe { CreatePipe(&mut pipe_read, &mut pipe_write, Some(&security), 0) }
        .context("creating detached server bootstrap pipe")?;
    let stdout = unsafe { std::fs::File::from_raw_handle(pipe_read.0) };
    let pipe_writer = unsafe { std::fs::File::from_raw_handle(pipe_write.0) };
    let stdin = std::fs::OpenOptions::new().read(true).open("NUL")?;
    let stderr = std::fs::OpenOptions::new().write(true).open("NUL")?;
    for file in [&stdin, &stderr] {
        unsafe {
            SetHandleInformation(
                HANDLE(file.as_raw_handle()),
                HANDLE_FLAG_INHERIT.0,
                HANDLE_FLAG_INHERIT,
            )
        }
        .context("making detached server NUL handles inheritable")?;
    }

    let handles = [
        HANDLE(stdin.as_raw_handle()),
        HANDLE(pipe_writer.as_raw_handle()),
        HANDLE(stderr.as_raw_handle()),
    ];
    let mut attribute_bytes = 0;
    let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut attribute_bytes) };
    anyhow::ensure!(
        attribute_bytes != 0,
        "detached server handle list has no size"
    );
    let mut storage = vec![0usize; attribute_bytes.div_ceil(std::mem::size_of::<usize>())];
    let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
    unsafe { InitializeProcThreadAttributeList(Some(attributes), 1, None, &mut attribute_bytes) }
        .context("initializing detached server handle list")?;
    let spawn_result: Result<DetachedChild> = (|| {
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(&handles),
                None,
                None,
            )
        }
        .context("restricting detached server inherited handles")?;
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes;
        let exe_wide = wide_null(exe.as_os_str());
        let mut command_line = child_command_line(exe.as_os_str(), args);
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | EXTENDED_STARTUPINFO_PRESENT;
        let mut process = PROCESS_INFORMATION::default();
        let mut spawn = |flags, process: &mut PROCESS_INFORMATION| unsafe {
            CreateProcessW(
                PCWSTR(exe_wide.as_ptr()),
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                true,
                flags,
                None,
                PCWSTR::null(),
                &startup.StartupInfo,
                process,
            )
        };
        // OpenSSH uses a job for session cleanup. A child left in that job
        // cannot outlive the bootstrap; a local launch can use the fallback.
        if let Err(error) = spawn(flags | CREATE_BREAKAWAY_FROM_JOB, &mut process) {
            anyhow::ensure!(
                std::env::var_os("SSH_CONNECTION").is_none(),
                "Windows SSH job prevents detaching zosh-server: {error}"
            );
            spawn(flags, &mut process).context("spawning detached Windows zosh-server child")?;
        }
        Ok(DetachedChild {
            process: process.hProcess,
            thread: process.hThread,
        })
    })();
    unsafe { DeleteProcThreadAttributeList(attributes) };
    let child = spawn_result?;
    drop(pipe_writer);
    Ok((stdout, child))
}

#[cfg(windows)]
fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn child_command_line(exe: &std::ffi::OsStr, args: &[OsString]) -> Vec<u16> {
    let mut result = Vec::new();
    for word in std::iter::once(exe).chain(args.iter().map(OsString::as_os_str)) {
        if !result.is_empty() {
            result.push(b' ' as u16);
        }
        result.extend(quote_windows_word(word));
    }
    result.push(0);
    result
}

#[cfg(windows)]
fn quote_windows_word(word: &std::ffi::OsStr) -> Vec<u16> {
    let units = word.encode_wide().collect::<Vec<_>>();
    let mut quoted = vec![b'"' as u16];
    let mut slashes = 0;
    for unit in units {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2 + 1));
        } else {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes));
        }
        quoted.push(unit);
        slashes = 0;
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    quoted.push(b'"' as u16);
    quoted
}

/// Release the bootstrap pipe after the child has printed its endpoint.
/// Keeping it open makes SSH wait for the entire UDP session to end.
#[cfg(windows)]
pub fn release_bootstrap_stdout() -> Result<std::fs::File> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE, SetStdHandle};

    let null = std::fs::OpenOptions::new()
        .write(true)
        .open("NUL")
        .context("opening NUL for detached server stdout")?;
    let original = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
        .context("reading detached server stdout handle")?;
    unsafe {
        SetStdHandle(
            STD_OUTPUT_HANDLE,
            windows::Win32::Foundation::HANDLE(null.as_raw_handle()),
        )
    }
    .context("redirecting detached server stdout")?;
    unsafe { CloseHandle(original) }.context("closing detached server bootstrap pipe")?;
    Ok(null)
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

#[cfg(test)]
#[path = "tests/lifecycle.rs"]
mod tests;
