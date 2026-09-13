//! The remote end of a pane carried over Mosh.
//!
//! Zetta normally reads a remote pane straight off the SSH forward: the
//! multiplexer's framed stream *is* the pane's data plane. Over Mosh it cannot,
//! because Mosh carries a terminal rather than a byte stream. So the byte
//! stream ends here instead, on the host that owns the pane: `zosh-server`
//! runs this command on a PTY of its own, and this copies the pane between
//! that PTY and the multiplexer.
//!
//! The shape that follows from it:
//!
//! - **Its stdio is the pane.** Raw mode goes on before anything is read, so
//!   the line discipline neither echoes the remote program's input back nor
//!   rewrites its bytes, and `Ctrl-C` arrives as the byte `0x03` for the pane
//!   rather than as a signal for this process.
//! - **The replay goes out first**, so the emulator in front of it holds the
//!   screen as it stands before the first live frame arrives.
//! - **`SIGWINCH` is a resize.** Mosh sizes the PTY from the client's own
//!   terminal, so the pane learns the viewer's size the same way any program
//!   under a terminal does. It is reported at the session's current geometry
//!   revision, because the daemon refuses a size that belongs to an older one:
//!   a viewport from before a split says nothing about the layout after it.
//! - **A protected session's secret is read from stdin**, never from `argv`:
//!   the command line of this process is visible to every account on the host,
//!   and the session key would be in it. By the time this runs, stdin is the
//!   inside of an established Mosh link.

use std::{
    io::{self, Read as _, Write as _},
    os::fd::AsRawFd as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context as _, Result};

use crate::{
    auth::SessionSecret,
    client::{AttachOutcome, Client, SharedPane},
    messages::SessionRevision,
};

/// How long a stalled output read waits before the loop looks at everything
/// else it is responsible for. The shared reader already returns `WouldBlock`
/// on its own timeout; this only bounds the case where it returns nothing at
/// all.
const IDLE_POLL: Duration = Duration::from_millis(20);

/// The longest a secret line may be before this gives up on finding its end.
/// A passphrase is not this long, and a peer that never sends a newline must
/// not be able to make this allocate without bound.
const MAX_SECRET_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayOptions {
    pub session_id: u64,
    pub pane_id: u64,
    /// Read the session secret from the first line of stdin before relaying
    /// anything. Needed only for a protected session.
    pub secret_from_stdin: bool,
}

/// Relays one pane between this host's multiplexer and this process's stdio.
///
/// Returns when the pane closes, which is what ends the Mosh session in front
/// of it and, in turn, the pane in the viewer's window.
pub fn run(options: RelayOptions) -> Result<()> {
    // Raw mode first: the secret below must not be echoed back into the
    // terminal that is about to display this pane.
    let raw_mode = RawMode::enter().context("putting the relay's terminal in raw mode")?;
    let secret = options
        .secret_from_stdin
        .then(read_secret_line)
        .transpose()?;

    let (pane, revision) = attach(options, secret.as_ref())?;
    let pane = Arc::new(pane);

    let mut stdout = io::stdout();
    stdout
        .write_all(&pane.replay)
        .and_then(|()| stdout.flush())
        .context("writing the pane's retained output")?;

    let mut revision = revision;
    if let Some((columns, lines)) = terminal_size() {
        pane.send_resize_for_revision(revision, columns, lines)
            .context("reporting the pane's initial size")?;
    }
    let resized = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&resized))
        .context("watching for terminal size changes")?;

    forward_input(Arc::clone(&pane));
    let result = forward_output(&pane, &resized, &mut revision, &mut stdout);
    drop(raw_mode);
    result
}

/// Attaches the pane, and reads the session's geometry revision, which is what
/// a size report has to name.
fn attach(
    options: RelayOptions,
    secret: Option<&SessionSecret>,
) -> Result<(SharedPane, SessionRevision)> {
    let client = Client::connect_existing()
        .context("connecting to this host's multiplexer")?
        .context("no multiplexer is running on this host")?;
    match client
        .attach_shared_with_secret(options.session_id, options.pane_id, secret)
        .context("attaching the pane")?
    {
        AttachOutcome::SharedAttached { pane, .. } => {
            let revision = client
                .shared_snapshot(options.session_id)
                .map(|state| state.revision)
                .unwrap_or(SessionRevision::INITIAL);
            Ok((pane, revision))
        }
        AttachOutcome::Attached { .. } => anyhow::bail!(
            "session {} is held exclusively and cannot be relayed; share it first",
            options.session_id
        ),
        AttachOutcome::AuthenticationRequired => anyhow::bail!(
            "session {} is protected and no secret was supplied",
            options.session_id
        ),
        AttachOutcome::AuthenticationFailed => {
            anyhow::bail!("the secret for session {} was refused", options.session_id)
        }
    }
}

/// Input runs on its own thread because its read blocks: the pane's output has
/// to keep flowing while nobody is typing.
fn forward_input(pane: Arc<SharedPane>) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut bytes = [0_u8; 4096];
        loop {
            match stdin.read(&mut bytes) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    if pane.send_input(&bytes[..count]).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// Copies the pane's output to this process's terminal until the pane closes,
/// reporting a size change whenever one arrives.
fn forward_output(
    pane: &SharedPane,
    resized: &AtomicBool,
    revision: &mut SessionRevision,
    stdout: &mut impl io::Write,
) -> Result<()> {
    let mut reader = pane.reader();
    let mut bytes = [0_u8; 16 * 1024];
    loop {
        // Every size the daemon arbitrates carries the revision it arbitrated
        // at, which is how this side learns that the layout moved on. Nothing
        // is done with the size itself: Mosh owns this terminal's geometry.
        if let Some((arbitrated, columns, lines)) = pane.take_revisioned_sizes().last() {
            *revision = *arbitrated;
            // Unlike the GUI terminal, this relay writes directly to a real
            // terminal whose geometry Mosh owns. It has still observed the
            // size boundary, so let the shared reader continue to the redraw
            // queued after it.
            pane.finish_size_application((*arbitrated, *columns, *lines));
        }
        if resized.swap(false, Ordering::SeqCst)
            && let Some((columns, lines)) = terminal_size()
            && let Err(error) = pane.send_resize_for_revision(*revision, columns, lines)
        {
            // A report can lose a race with a layout change, and the next one
            // carries the newer revision. Losing the pane over it would be
            // worse than the size being briefly wrong.
            eprintln!("zmux relay-pane: could not report the terminal size: {error:#}");
        }
        match reader.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(count) => stdout
                .write_all(&bytes[..count])
                .and_then(|()| stdout.flush())
                .context("writing the pane's output")?,
            // The shared reader reports every recoverable stall this way,
            // including its own read timeout; only an ended pane is an end.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(IDLE_POLL);
            }
            Err(error) => return Err(error).context("reading the pane's output"),
        }
    }
}

/// Reads one line from stdin, which is already in raw mode, so this stops at
/// the newline itself rather than relying on the line discipline.
fn read_secret_line() -> Result<SessionSecret> {
    read_secret_from(&mut io::stdin())
}

fn read_secret_from(input: &mut impl io::Read) -> Result<SessionSecret> {
    let mut secret = zeroize::Zeroizing::new(String::new());
    let mut byte = [0_u8; 1];
    loop {
        let read = input
            .read(&mut byte)
            .context("reading the session secret")?;
        anyhow::ensure!(read != 0, "the session secret was not supplied");
        match byte[0] {
            b'\n' => return Ok(SessionSecret::from_zeroizing(secret)),
            b'\r' => {}
            value => {
                anyhow::ensure!(
                    secret.len() < MAX_SECRET_BYTES,
                    "the session secret is longer than {MAX_SECRET_BYTES} bytes"
                );
                secret.push(char::from(value));
            }
        }
    }
}

fn terminal_size() -> Option<(u16, u16)> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: the descriptor is this process's own standard output and the
    // argument is the `winsize` this request writes.
    let result = unsafe { libc::ioctl(io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
    (result == 0 && size.ws_col != 0 && size.ws_row != 0).then_some((size.ws_col, size.ws_row))
}

/// The terminal settings this process found, restored when it ends.
///
/// A relay whose stdin is not a terminal — a test, or a pipe — has nothing to
/// put in raw mode and nothing to restore.
struct RawMode {
    previous: Option<libc::termios>,
}

impl RawMode {
    fn enter() -> io::Result<Self> {
        let descriptor = io::stdin().as_raw_fd();
        // SAFETY: `isatty` only inspects the descriptor.
        if unsafe { libc::isatty(descriptor) } != 1 {
            return Ok(Self { previous: None });
        }
        let mut settings = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: the descriptor is this process's own standard input and
        // `tcgetattr` initializes the structure it is given.
        if unsafe { libc::tcgetattr(descriptor, settings.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `tcgetattr` returned success, so the value is initialized.
        let previous = unsafe { settings.assume_init() };
        let mut raw = previous;
        // SAFETY: `cfmakeraw` only rewrites the structure it is given.
        unsafe { libc::cfmakeraw(&raw mut raw) };
        // SAFETY: the descriptor is this process's own standard input and
        // `raw` is a complete, initialized settings structure.
        if unsafe { libc::tcsetattr(descriptor, libc::TCSANOW, &raw const raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            previous: Some(previous),
        })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let Some(previous) = self.previous else {
            return;
        };
        // SAFETY: the descriptor is this process's own standard input and
        // `previous` is the settings structure read from it.
        unsafe {
            libc::tcsetattr(io::stdin().as_raw_fd(), libc::TCSANOW, &raw const previous);
        }
    }
}

#[cfg(test)]
#[path = "tests/relay.rs"]
mod tests;
