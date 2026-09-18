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
//! - **The screen is blanked once the prelude has been read**, because raw mode
//!   cannot be on before this process exists and the prelude arrives on a Mosh
//!   link that was established before it was started. Whatever the line
//!   discipline echoed in between is on the screen and is not the pane's. The
//!   race cannot be won from here — `zosh-server` opens the PTY, and the
//!   remote server may be a stock `mosh-server` that takes no say in the
//!   matter — so what is on the screen is discarded rather than prevented. An
//!   echo can therefore still be painted for the frame it arrived in; it
//!   cannot survive into the pane.
//! - **The replay goes out first**, so the emulator in front of it holds the
//!   screen as it stands before the first live frame arrives.
//! - **`SIGWINCH` is a resize.** Mosh sizes the PTY from the client's own
//!   terminal, so the pane learns the viewer's size the same way any program
//!   under a terminal does. It is reported at the session's current geometry
//!   revision, because the daemon refuses a size that belongs to an older one:
//!   a viewport from before a split says nothing about the layout after it.
//!   That revision is asked of the daemon as each report is sent — see
//!   [`SizeReports`] for why remembering it is not enough.
//! - **A protected session's secret is read from stdin**, never from `argv`:
//!   the command line of this process is visible to every account on the host,
//!   and the session key would be in it. By the time this runs, stdin is the
//!   inside of an established Mosh link. The viewer's client ID travels the
//!   same way and for the same reason: it is what the daemon matches a control
//!   request against, so on a command line it would let any account on this
//!   host pose as the window this relay serves. When both are read, the secret
//!   is the first line and the viewer the second.

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
    messages::{ClientId, SessionRevision},
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
    /// Read the client ID of the window this pane is being relayed to from the
    /// next line of stdin, and declare it to the daemon as the viewer this
    /// attachment stands in for. Without it that window is nowhere in the
    /// pane's shared set, and its own control requests — pasting an image — are
    /// refused as coming from a client that is not watching the pane.
    pub viewer_from_stdin: bool,
}

/// Relays one pane between this host's multiplexer and this process's stdio.
///
/// Returns when the pane closes, which is what ends the Mosh session in front
/// of it and, in turn, the pane in the viewer's window.
pub fn run(options: RelayOptions) -> Result<()> {
    // Raw mode first: the secret below must not be echoed back into the
    // terminal that is about to display this pane. It is not enough on its own
    // — the prelude can arrive before this process does — which is what
    // [`blank_the_screen`] is for.
    let raw_mode = RawMode::enter().context("putting the relay's terminal in raw mode")?;
    // In this order, because it is the order the viewer writes them and neither
    // line is self-describing.
    let secret = options
        .secret_from_stdin
        .then(read_secret_line)
        .transpose()?;
    let viewer = options
        .viewer_from_stdin
        .then(read_viewer_line)
        .transpose()?;

    let mut stdout = io::stdout();
    blank_the_screen(&mut stdout)?;

    let client = Client::connect_existing()
        .context("connecting to this host's multiplexer")?
        .context("no multiplexer is running on this host")?;
    // Kept on the client, so the snapshot each size report is stamped from is
    // authorized on a protected session the same way the attach was.
    client.set_session_secret(secret.as_ref());
    let (pane, revision) = attach(&client, options, secret.as_ref(), viewer)?;
    let pane = Arc::new(pane);

    stdout
        .write_all(&pane.replay)
        .and_then(|()| stdout.flush())
        .context("writing the pane's retained output")?;

    let mut sizes = SizeReports {
        client: &client,
        pane: &pane,
        session_id: options.session_id,
        revision,
    };
    // Sent from here rather than through [`SizeReports::report_terminal_size`]:
    // the attach just read this revision, and a relay that cannot report a size
    // at all has nothing to relay, so this one is fatal where a later one is not.
    if let Some((columns, lines)) = terminal_size() {
        pane.send_resize_for_revision(revision, columns, lines)
            .context("reporting the pane's initial size")?;
    }
    let resized = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&resized))
        .context("watching for terminal size changes")?;

    forward_input(Arc::clone(&pane));
    let result = forward_output(&mut sizes, &resized, &mut stdout);
    drop(raw_mode);
    result
}

/// Discards everything on the terminal this relay was given.
///
/// The prelude travels inside an established Mosh link, so it can reach this
/// PTY before this process has run at all — and until [`RawMode::enter`] has,
/// the line discipline echoes whatever arrives. Losing that race put the
/// viewer's client ID on the first line of the pane, above its first prompt.
/// The race cannot be won from this side, but the screen does not have to be
/// kept: nothing on it belongs to the pane, whose every byte is written below.
fn blank_the_screen(output: &mut impl io::Write) -> Result<()> {
    output
        .write_all(BLANK_SCREEN)
        .and_then(|()| output.flush())
        .context("clearing the relay's terminal")
}

/// Erase the screen, erase what has scrolled off it, and home the cursor.
const BLANK_SCREEN: &[u8] = b"\x1b[2J\x1b[3J\x1b[H";

/// Attaches the pane, and reads the session's geometry revision, which is what
/// a size report has to name.
fn attach(
    client: &Client,
    options: RelayOptions,
    secret: Option<&SessionSecret>,
    viewer: Option<ClientId>,
) -> Result<(SharedPane, SessionRevision)> {
    let attached = match viewer {
        Some(viewer) => {
            client.attach_shared_relaying_for(options.session_id, options.pane_id, secret, viewer)
        }
        None => client.attach_shared_with_secret(options.session_id, options.pane_id, secret),
    };
    match attached.context("attaching the pane")? {
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

/// Keeps the pane's size in step with the terminal Mosh gives this relay.
///
/// The revision is the difficulty. The daemon drops a size report that does not
/// name the revision it is on, and says nothing about having done so; this side
/// hears about a revision at all only when an arbitrated size happens to be
/// broadcast to it, which for a pane whose one viewer is this relay never
/// happens. So the revision an attach started from was the only one this relay
/// ever reported at: adding a second pane to the session moved the session on,
/// and from then on every resize of the pane this relay carries was discarded
/// in silence. The window showed 98 columns while the shell inside it went on
/// wrapping at 198.
///
/// The revision is therefore asked of the daemon as each report is sent, which
/// is what the windowed client does too — it reads the revision at the moment
/// it sends rather than the one the layout it measured was for. A remembered
/// revision is still the fallback, so a daemon that cannot answer leaves the
/// reporting no worse than it was.
struct SizeReports<'a> {
    client: &'a Client,
    pane: &'a SharedPane,
    session_id: u64,
    revision: SessionRevision,
}

impl SizeReports<'_> {
    /// Records the revision an arbitrated size arrived at, and releases the
    /// output held behind it.
    ///
    /// Nothing is done with the size itself: unlike the GUI terminal, this
    /// relay writes to a real terminal whose geometry Mosh owns. It has still
    /// observed the size boundary, so the shared reader may continue to the
    /// redraw queued after it.
    fn observe_arbitrated(&mut self) {
        let Some(&(arbitrated, columns, lines)) = self.pane.take_revisioned_sizes().last() else {
            return;
        };
        self.revision = arbitrated;
        self.pane
            .finish_size_application((arbitrated, columns, lines));
    }

    /// Reports the terminal's current size at the session's current revision.
    fn report_terminal_size(&mut self) {
        let Some((columns, lines)) = terminal_size() else {
            return;
        };
        self.revision = self.current_revision();
        if let Err(error) = self
            .pane
            .send_resize_for_revision(self.revision, columns, lines)
        {
            // A report can lose a race with a layout change, and the next one
            // carries the newer revision. Losing the pane over it would be
            // worse than the size being briefly wrong.
            eprintln!("zmux relay-pane: could not report the terminal size: {error:#}");
        }
    }

    fn current_revision(&self) -> SessionRevision {
        match self.client.shared_snapshot(self.session_id) {
            Ok(state) => state.revision,
            Err(error) => {
                log::debug!(
                    "relaying pane {} of session {}: could not read the current revision, \
                     reporting at {}: {error:#}",
                    self.pane.pane_id(),
                    self.session_id,
                    self.revision.0
                );
                self.revision
            }
        }
    }
}

/// Copies the pane's output to this process's terminal until the pane closes,
/// reporting a size change whenever one arrives.
fn forward_output(
    sizes: &mut SizeReports<'_>,
    resized: &AtomicBool,
    stdout: &mut impl io::Write,
) -> Result<()> {
    let mut reader = sizes.pane.reader();
    let mut bytes = [0_u8; 16 * 1024];
    loop {
        sizes.observe_arbitrated();
        if resized.swap(false, Ordering::SeqCst) {
            sizes.report_terminal_size();
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

/// Reads the viewer's client ID from the next line of stdin.
///
/// Read with the secret's reader because it has the same two requirements: the
/// terminal is in raw mode, so the line's end has to be found here, and a peer
/// that never sends one must not be able to make this allocate without bound.
/// Everything typed after it belongs to the pane.
fn read_viewer_line() -> Result<ClientId> {
    read_viewer_from(&mut io::stdin())
}

fn read_viewer_from(input: &mut impl io::Read) -> Result<ClientId> {
    let line = read_secret_from(input).context("reading the relayed viewer's client ID")?;
    let viewer = line.expose().trim();
    // An empty line is not an identity, and treating it as one would name a
    // client that cannot exist — while looking, in the daemon's state, exactly
    // like a relay that had declared a real one.
    anyhow::ensure!(
        !viewer.is_empty(),
        "the relayed viewer's client ID was not supplied"
    );
    Ok(ClientId::new(viewer))
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
