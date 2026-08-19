//! TTY related functionality.

use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::File;
use std::io::{Error, ErrorKind, Read, Result, Write};
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::{env, ptr};

use libc::{F_GETFL, F_SETFL, O_NONBLOCK, TIOCSCTTY, c_int, fcntl};
use log::error;
use polling::{Event, PollMode, Poller};
use rustix_openpty::openpty;
use rustix_openpty::rustix::termios::Winsize;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix_openpty::rustix::termios::{self, InputModes, OptionalActions};
use signal_hook::low_level::{pipe as signal_pipe, unregister as unregister_signal};
use signal_hook::{SigId, consts as sigconsts};

use crate::event::{OnResize, WindowSize};
use crate::tty::{ChildEvent, EventedPty, EventedReadWrite, Options};

// Interest in PTY read/writes.
pub(crate) const PTY_READ_WRITE_TOKEN: usize = 0;

// Interest in new child events.
pub(crate) const PTY_CHILD_EVENT_TOKEN: usize = 1;

macro_rules! die {
    ($($arg:tt)*) => {{
        error!($($arg)*);
        std::process::exit(1);
    }};
}

/// Really only needed on BSD, but should be fine elsewhere.
fn set_controlling_terminal(fd: c_int) -> Result<()> {
    let res = unsafe {
        // TIOSCTTY changes based on platform and the `ioctl` call is different
        // based on architecture (32/64). So a generic cast is used to make sure
        // there are no issues. To allow such a generic cast the clippy warning
        // is disabled.
        #[allow(clippy::cast_lossless)]
        libc::ioctl(fd, TIOCSCTTY as _, 0)
    };

    if res == 0 { Ok(()) } else { Err(Error::last_os_error()) }
}

/// Signal mask to apply to a spawned PTY child before exec.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SignalMask(libc::sigset_t);

impl SignalMask {
    /// Capture the calling thread's current signal mask.
    pub fn current() -> Result<Self> {
        let mut signal_set = MaybeUninit::<libc::sigset_t>::uninit();

        // `pthread_sigmask` only writes the kernel-relevant portion of `sigset_t` (e.g. 8 bytes
        // on 64-bit glibc, where `sigset_t` is 128 bytes). Zero the whole struct first so the
        // remaining padding is deterministic, otherwise reading it (in `PartialEq`) would be
        // undefined behavior and could spuriously compare unequal.
        if unsafe { libc::sigemptyset(signal_set.as_mut_ptr()) } != 0 {
            return Err(Error::last_os_error());
        }

        let result = unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, ptr::null(), signal_set.as_mut_ptr())
        };

        if result != 0 {
            Err(Error::from_raw_os_error(result))
        } else {
            Ok(Self(unsafe { signal_set.assume_init() }))
        }
    }

    fn apply(self) -> Result<()> {
        let result = unsafe { libc::sigprocmask(libc::SIG_SETMASK, &self.0, ptr::null_mut()) };

        if result == -1 { Err(Error::last_os_error()) } else { Ok(()) }
    }
}

impl fmt::Debug for SignalMask {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("SignalMask").finish()
    }
}

#[derive(Debug)]
struct Passwd<'a> {
    name: &'a str,
    dir: &'a str,
    shell: &'a str,
}

/// Return a Passwd struct with pointers into the provided buf.
///
/// # Unsafety
///
/// If `buf` is changed while `Passwd` is alive, bad thing will almost certainly happen.
fn get_pw_entry(buf: &mut [i8; 1024]) -> Result<Passwd<'_>> {
    // Create zeroed passwd struct.
    let mut entry: MaybeUninit<libc::passwd> = MaybeUninit::uninit();

    let mut res: *mut libc::passwd = ptr::null_mut();

    // Try and read the pw file.
    let uid = unsafe { libc::getuid() };
    let status = unsafe {
        libc::getpwuid_r(uid, entry.as_mut_ptr(), buf.as_mut_ptr() as *mut _, buf.len(), &mut res)
    };
    let entry = unsafe { entry.assume_init() };

    if status < 0 {
        return Err(Error::other("getpwuid_r failed"));
    }

    if res.is_null() {
        return Err(Error::other("pw not found"));
    }

    // Sanity check.
    assert_eq!(entry.pw_uid, uid);

    // Build a borrowed Passwd struct.
    Ok(Passwd {
        name: unsafe { CStr::from_ptr(entry.pw_name).to_str().unwrap() },
        dir: unsafe { CStr::from_ptr(entry.pw_dir).to_str().unwrap() },
        shell: unsafe { CStr::from_ptr(entry.pw_shell).to_str().unwrap() },
    })
}

pub struct Pty {
    child: PtyChild,
    file: File,
    signals: UnixStream,
    sig_id: Option<SigId>,
}

/// Who owns the process on the far side of the PTY.
enum PtyChild {
    /// Spawned by this process, so `SIGCHLD` plus `waitpid` reports its exit
    /// and dropping the PTY must hang it up.
    Owned(Child),
    /// Spawned by the multiplexer, which passed this process the master file
    /// descriptor. The exit status cannot be reaped here — only the parent may
    /// do that — so it arrives over [`AttachedChildEvents`] instead, and
    /// dropping the PTY must leave the process running for the next client.
    Attached { pid: u32, pending: Vec<u8> },
    /// Spawned by an earlier image of *this* process, which replaced itself
    /// with `execv`. The process ID is still a child of this one — an exec does
    /// not change that — so it is reaped here, but there is no `Child` to do it
    /// through because that was a value in the previous image.
    Reclaimed { pid: u32 },
}

/// One child event, as written into an attached PTY's event socket.
const ATTACHED_EVENT_LEN: usize = 5;
const ATTACHED_EXITED: u8 = 0;
const ATTACHED_STATUS_UNAVAILABLE: u8 = 1;
const ATTACHED_WATCHER_DISCONNECTED: u8 = 2;

/// The reporting end of an attached PTY's child events.
///
/// The event loop only stops on a child event — a read error on the master is
/// deliberately retried, because it cannot distinguish a hangup from a
/// transient failure. An attached PTY therefore has to be told, and whoever
/// holds this is responsible for saying so: either the real exit status, or
/// [`Self::report_watcher_disconnected`] if the connection to the multiplexer
/// is lost before the status arrives.
pub struct AttachedChildEvents(UnixStream);

impl AttachedChildEvents {
    /// Reports the raw `waitpid` status the multiplexer observed.
    pub fn report_exit(&mut self, raw_status: i32) -> Result<()> {
        self.report(ATTACHED_EXITED, raw_status)
    }

    pub fn report_status_unavailable(&mut self) -> Result<()> {
        self.report(ATTACHED_STATUS_UNAVAILABLE, 0)
    }

    pub fn report_watcher_disconnected(&mut self) -> Result<()> {
        self.report(ATTACHED_WATCHER_DISCONNECTED, 0)
    }

    fn report(&mut self, tag: u8, raw_status: i32) -> Result<()> {
        let mut frame = [0; ATTACHED_EVENT_LEN];
        frame[0] = tag;
        frame[1..].copy_from_slice(&raw_status.to_le_bytes());
        self.0.write_all(&frame)
    }
}

impl Pty {
    /// The PTY child's process ID, whether this process spawned it or not.
    pub fn child_pid(&self) -> u32 {
        match &self.child {
            PtyChild::Owned(child) => child.id(),
            PtyChild::Attached { pid, .. } => *pid,
            PtyChild::Reclaimed { pid } => *pid,
        }
    }

    pub fn file(&self) -> &File {
        &self.file
    }

    /// Polls an owned child. An attached one belongs to the multiplexer, which
    /// reports its exit over [`AttachedChildEvents`] instead.
    #[cfg(test)]
    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        match &mut self.child {
            PtyChild::Owned(child) => child.try_wait(),
            PtyChild::Attached { .. } => Ok(None),
            PtyChild::Reclaimed { pid } => {
                let mut status = 0;
                // SAFETY: a reclaimed child is still this process's own, and
                // `status` is writable for the duration of the call.
                match unsafe { libc::waitpid(*pid as i32, &mut status, libc::WNOHANG) } {
                    0 => Ok(None),
                    result if result < 0 => Err(Error::last_os_error()),
                    _ => Ok(Some(ExitStatus::from_raw(status))),
                }
            },
        }
    }
}

/// Rebuilds a PTY this process already owns, after replacing its own image.
///
/// `execv` keeps both the descriptor and the parent/child relationship, so the
/// exit status is still reapable here — unlike [`attach`], where the child
/// belongs to a different process entirely.
pub fn reclaim(master: OwnedFd, child_pid: u32) -> Result<Pty> {
    let master_fd = master.as_raw_fd();
    unsafe {
        set_nonblocking(master_fd)?;
    }
    let (sender, signals) = UnixStream::pair()?;
    let sig_id = signal_pipe::register(sigconsts::SIGCHLD, sender)?;
    signals.set_nonblocking(true)?;
    Ok(Pty {
        child: PtyChild::Reclaimed { pid: child_pid },
        file: File::from(master),
        signals,
        sig_id: Some(sig_id),
    })
}

/// Builds a PTY around a master file descriptor whose child belongs to another
/// process, as the multiplexer hands over on attach.
///
/// Resizing still works from here — `TIOCSWINSZ` applies to any holder of the
/// master — but reaping does not, so the returned [`AttachedChildEvents`] is
/// how the exit status gets in.
pub fn attach(master: OwnedFd, child_pid: u32) -> Result<(Pty, AttachedChildEvents)> {
    let master_fd = master.as_raw_fd();
    unsafe {
        set_nonblocking(master_fd)?;
    }
    let (reporter, events) = UnixStream::pair()?;
    events.set_nonblocking(true)?;
    Ok((
        Pty {
            child: PtyChild::Attached {
                pid: child_pid,
                pending: Vec::with_capacity(ATTACHED_EVENT_LEN),
            },
            file: File::from(master),
            signals: events,
            sig_id: None,
        },
        AttachedChildEvents(reporter),
    ))
}

/// User information that is required for a new shell session.
struct ShellUser {
    user: String,
    home: String,
    shell: String,
}

impl ShellUser {
    /// look for shell, username, longname, and home dir in the respective environment variables
    /// before falling back on looking into `passwd`.
    fn from_env() -> Result<Self> {
        let mut buf = [0; 1024];
        let pw = get_pw_entry(&mut buf);

        let user = match env::var("USER") {
            Ok(user) => user,
            Err(_) => match pw {
                Ok(ref pw) => pw.name.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let home = match env::var("HOME") {
            Ok(home) => home,
            Err(_) => match pw {
                Ok(ref pw) => pw.dir.to_owned(),
                Err(err) => return Err(err),
            },
        };

        let shell = match env::var("SHELL") {
            Ok(shell) => shell,
            Err(_) => match pw {
                Ok(ref pw) => pw.shell.to_owned(),
                Err(err) => return Err(err),
            },
        };

        Ok(Self { user, home, shell })
    }
}

#[cfg(not(target_os = "macos"))]
fn default_shell_command(shell: &str, _user: &str, _home: &str) -> Command {
    Command::new(shell)
}

#[cfg(target_os = "macos")]
fn default_shell_command(shell: &str, user: &str, home: &str) -> Command {
    let shell_name = shell.rsplit('/').next().unwrap();

    // On macOS, use the `login` command so the shell will appear as a tty session.
    let mut login_command = Command::new("/usr/bin/login");

    // Exec the shell with argv[0] prepended by '-' so it becomes a login shell.
    // `login` normally does this itself, but `-l` disables this.
    let exec = format!("exec -a -{} {}", shell_name, shell);

    // Since we use -l, `login` will not change directory to the user's home. However,
    // `login` only checks the current working directory for a .hushlogin file, causing
    // it to miss any in the user's home directory. We can fix this by doing the check
    // ourselves and passing `-q`
    let has_home_hushlogin = Path::new(home).join(".hushlogin").exists();

    // -f: Bypasses authentication for the already-logged-in user.
    // -l: Skips changing directory to $HOME and prepending '-' to argv[0].
    // -p: Preserves the environment.
    // -q: Act as if `.hushlogin` exists.
    //
    // XXX: we use zsh here over sh due to `exec -a`.
    let flags = if has_home_hushlogin { "-qflp" } else { "-flp" };
    login_command.args([flags, user, "/bin/zsh", "-fc", &exec]);
    login_command
}

/// Create a new TTY and return a handle to interact with it.
pub fn new(config: &Options, window_size: WindowSize, window_id: u64) -> Result<Pty> {
    let pty = openpty(None, Some(&window_size.to_winsize()))?;
    let (master, slave) = (pty.controller, pty.user);
    from_fd(config, window_id, master, slave)
}

/// Create a new TTY from a PTY's file descriptors.
pub fn from_fd(config: &Options, window_id: u64, master: OwnedFd, slave: OwnedFd) -> Result<Pty> {
    let master_fd = master.as_raw_fd();
    let slave_fd = slave.as_raw_fd();

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Ok(mut termios) = termios::tcgetattr(&master) {
        // Set character encoding to UTF-8.
        termios.input_modes.set(InputModes::IUTF8, true);
        let _ = termios::tcsetattr(&master, OptionalActions::Now, &termios);
    }

    let user = ShellUser::from_env()?;

    let mut builder = if let Some(shell) = config.shell.as_ref() {
        let mut cmd = Command::new(&shell.program);
        cmd.args(shell.args.as_slice());
        cmd
    } else {
        default_shell_command(&user.shell, &user.user, &user.home)
    };

    // Setup child stdin/stdout/stderr as slave fd of PTY.
    builder.stdin(slave.try_clone()?);
    builder.stderr(slave.try_clone()?);
    builder.stdout(slave);

    // Setup shell environment.
    let window_id = window_id.to_string();
    builder.env("ALACRITTY_WINDOW_ID", &window_id);
    builder.env("USER", user.user);
    builder.env("HOME", user.home);
    // Set Window ID for clients relying on X11 hacks.
    builder.env("WINDOWID", window_id);
    for (key, value) in &config.env {
        builder.env(key, value);
    }

    // Prevent child processes from inheriting linux-specific startup notification env.
    builder.env_remove("XDG_ACTIVATION_TOKEN");
    builder.env_remove("DESKTOP_STARTUP_ID");

    let working_directory = config
        .working_directory
        .as_ref()
        .and_then(|path| CString::new(path.as_os_str().as_bytes()).ok());
    let child_signal_mask = config.child_signal_mask;
    unsafe {
        builder.pre_exec(move || {
            // Create a new process group.
            let err = libc::setsid();
            if err == -1 {
                return Err(Error::last_os_error());
            }

            // Set working directory, ignoring invalid paths.
            if let Some(working_directory) = working_directory.as_ref() {
                libc::chdir(working_directory.as_ptr());
            }

            set_controlling_terminal(slave_fd)?;

            // No longer need slave/master fds.
            libc::close(slave_fd);
            libc::close(master_fd);

            if let Some(child_signal_mask) = child_signal_mask {
                child_signal_mask.apply()?;
            }

            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGALRM, libc::SIG_DFL);

            Ok(())
        });
    }

    // Prepare signal handling before spawning child.
    let (signals, sig_id) = {
        let (sender, recv) = UnixStream::pair()?;

        // Register the recv end of the pipe for SIGCHLD.
        let sig_id = signal_pipe::register(sigconsts::SIGCHLD, sender)?;
        recv.set_nonblocking(true)?;
        (recv, sig_id)
    };

    match builder.spawn() {
        Ok(child) => {
            unsafe {
                // Maybe this should be done outside of this function so nonblocking
                // isn't forced upon consumers. Although maybe it should be?
                set_nonblocking(master_fd)?;
            }

            Ok(Pty {
                child: PtyChild::Owned(child),
                file: File::from(master),
                signals,
                sig_id: Some(sig_id),
            })
        },
        Err(err) => Err(Error::new(
            err.kind(),
            format!(
                "Failed to spawn command '{}': {}",
                builder.get_program().to_string_lossy(),
                err
            ),
        )),
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Clear signal-hook handler.
        if let Some(sig_id) = self.sig_id {
            unregister_signal(sig_id);
        }

        match &mut self.child {
            PtyChild::Owned(child) => {
                // Make sure the PTY is terminated properly.
                unsafe {
                    libc::kill(child.id() as i32, libc::SIGHUP);
                }

                let _ = child.wait();
            },
            // An attached child outlives this process by design: detaching a
            // session is exactly dropping the PTY here. Hanging it up would
            // kill the session the multiplexer is holding, and waiting on it
            // would reap a process this one did not spawn.
            PtyChild::Attached { .. } => {},
            PtyChild::Reclaimed { pid } => {
                let pid = *pid as i32;
                unsafe {
                    libc::kill(pid, libc::SIGHUP);
                    let mut status = 0;
                    libc::waitpid(pid, &mut status, 0);
                }
            },
        }
    }
}

impl EventedReadWrite for Pty {
    type Reader = File;
    type Writer = File;

    #[inline]
    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        poll_opts: PollMode,
    ) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        unsafe {
            poll.add_with_mode(&self.file, interest, poll_opts)?;
        }

        unsafe {
            poll.add_with_mode(
                &self.signals,
                Event::readable(PTY_CHILD_EVENT_TOKEN),
                PollMode::Level,
            )
        }
    }

    #[inline]
    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        mut interest: Event,
        poll_opts: PollMode,
    ) -> Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        poll.modify_with_mode(&self.file, interest, poll_opts)?;

        poll.modify_with_mode(
            &self.signals,
            Event::readable(PTY_CHILD_EVENT_TOKEN),
            PollMode::Level,
        )
    }

    #[inline]
    fn deregister(&mut self, poll: &Arc<Poller>) -> Result<()> {
        poll.delete(&self.file)?;
        poll.delete(&self.signals)
    }

    #[inline]
    fn reader(&mut self) -> &mut File {
        &mut self.file
    }

    #[inline]
    fn writer(&mut self) -> &mut File {
        &mut self.file
    }
}

impl EventedPty for Pty {
    #[inline]
    fn child_is_foreign(&self) -> bool {
        // A reclaimed child is still this process's own — `execv` does not
        // change parentage — so only an attached one is foreign.
        matches!(self.child, PtyChild::Attached { .. })
    }

    #[inline]
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        match &mut self.child {
            PtyChild::Owned(_) => {
                // See if there has been a SIGCHLD.
                let mut buf = [0u8; 1];
                if let Err(err) = self.signals.read(&mut buf) {
                    if err.kind() != ErrorKind::WouldBlock {
                        error!("Error reading from signal pipe: {err}");
                    }
                    return None;
                }

                let PtyChild::Owned(child) = &mut self.child else { return None };

                // Match on the child process.
                match child.try_wait() {
                    Err(err) => {
                        error!("Error checking child process termination: {err}");
                        Some(ChildEvent::ExitStatusUnavailable)
                    },
                    Ok(None) => None,
                    Ok(Some(exit_status)) => Some(ChildEvent::Exited(exit_status)),
                }
            },
            PtyChild::Attached { .. } => self.next_attached_child_event(),
            PtyChild::Reclaimed { .. } => {
                // Same SIGCHLD pipe as an owned child; only the reaping differs.
                let mut buf = [0u8; 1];
                if let Err(err) = self.signals.read(&mut buf) {
                    if err.kind() != ErrorKind::WouldBlock {
                        error!("Error reading from signal pipe: {err}");
                    }
                    return None;
                }
                let pid = self.child_pid() as i32;
                let mut status = 0;
                // SAFETY: `pid` is a child of this process and `status` is
                // writable for the duration of the call.
                let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
                match reaped {
                    0 => None,
                    result if result < 0 => Some(ChildEvent::ExitStatusUnavailable),
                    _ => Some(ChildEvent::Exited(ExitStatus::from_raw(status))),
                }
            },
        }
    }
}

impl Pty {
    /// Decodes one child event reported by the multiplexer.
    ///
    /// A stream socket may split a write, so a partial frame is buffered rather
    /// than misread: acting on half an exit status would report a wrong code
    /// for a session the user still cares about.
    fn next_attached_child_event(&mut self) -> Option<ChildEvent> {
        let mut buf = [0u8; ATTACHED_EVENT_LEN];
        let read = match self.signals.read(&mut buf) {
            Ok(0) => {
                // The reporting end was dropped without saying anything, so no
                // status is ever coming.
                return Some(ChildEvent::WatcherDisconnected);
            },
            Ok(read) => read,
            Err(err) => {
                if err.kind() != ErrorKind::WouldBlock {
                    error!("Error reading from the multiplexer's child event socket: {err}");
                }
                return None;
            },
        };

        let PtyChild::Attached { pending, .. } = &mut self.child else { return None };
        pending.extend_from_slice(&buf[..read]);
        if pending.len() < ATTACHED_EVENT_LEN {
            return None;
        }

        let frame = pending.drain(..ATTACHED_EVENT_LEN).collect::<Vec<_>>();
        let raw_status = i32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]);
        match frame[0] {
            ATTACHED_EXITED => Some(ChildEvent::Exited(ExitStatus::from_raw(raw_status))),
            ATTACHED_STATUS_UNAVAILABLE => Some(ChildEvent::ExitStatusUnavailable),
            ATTACHED_WATCHER_DISCONNECTED => Some(ChildEvent::WatcherDisconnected),
            unknown => {
                error!("Unknown child event {unknown} from the multiplexer");
                Some(ChildEvent::ExitStatusUnavailable)
            },
        }
    }
}

impl OnResize for Pty {
    /// Resize the PTY.
    ///
    /// Tells the kernel that the window size changed with the new pixel
    /// dimensions and line/column counts.
    fn on_resize(&mut self, window_size: WindowSize) {
        let win = window_size.to_winsize();

        let res = unsafe { libc::ioctl(self.file.as_raw_fd(), libc::TIOCSWINSZ, &win as *const _) };

        if res < 0 {
            die!("ioctl TIOCSWINSZ failed: {}", Error::last_os_error());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::mem::MaybeUninit;
    use std::os::unix::process::ExitStatusExt;
    use std::ptr;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use crate::event::WindowSize;
    use crate::tty::{Options, Shell};

    struct SignalMaskGuard {
        previous: libc::sigset_t,
    }

    impl SignalMaskGuard {
        fn block_signal(signal: libc::c_int) -> Self {
            let mut blocked = MaybeUninit::<libc::sigset_t>::uninit();
            let mut previous = MaybeUninit::<libc::sigset_t>::uninit();

            unsafe {
                assert_eq!(libc::sigemptyset(blocked.as_mut_ptr()), 0);
                let mut blocked = blocked.assume_init();
                assert_eq!(libc::sigaddset(&mut blocked, signal), 0);
                assert_eq!(
                    libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, previous.as_mut_ptr()),
                    0
                );

                Self { previous: previous.assume_init() }
            }
        }
    }

    impl Drop for SignalMaskGuard {
        fn drop(&mut self) {
            unsafe {
                assert_eq!(
                    libc::pthread_sigmask(libc::SIG_SETMASK, &self.previous, ptr::null_mut()),
                    0
                );
            }
        }
    }

    fn sleep_command_options(child_signal_mask: Option<super::SignalMask>) -> Options {
        Options {
            shell: Some(Shell::new("/bin/sleep".to_owned(), vec!["30".to_owned()])),
            child_signal_mask,
            ..Options::default()
        }
    }

    fn window_size() -> WindowSize {
        WindowSize { num_lines: 24, num_cols: 80, cell_width: 8, cell_height: 16 }
    }

    fn wait_for_child_exit(
        pty: &mut super::Pty,
        timeout: Duration,
    ) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = pty.try_wait().expect("failed to poll PTY child") {
                return Some(status);
            }

            if Instant::now() >= deadline {
                return None;
            }

            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_ctrl_c_terminates_sleep_child(child_signal_mask: Option<super::SignalMask>) {
        let mut pty = super::new(&sleep_command_options(child_signal_mask), window_size(), 0)
            .expect("failed to spawn PTY child");

        pty.file.write_all(b"\x03").expect("failed to write Ctrl-C to PTY");

        let status = wait_for_child_exit(&mut pty, Duration::from_secs(2))
            .expect("PTY child did not exit after Ctrl-C");

        assert_eq!(status.signal(), Some(libc::SIGINT));
    }

    #[test]
    fn ctrl_c_reaches_child_spawned_with_default_signal_mask() {
        assert_ctrl_c_terminates_sleep_child(None);
    }

    #[test]
    fn ctrl_c_reaches_child_spawned_with_sigint_blocked_on_parent_thread() {
        let child_signal_mask =
            super::SignalMask::current().expect("failed to capture signal mask");
        let _signal_mask_guard = SignalMaskGuard::block_signal(libc::SIGINT);

        assert_ctrl_c_terminates_sleep_child(Some(child_signal_mask));
    }

    /// Spawns a `/bin/sleep` PTY child the way Zed does: the signal mask is captured on a
    /// "foreground" thread (where terminal signals are unblocked) while the PTY itself is
    /// created on a separate "background" thread that has `signal` blocked.
    ///
    /// On macOS, Zed's background executor runs work on libdispatch (GCD) worker threads,
    /// which start with terminal signals blocked. Without forwarding the foreground mask
    /// through `Options::child_signal_mask`, the forked child inherits that blocked mask
    /// and never receives the signal.
    fn spawn_sleep_child_with_foreground_signal_mask(signal: libc::c_int) -> super::Pty {
        let (mask_tx, mask_rx) = mpsc::channel();

        // "Foreground" thread: capture the mask while `signal` is unblocked, then hand it off.
        let foreground = thread::spawn(move || {
            let mask = super::SignalMask::current().expect("failed to capture signal mask");
            mask_tx.send(mask).expect("failed to forward captured signal mask");
        });

        // "Background" thread: block `signal`, then spawn the child with the captured mask.
        let background = thread::spawn(move || {
            let _signal_mask_guard = SignalMaskGuard::block_signal(signal);
            let mask = mask_rx.recv().expect("failed to receive captured signal mask");
            super::new(&sleep_command_options(Some(mask)), window_size(), 0)
                .expect("failed to spawn PTY child")
        });

        foreground.join().expect("foreground thread panicked");
        background.join().expect("background thread panicked")
    }

    /// Asserts that a signal Alacritty does *not* reset to `SIG_DFL` in its `pre_exec` hook is
    /// still deliverable to a child spawned on a background thread that had it blocked, because
    /// the captured foreground mask is applied before exec.
    ///
    /// Alacritty only resets the dispositions of `SIGCHLD`, `SIGHUP`, `SIGINT`, `SIGQUIT`,
    /// `SIGTERM` and `SIGALRM`. Every other signal relies entirely on the inherited signal mask
    /// being correct, so this is where forwarding the foreground mask actually matters.
    fn assert_signal_reaches_child_spawned_on_background_thread(signal: libc::c_int) {
        let mut pty = spawn_sleep_child_with_foreground_signal_mask(signal);
        let pid = pty.child_pid() as libc::pid_t;

        assert_eq!(
            unsafe { libc::kill(pid, signal) },
            0,
            "failed to send signal {signal} to PTY child: {}",
            std::io::Error::last_os_error(),
        );

        let status = wait_for_child_exit(&mut pty, Duration::from_secs(2))
            .unwrap_or_else(|| panic!("PTY child did not exit after signal {signal}"));

        assert_eq!(status.signal(), Some(signal));
    }

    // The signals below are representative of those Alacritty does not reset: their default
    // action is to terminate, and they are not used by the Rust test runtime, so delivery can
    // be observed via process termination. Stop-default (`SIGTSTP`/`SIGTTIN`/`SIGTTOU`) and
    // ignore-default (`SIGWINCH`) signals can't be observed this way and are not covered here.
    #[test]
    fn sigusr1_reaches_child_spawned_on_background_thread() {
        assert_signal_reaches_child_spawned_on_background_thread(libc::SIGUSR1);
    }

    #[test]
    fn sigusr2_reaches_child_spawned_on_background_thread() {
        assert_signal_reaches_child_spawned_on_background_thread(libc::SIGUSR2);
    }

    #[test]
    fn sigvtalrm_reaches_child_spawned_on_background_thread() {
        assert_signal_reaches_child_spawned_on_background_thread(libc::SIGVTALRM);
    }

    /// Without forwarding the foreground mask, a child spawned on a thread that has a signal
    /// blocked inherits that blocked mask and never receives the signal. This pins down the
    /// regression that broke Ctrl-C in the terminal when spawning moved to the background
    /// executor (zed-industries/zed#42234, #42411).
    #[test]
    fn signal_blocked_on_spawn_thread_is_not_delivered_without_foreground_mask() {
        let signal = libc::SIGUSR1;

        let mut pty = thread::spawn(move || {
            let _signal_mask_guard = SignalMaskGuard::block_signal(signal);
            super::new(&sleep_command_options(None), window_size(), 0)
                .expect("failed to spawn PTY child")
        })
        .join()
        .expect("spawn thread panicked");

        let pid = pty.child_pid() as libc::pid_t;
        assert_eq!(
            unsafe { libc::kill(pid, signal) },
            0,
            "failed to send signal {signal} to PTY child: {}",
            std::io::Error::last_os_error(),
        );

        // The signal is blocked in the child, so it must still be running after a grace period.
        assert!(
            wait_for_child_exit(&mut pty, Duration::from_millis(500)).is_none(),
            "child exited after a blocked signal was sent; it should have stayed blocked",
        );

        // Clean up the still-running child.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

/// Types that can produce a `Winsize`.
pub trait ToWinsize {
    /// Get a `Winsize`.
    fn to_winsize(self) -> Winsize;
}

impl ToWinsize for WindowSize {
    fn to_winsize(self) -> Winsize {
        let ws_row = self.num_lines as libc::c_ushort;
        let ws_col = self.num_cols as libc::c_ushort;

        let ws_xpixel = ws_col * self.cell_width as libc::c_ushort;
        let ws_ypixel = ws_row * self.cell_height as libc::c_ushort;
        Winsize { ws_row, ws_col, ws_xpixel, ws_ypixel }
    }
}

unsafe fn set_nonblocking(fd: c_int) -> Result<()> {
    let res = unsafe { fcntl(fd, F_SETFL, fcntl(fd, F_GETFL, 0) | O_NONBLOCK) };
    if res == 0 { Ok(()) } else { Err(Error::last_os_error()) }
}

#[test]
fn test_get_pw_entry() {
    let mut buf: [i8; 1024] = [0; 1024];
    let _pw = get_pw_entry(&mut buf).unwrap();
}

#[cfg(test)]
mod attached_tests {
    use super::*;
    use crate::tty::Shell;
    use std::time::Duration;

    fn attached_pty(child_pid: u32) -> (Pty, AttachedChildEvents) {
        let pty = openpty(None, None).unwrap();
        attach(pty.controller, child_pid).unwrap()
    }

    #[test]
    fn an_attached_child_reports_its_exit_status_through_the_multiplexer() {
        let (mut pty, mut events) = attached_pty(1234);

        // Nothing reported yet: the event loop must keep running.
        assert!(pty.next_child_event().is_none());

        events.report_exit(ExitStatus::from_raw(3 << 8).into_raw()).unwrap();
        match pty.next_child_event() {
            Some(ChildEvent::Exited(status)) => assert_eq!(status.code(), Some(3)),
            other => panic!("expected an exit status, got {other:?}"),
        }
    }

    #[test]
    fn a_split_child_event_is_buffered_rather_than_misread() {
        let (mut pty, events) = attached_pty(1234);

        // A stream socket may deliver a write in pieces. Acting on a partial
        // frame would report an exit code assembled from whatever arrived.
        let frame = [ATTACHED_EXITED, 0, 1, 0, 0];
        (&events.0).write_all(&frame[..2]).unwrap();
        assert!(pty.next_child_event().is_none());

        (&events.0).write_all(&frame[2..]).unwrap();
        match pty.next_child_event() {
            Some(ChildEvent::Exited(status)) => {
                assert_eq!(status.into_raw(), i32::from_le_bytes([0, 1, 0, 0]))
            },
            other => panic!("expected the reassembled exit status, got {other:?}"),
        }
    }

    #[test]
    fn losing_the_multiplexer_reports_a_disconnected_watcher() {
        let (mut pty, events) = attached_pty(1234);

        // Otherwise a pane whose daemon died would spin: the event loop retries
        // a failing read on the master and only stops on a child event.
        drop(events);
        assert_eq!(pty.next_child_event(), Some(ChildEvent::WatcherDisconnected));
    }

    #[test]
    fn dropping_an_attached_pty_leaves_the_session_running() {
        // Detaching a session is exactly this drop. Hanging the child up here
        // would kill the session the multiplexer is holding.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let (pty, _events) = attached_pty(pid);

        drop(pty);

        // `kill(pid, 0)` is not the check to make here: a child killed by the
        // drop stays a zombie until it is reaped, and signal 0 succeeds on a
        // zombie. Reaping it is what distinguishes the two.
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().unwrap().is_none(),
            "the attached child was killed by dropping the PTY"
        );
        let _ = pid;
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn only_a_multiplexer_owned_child_is_foreign() {
        // What the event loop uses to decide whether a hung-up master is worth
        // waiting on. For a child this process spawned the exit notification is
        // already on its way, so waiting is right and the loop keeps upstream's
        // behaviour. For the multiplexer's child the notification can only
        // arrive as a report, so waiting forever is what a broken control
        // channel used to mean.
        let (attached, _events) = attached_pty(1234);
        assert!(attached.child_is_foreign());

        let options = Options {
            shell: Some(Shell::new(
                "/bin/sh".to_owned(),
                vec!["-c".to_owned(), "sleep 30".to_owned()],
            )),
            ..Options::default()
        };
        let owned = crate::tty::new(
            &options,
            WindowSize { num_lines: 24, num_cols: 80, cell_width: 1, cell_height: 1 },
            0,
        )
        .unwrap();
        assert!(!owned.child_is_foreign());
        drop(owned);

        // A pane reclaimed across the multiplexer's own re-exec is still this
        // process's child — an exec does not change parentage — so it must not
        // be treated as foreign either, or nothing would ever reap it.
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let terminal = openpty(None, None).unwrap();
        let reclaimed = reclaim(terminal.controller, child.id()).unwrap();
        assert!(!reclaimed.child_is_foreign());
        // Its drop hangs the child up and reaps it, which is what `Reclaimed`
        // means; the `Child` is then already gone, hence the ignored result.
        drop(reclaimed);
        let _ = child.kill();
        let _ = child.wait();
    }
}
