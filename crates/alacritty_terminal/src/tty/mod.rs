//! TTY related functionality.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;
use std::{env, io};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use polling::{Event, PollMode, Poller};

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use self::unix::*;

#[cfg(windows)]
pub mod windows;
#[cfg(windows)]
pub use self::windows::*;

/// Configuration for the `Pty` interface.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Options {
    /// Shell options.
    ///
    /// [`None`] will use the default shell.
    pub shell: Option<Shell>,

    /// Shell startup directory.
    pub working_directory: Option<PathBuf>,

    /// Drain the child process output before exiting the terminal.
    pub drain_on_exit: bool,

    /// Extra environment variables.
    pub env: HashMap<String, String>,

    /// Signal mask to apply in the child process before exec.
    #[cfg(not(windows))]
    pub child_signal_mask: Option<SignalMask>,

    /// Specifies whether the Windows shell arguments should be escaped.
    ///
    /// - When `true`: Arguments will be escaped according to the standard C runtime rules.
    /// - When `false`: Arguments will be passed raw without additional escaping.
    #[cfg(target_os = "windows")]
    pub escape_args: bool,

    /// Initial legacy Win32 console colors for the pseudoconsole.
    #[cfg(target_os = "windows")]
    pub console_palette: ConsolePalette,

    /// Internal executable used to apply legacy colors inside the pseudoconsole.
    #[cfg(target_os = "windows")]
    pub console_palette_helper: Option<PathBuf>,
}

/// The legacy Win32 console color state associated with a pseudoconsole.
///
/// Colors are in ANSI order (normal 0-7, bright 8-15). The foreground and
/// background are indices into that exact table because Win32 attributes can
/// represent palette entries, not arbitrary RGB values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ConsolePalette {
    pub colors: [[u8; 3]; 16],
    pub foreground_index: u8,
    pub background_index: u8,
}

impl Default for ConsolePalette {
    fn default() -> Self {
        Self {
            colors: [
                [0, 0, 0],
                [128, 0, 0],
                [0, 128, 0],
                [128, 128, 0],
                [0, 0, 128],
                [128, 0, 128],
                [0, 128, 128],
                [192, 192, 192],
                [128, 128, 128],
                [255, 0, 0],
                [0, 255, 0],
                [255, 255, 0],
                [0, 0, 255],
                [255, 0, 255],
                [0, 255, 255],
                [255, 255, 255],
            ],
            foreground_index: 7,
            background_index: 0,
        }
    }
}

impl ConsolePalette {
    /// Environment-safe fixed-width representation used only by the bundled
    /// Windows helper. Fixed width makes malformed or partial payloads easy to
    /// reject before any console state is touched.
    pub fn to_private_payload(self) -> String {
        use std::fmt::Write as _;

        let mut payload = String::with_capacity(100);
        for color in self.colors {
            for channel in color {
                write!(&mut payload, "{channel:02x}").expect("writing to a string cannot fail");
            }
        }
        write!(&mut payload, "{:02x}{:02x}", self.foreground_index, self.background_index)
            .expect("writing to a string cannot fail");
        payload
    }

    pub fn from_private_payload(payload: &str) -> Option<Self> {
        if payload.len() != 100 || !payload.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut bytes = [0_u8; 50];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&payload[index * 2..index * 2 + 2], 16).ok()?;
        }
        let mut colors = [[0_u8; 3]; 16];
        for (color, channels) in colors.iter_mut().zip(bytes[..48].chunks_exact(3)) {
            color.copy_from_slice(channels);
        }
        let foreground_index = bytes[48];
        let background_index = bytes[49];
        if foreground_index >= 16 || background_index >= 16 {
            return None;
        }
        Some(Self { colors, foreground_index, background_index })
    }
}

/// Shell options.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Shell {
    /// Path to a shell program to run on startup.
    pub(crate) program: String,
    /// Arguments passed to shell.
    pub(crate) args: Vec<String>,
}

impl Shell {
    pub fn new(program: String, args: Vec<String>) -> Self {
        Self { program, args }
    }
}

/// Stream read and/or write behavior.
///
/// This defines an abstraction over polling's interface in order to allow either
/// one read/write object or a separate read and write object.
pub trait EventedReadWrite {
    type Reader: io::Read;
    type Writer: io::Write;

    /// # Safety
    ///
    /// The underlying sources must outlive their registration in the `Poller`.
    unsafe fn register(&mut self, _: &Arc<Poller>, _: Event, _: PollMode) -> io::Result<()>;
    fn reregister(&mut self, _: &Arc<Poller>, _: Event, _: PollMode) -> io::Result<()>;
    fn deregister(&mut self, _: &Arc<Poller>) -> io::Result<()>;

    /// Re-arm a level-triggered read after a deliberately bounded read batch.
    ///
    /// Native polling backends remain readable automatically. Adaptors backed by an in-memory
    /// pipe can override this to register a wakeup for newly buffered bytes.
    fn rearm_read(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn reader(&mut self) -> &mut Self::Reader;
    fn writer(&mut self) -> &mut Self::Writer;
}

/// Events concerning TTY child processes.
#[derive(Debug, PartialEq, Eq)]
pub enum ChildEvent {
    /// Indicates the child has exited.
    Exited(ExitStatus),
    /// The child has exited, but the operating system did not provide a
    /// usable exit status.
    ExitStatusUnavailable,
    /// The child watcher stopped communicating before the exit status could
    /// be observed.
    WatcherDisconnected,
}

/// A pseudoterminal (or PTY).
///
/// This is a refinement of EventedReadWrite that also provides a channel through which we can be
/// notified if the PTY child process does something we care about (other than writing to the TTY).
/// In particular, this allows for race-free child exit notification on UNIX (cf. `SIGCHLD`).
pub trait EventedPty: EventedReadWrite {
    /// Tries to retrieve an event.
    ///
    /// Returns `Some(event)` on success, or `None` if there are no events to retrieve.
    fn next_child_event(&mut self) -> Option<ChildEvent>;

    /// Whether the child belongs to another process, so its exit can only ever
    /// arrive as a report rather than be observed here.
    ///
    /// The event loop treats a hung-up master as a reason to keep waiting,
    /// because for a child it spawned itself the exit notification really is
    /// inevitable. For a foreign child it is not: only the process that forked
    /// it may reap it, so if that report never comes the wait never ends.
    /// Implementations that own their child leave this `false` and keep the
    /// original behaviour exactly.
    fn child_is_foreign(&self) -> bool {
        false
    }

    /// Updates legacy console colors where the platform exposes them.
    fn set_console_palette(&mut self, _palette: ConsolePalette) {}
}

/// Setup environment variables.
pub fn setup_env() {
    // Default to 'alacritty' terminfo if it is available, otherwise
    // default to 'xterm-256color'. May be overridden by user's config
    // below.
    let terminfo = if terminfo_exists("alacritty") { "alacritty" } else { "xterm-256color" };
    unsafe { env::set_var("TERM", terminfo) };

    // Advertise 24-bit color support.
    unsafe { env::set_var("COLORTERM", "truecolor") };
}

/// Check if a terminfo entry exists on the system.
fn terminfo_exists(terminfo: &str) -> bool {
    // Get first terminfo character for the parent directory.
    let first = terminfo.get(..1).unwrap_or_default();
    let first_hex = format!("{:x}", first.chars().next().unwrap_or_default() as usize);

    // Return true if the terminfo file exists at the specified location.
    macro_rules! check_path {
        ($path:expr) => {
            if $path.join(first).join(terminfo).exists()
                || $path.join(&first_hex).join(terminfo).exists()
            {
                return true;
            }
        };
    }

    if let Some(dir) = env::var_os("TERMINFO") {
        check_path!(PathBuf::from(&dir));
    } else if let Some(home) = home::home_dir() {
        check_path!(home.join(".terminfo"));
    }

    if let Ok(dirs) = env::var("TERMINFO_DIRS") {
        for dir in dirs.split(':') {
            check_path!(PathBuf::from(dir));
        }
    }

    if let Ok(prefix) = env::var("PREFIX") {
        let path = PathBuf::from(prefix);
        check_path!(path.join("etc/terminfo"));
        check_path!(path.join("lib/terminfo"));
        check_path!(path.join("share/terminfo"));
    }

    check_path!(PathBuf::from("/etc/terminfo"));
    check_path!(PathBuf::from("/lib/terminfo"));
    check_path!(PathBuf::from("/usr/share/terminfo"));
    check_path!(PathBuf::from("/boot/system/data/terminfo"));

    // No valid terminfo path has been found.
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_console_palette_payload_round_trips() {
        let palette = ConsolePalette {
            colors: std::array::from_fn(|index| [index as u8, 255 - index as u8, index as u8 * 3]),
            foreground_index: 15,
            background_index: 3,
        };
        assert_eq!(
            ConsolePalette::from_private_payload(&palette.to_private_payload()),
            Some(palette)
        );
    }

    #[test]
    fn private_console_palette_payload_rejects_malformed_values() {
        assert_eq!(ConsolePalette::from_private_payload("00"), None);
        let mut payload = ConsolePalette::default().to_private_payload();
        payload.replace_range(96..98, "10");
        assert_eq!(ConsolePalette::from_private_payload(&payload), None);
        payload.replace_range(96..98, "zz");
        assert_eq!(ConsolePalette::from_private_payload(&payload), None);
    }
}
