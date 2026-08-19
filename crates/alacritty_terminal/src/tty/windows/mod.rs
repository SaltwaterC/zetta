use std::ffi::OsStr;
use std::io::{self, Result};
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use std::sync::mpsc::TryRecvError;

use crate::event::{OnResize, WindowSize};
use crate::tty::windows::child::ChildExitWatcher;
use crate::tty::{ChildEvent, EventedPty, EventedReadWrite, Options, Shell};

mod blocking;
mod child;
mod conpty;

use std::num::NonZeroU32;
use std::os::windows::io::FromRawHandle as _;

use blocking::{UnblockedReader, UnblockedWriter};
use conpty::Conpty as Backend;
use miow::pipe::{AnonRead, AnonWrite};
use polling::{Event, Poller};

pub const PTY_CHILD_EVENT_TOKEN: usize = 1;
pub const PTY_READ_WRITE_TOKEN: usize = 2;

type ReadPipe = UnblockedReader<AnonRead>;
type WritePipe = UnblockedWriter<AnonWrite>;

pub struct Pty {
    // XXX: Backend is required to be the first field, to ensure correct drop order. Dropping
    // `conout` before `backend` will cause a deadlock (with Conpty).
    backend: PtyBackend,
    conout: ReadPipe,
    conin: WritePipe,
    child_watcher: ChildExitWatcher,
    /// Duplicates of the console's pipes, retained only by a PTY this process
    /// created, which is the only one that has anything to hand over.
    handover: Option<(std::os::windows::io::OwnedHandle, std::os::windows::io::OwnedHandle)>,
}

/// Who owns the pseudoconsole behind this PTY.
enum PtyBackend {
    /// Created here, so resizing and teardown happen here.
    Owned(Backend),
    /// Created by the multiplexer, which passed this process the console's
    /// pipes. A pseudoconsole handle cannot cross a process boundary — it is
    /// only meaningful to its creator — so resizing has to be asked of the
    /// owner over the control channel, and dropping this must leave the
    /// console alone so the session survives being detached.
    Attached,
}

/// The reporting end of an attached PTY's child events.
///
/// Named and shaped to match the Unix implementation, so everything above the
/// platform layer treats an attached terminal the same way on either.
pub struct AttachedChildEvents(child::ChildExitReporter);

impl AttachedChildEvents {
    /// Reports the exit code the multiplexer observed.
    pub fn report_exit(&mut self, exit_code: i32) -> io::Result<()> {
        self.report(ChildEvent::Exited(exit_status_from_code(exit_code)))
    }

    pub fn report_status_unavailable(&mut self) -> io::Result<()> {
        self.report(ChildEvent::ExitStatusUnavailable)
    }

    pub fn report_watcher_disconnected(&mut self) -> io::Result<()> {
        self.report(ChildEvent::WatcherDisconnected)
    }

    fn report(&mut self, event: ChildEvent) -> io::Result<()> {
        self.0
            .report(event)
            .map_err(|()| io::Error::other("the terminal is no longer listening for its exit"))
    }
}

/// Builds an `ExitStatus` from a raw Windows exit code.
///
/// Unix carries a wait status and Windows an exit code; both arrive here as an
/// `i32` from the multiplexer, and each platform interprets it as its own.
fn exit_status_from_code(exit_code: i32) -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(exit_code as u32)
}

/// Builds a PTY around a pseudoconsole another process owns.
///
/// `conout` and `conin` are that console's pipes, duplicated into this process.
/// Resizing is not possible from here, so the caller forwards it to the owner;
/// the exit status arrives through the returned reporter, because only the
/// owner can wait on the process.
pub fn attach(
    conout: std::os::windows::io::OwnedHandle,
    conin: std::os::windows::io::OwnedHandle,
    child_pid: u32,
) -> io::Result<(Pty, AttachedChildEvents)> {
    use std::os::windows::io::IntoRawHandle as _;

    // SAFETY: both handles are owned by this process and are given up here
    // exactly once, so each pipe becomes the sole owner of its handle.
    let conout_pipe = unsafe { AnonRead::from_raw_handle(conout.into_raw_handle()) };
    let conin_pipe = unsafe { AnonWrite::from_raw_handle(conin.into_raw_handle()) };
    let (child_watcher, reporter) = ChildExitWatcher::external(NonZeroU32::new(child_pid));
    let pty = Pty {
        backend: PtyBackend::Attached,
        conout: UnblockedReader::new(conout_pipe, PIPE_CAPACITY),
        conin: UnblockedWriter::new(conin_pipe, PIPE_CAPACITY),
        child_watcher,
        // An attached terminal is not this process's to hand on.
        handover: None,
    };
    Ok((pty, AttachedChildEvents(reporter)))
}

/// The buffer each direction of an attached console is unblocked through,
/// matching what `conpty::new` uses for a console it creates.
///
/// It has to be the same constant, not merely a similar one: `piper::pipe`
/// asserts a positive capacity, so a zero here panicked every attached pane
/// before it could be built.
const PIPE_CAPACITY: usize = conpty::PIPE_CAPACITY;

pub fn new(config: &Options, window_size: WindowSize, _window_id: u64) -> Result<Pty> {
    conpty::new(config, window_size)
}

impl Pty {
    fn new(
        backend: impl Into<Backend>,
        conout: impl Into<ReadPipe>,
        conin: impl Into<WritePipe>,
        child_watcher: ChildExitWatcher,
        handover: Option<(std::os::windows::io::OwnedHandle, std::os::windows::io::OwnedHandle)>,
    ) -> Self {
        Self {
            backend: PtyBackend::Owned(backend.into()),
            conout: conout.into(),
            conin: conin.into(),
            child_watcher,
            handover,
        }
    }

    /// The console's pipes, kept so this terminal can be handed to another
    /// process.
    ///
    /// The unblocking wrappers take ownership of the pipes and read them on
    /// their own threads, so duplicates are retained at construction: there is
    /// otherwise nothing left to duplicate into a client when it attaches.
    pub fn handover_handles(
        &self,
    ) -> Option<(&std::os::windows::io::OwnedHandle, &std::os::windows::io::OwnedHandle)> {
        self.handover.as_ref().map(|(conout, conin)| (conout, conin))
    }

    /// The PTY child's process ID, whether this process spawned it or not.
    pub fn child_pid(&self) -> u32 {
        self.child_watcher.pid().map(|pid| pid.get()).unwrap_or_default()
    }

    pub fn child_watcher(&self) -> &ChildExitWatcher {
        &self.child_watcher
    }
}

fn with_key(mut event: Event, key: usize) -> Event {
    event.key = key;
    event
}

impl EventedReadWrite for Pty {
    type Reader = ReadPipe;
    type Writer = WritePipe;

    #[inline]
    unsafe fn register(
        &mut self,
        poll: &Arc<Poller>,
        interest: polling::Event,
        poll_opts: polling::PollMode,
    ) -> io::Result<()> {
        self.conin.register(poll, with_key(interest, PTY_READ_WRITE_TOKEN), poll_opts);
        self.conout.register(poll, with_key(interest, PTY_READ_WRITE_TOKEN), poll_opts);
        self.child_watcher.register(poll, with_key(interest, PTY_CHILD_EVENT_TOKEN));

        Ok(())
    }

    #[inline]
    fn reregister(
        &mut self,
        poll: &Arc<Poller>,
        interest: polling::Event,
        poll_opts: polling::PollMode,
    ) -> io::Result<()> {
        self.conin.register(poll, with_key(interest, PTY_READ_WRITE_TOKEN), poll_opts);
        self.conout.register(poll, with_key(interest, PTY_READ_WRITE_TOKEN), poll_opts);
        self.child_watcher.register(poll, with_key(interest, PTY_CHILD_EVENT_TOKEN));

        Ok(())
    }

    #[inline]
    fn deregister(&mut self, _poll: &Arc<Poller>) -> io::Result<()> {
        self.conin.deregister();
        self.conout.deregister();
        self.child_watcher.deregister();

        Ok(())
    }

    #[inline]
    fn rearm_read(&mut self) -> io::Result<()> {
        self.conout.rearm();
        Ok(())
    }

    #[inline]
    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.conout
    }

    #[inline]
    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.conin
    }
}

impl EventedPty for Pty {
    fn child_is_foreign(&self) -> bool {
        matches!(self.backend, PtyBackend::Attached)
    }

    fn next_child_event(&mut self) -> Option<ChildEvent> {
        child_event_from_recv(self.child_watcher.event_rx().try_recv())
    }
}

fn child_event_from_recv(
    result: std::result::Result<ChildEvent, TryRecvError>,
) -> Option<ChildEvent> {
    match result {
        Ok(event) => Some(event),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(ChildEvent::WatcherDisconnected),
    }
}

impl OnResize for Pty {
    fn on_resize(&mut self, window_size: WindowSize) {
        match &mut self.backend {
            PtyBackend::Owned(backend) => backend.on_resize(window_size),
            // Only the pseudoconsole's creator can resize it. The caller sends
            // the new size to the multiplexer instead; doing nothing here is
            // what keeps that from silently appearing to have worked.
            PtyBackend::Attached => {},
        }
    }
}

// Modified per stdlib implementation.
// https://github.com/rust-lang/rust/blob/6707bf0f59485cf054ac1095725df43220e4be20/library/std/src/sys/args/windows.rs#L174
fn push_escaped_arg(cmd: &mut String, arg: &str) {
    let arg_bytes = arg.as_bytes();
    let quote = arg_bytes.iter().any(|c| *c == b' ' || *c == b'\t') || arg_bytes.is_empty();
    if quote {
        cmd.push('"');
    }

    let mut backslashes: usize = 0;
    for x in arg.chars() {
        if x == '\\' {
            backslashes += 1;
        } else {
            if x == '"' {
                // Add n+1 backslashes to total 2n+1 before internal '"'.
                cmd.extend((0..=backslashes).map(|_| '\\'));
            }
            backslashes = 0;
        }
        cmd.push(x);
    }

    if quote {
        // Add n backslashes to total 2n before ending '"'.
        cmd.extend((0..backslashes).map(|_| '\\'));
        cmd.push('"');
    }
}

fn cmdline(config: &Options) -> String {
    let default_shell = Shell::new("powershell".to_owned(), Vec::new());
    let shell = config.shell.as_ref().unwrap_or(&default_shell);

    let mut cmd = String::new();
    cmd.push_str(&shell.program);

    for arg in &shell.args {
        cmd.push(' ');
        if config.escape_args {
            push_escaped_arg(&mut cmd, arg);
        } else {
            cmd.push_str(arg)
        }
    }
    cmd
}

/// Converts the string slice into a Windows-standard representation for "W"-
/// suffixed function variants, which accept UTF-16 encoded string values.
pub fn win32_string<S: AsRef<OsStr> + ?Sized>(value: &S) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(once(0)).collect()
}

#[cfg(test)]
mod test {
    use crate::tty::windows::{cmdline, push_escaped_arg};
    use crate::tty::{ChildEvent, Options, Shell};
    use std::sync::mpsc::TryRecvError;

    #[test]
    fn disconnected_child_watcher_is_classified() {
        assert_eq!(
            super::child_event_from_recv(Err(TryRecvError::Disconnected)),
            Some(ChildEvent::WatcherDisconnected)
        );
        assert_eq!(super::child_event_from_recv(Err(TryRecvError::Empty)), None);
    }

    #[test]
    fn test_escape() {
        let test_set = vec![
            // Basic cases - no escaping needed
            ("abc", "abc"),
            // Cases requiring quotes (space/tab)
            ("", "\"\""),
            (" ", "\" \""),
            ("ab c", "\"ab c\""),
            ("ab\tc", "\"ab\tc\""),
            // Cases with backslashes only (no spaces, no quotes) - no quotes added
            ("ab\\c", "ab\\c"),
            // Cases with quotes only (no spaces) - quotes escaped but no outer quotes
            ("ab\"c", "ab\\\"c"),
            ("\"", "\\\""),
            ("a\"b\"c", "a\\\"b\\\"c"),
            // Cases requiring both quotes and escaping (contains spaces)
            ("ab \"c", "\"ab \\\"c\""),
            ("a \"b\" c", "\"a \\\"b\\\" c\""),
            // Complex real-world cases
            ("C:\\Program Files\\", "\"C:\\Program Files\\\\\""),
            ("C:\\Program Files\\a.txt", "\"C:\\Program Files\\a.txt\""),
            (
                r#"sh -c "cd /home/user; ARG='abc' \""'${SHELL:-sh}" -i -c '"'echo hello'""#,
                r#""sh -c \"cd /home/user; ARG='abc' \\\"\"'${SHELL:-sh}\" -i -c '\"'echo hello'\"""#,
            ),
        ];

        for (input, expected) in test_set {
            let mut escaped_arg = String::new();
            push_escaped_arg(&mut escaped_arg, input);
            assert_eq!(escaped_arg, expected, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_cmdline() {
        let mut options = Options {
            shell: Some(Shell {
                program: "echo".to_string(),
                args: vec!["hello world".to_string()],
            }),
            working_directory: None,
            drain_on_exit: true,
            env: Default::default(),
            escape_args: false,
        };
        assert_eq!(cmdline(&options), "echo hello world");

        options.escape_args = true;
        assert_eq!(cmdline(&options), "echo \"hello world\"");
    }
}
