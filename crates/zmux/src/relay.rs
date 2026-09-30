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
//! - **A lost stream is re-attached, not the end of the pane.** The daemon
//!   ends a viewer's stream when it gives up on it — one that has not read for
//!   half a minute — and a window supplies the replacement itself. This relay
//!   has nobody to do that for it, so it does it: attach again, blank the
//!   screen, write the replay, carry on. Keystrokes typed in between are held
//!   and sent on the new stream rather than dropped.
//! - **Its stderr is the pane.** So nothing is printed there; warnings go to
//!   the host's `daemon.log` alongside the daemon's own (see `logging.rs`).

use std::{
    io::{self, Read as _, Write as _},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};

#[cfg(unix)]
mod agent_link;
#[cfg(unix)]
#[path = "relay/terminal_unix.rs"]
mod terminal;
#[cfg(windows)]
#[path = "relay/terminal_windows.rs"]
mod terminal;

#[cfg(unix)]
use agent_link::ForwardedAgentLink;
use terminal::RawMode;

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

/// How long a lost stream is tried for before the relay gives up and ends,
/// which ends the pane in the viewer's window rather than leaving it frozen.
const REATTACH_PATIENCE: Duration = Duration::from_secs(60);

/// Between two attempts to re-attach, and between two attempts to resend
/// input while the stream is being replaced.
const RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// A gap between two reads of the pane longer than this is logged, with what
/// the relay was doing instead. The daemon gives up on a viewer after 30
/// seconds without progress; this says which step took them.
const READ_GAP_WARNING: Duration = Duration::from_secs(2);

/// A write to the terminal slower than this is logged. The daemon gives up on
/// a viewer that reads nothing for 30 seconds, and a relay reads nothing while
/// it is blocked writing — so this is what says why it did.
const SLOW_WRITE: Duration = Duration::from_secs(1);

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
    crate::logging::init_daemon_log(&crate::paths::session_catalog_dir());
    // Raw mode first: the secret below must not be echoed back into the
    // terminal that is about to display this pane. It is not enough on its own
    // — the prelude can arrive before this process does — which is what
    // [`blank_the_screen`] is for.
    let raw_mode = RawMode::enter().context("putting the relay's terminal in raw mode")?;
    // The Zosh server gives this relay its private forwarded-agent socket. A
    // pane shell predates the relay, so it inherits a stable name owned by the
    // daemon instead; point that name at this session for as long as the relay
    // lives. Failure is deliberately non-fatal: pane transport must continue
    // when agent forwarding is unavailable.
    #[cfg(unix)]
    let _agent_link = match ForwardedAgentLink::install(options.pane_id) {
        Ok(link) => link,
        Err(error) => {
            log::debug!("could not publish the relayed SSH agent: {error:#}");
            None
        }
    };
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

    let mut stdout = terminal::output();
    blank_the_screen(&mut stdout)?;

    let client = Arc::new(
        Client::connect_existing()
            .context("connecting to this host's multiplexer")?
            .context("no multiplexer is running on this host")?,
    );
    // Kept on the client, so the snapshot each size report is stamped from is
    // authorized on a protected session the same way the attach was.
    client.set_session_secret(secret.as_ref());
    let (pane, revision) = attach(&client, options, secret.as_ref(), viewer.clone())?;
    let pane = Arc::new(pane);

    stdout
        .write_all(&pane.replay)
        .and_then(|()| stdout.flush())
        .context("writing the pane's retained output")?;

    let sizes = Arc::new(SizeReports {
        client: Arc::clone(&client),
        pane: Arc::clone(&pane),
        session_id: options.session_id,
        revision: AtomicU64::new(revision.0),
        report_requested: AtomicBool::new(false),
    });
    let reattach = Reattach {
        options,
        secret: secret.as_ref(),
        viewer,
        input_lost: Arc::new(AtomicBool::new(false)),
    };
    // Sent from here rather than through [`SizeReports::report_terminal_size`]:
    // the attach just read this revision, and a relay that cannot report a size
    // at all has nothing to relay, so this one is fatal where a later one is not.
    if let Some((columns, lines)) = terminal::size() {
        pane.send_resize_for_revision(revision, columns, lines)
            .context("reporting the pane's initial size")?;
    }
    let resized = Arc::new(AtomicBool::new(false));
    let ending = Arc::new(AtomicBool::new(false));
    terminal::watch_resizes(Arc::clone(&resized), Arc::clone(&ending))?;
    forward_input(
        Arc::clone(&pane),
        Arc::clone(&reattach.input_lost),
        Arc::clone(&ending),
    );
    report_sizes(Arc::clone(&sizes), resized, Arc::clone(&ending));
    let result = forward_output(&sizes, &reattach, &mut stdout);
    ending.store(true, Ordering::Release);
    if let Err(error) = &result {
        log::warn!(
            "relaying session {} pane {} ended: {error:#}",
            options.session_id,
            options.pane_id
        );
    }
    drop(raw_mode);
    result
}

/// What it takes to attach the pane again after its stream is lost.
struct Reattach<'a> {
    options: RelayOptions,
    secret: Option<&'a SessionSecret>,
    viewer: Option<ClientId>,
    /// Set by the input thread when a keystroke could not be sent: a stream
    /// can break while nothing is being output, and this is how the output
    /// side hears of it.
    input_lost: Arc<AtomicBool>,
}

impl Reattach<'_> {
    fn needed(&self, pane: &SharedPane) -> bool {
        pane.stream_lost() || self.input_lost.load(Ordering::Acquire)
    }

    /// Attaches the pane again and hands the new stream to the reader, with
    /// the screen blanked so the replay repaints it from nothing. Keeps trying
    /// for [`REATTACH_PATIENCE`]; a pane that has gone meanwhile ends the relay
    /// the way it ends any other time.
    fn run(&self, sizes: &SizeReports, stdout: &mut impl io::Write) -> Result<Continue> {
        let (client, pane, session_id) = (&*sizes.client, &*sizes.pane, sizes.session_id);
        log::warn!(
            "relaying session {session_id} pane {}: the stream was lost; attaching again",
            pane.pane_id()
        );
        let deadline = Instant::now() + REATTACH_PATIENCE;
        loop {
            match attach(client, self.options, self.secret, self.viewer.clone()) {
                Ok((replacement, revision)) => {
                    blank_the_screen(stdout)?;
                    pane.replace_connection_from(&replacement)
                        .context("installing the re-attached stream")?;
                    self.input_lost.store(false, Ordering::Release);
                    sizes.revision.store(revision.0, Ordering::Release);
                    // The new attachment is unmeasured until it reports. Left
                    // to the size thread, like every other report, so this
                    // loop does not wait on the daemon to answer it.
                    sizes.report_requested.store(true, Ordering::Release);
                    log::warn!(
                        "relaying session {session_id} pane {}: attached again",
                        pane.pane_id()
                    );
                    return Ok(Continue::Relaying);
                }
                Err(error) if pane_is_gone(&error) => {
                    log::warn!(
                        "relaying session {session_id} pane {}: the pane has gone: {error:#}",
                        pane.pane_id()
                    );
                    return Ok(Continue::Ended);
                }
                Err(error) if Instant::now() < deadline => {
                    log::debug!("re-attaching the relayed pane failed, retrying: {error:#}");
                    thread::sleep(RETRY_INTERVAL);
                }
                Err(error) => {
                    return Err(error).context("re-attaching the relayed pane");
                }
            }
        }
    }
}

enum Continue {
    Relaying,
    Ended,
}

/// Whether an attach failed because there is no longer a pane to attach.
fn pane_is_gone(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}");
    message.contains("does not exist")
        || message.contains("has no pane")
        || message.contains("has ended")
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
///
/// A keystroke that cannot be sent is held, not dropped: the stream is being
/// replaced (see [`Reattach`]), and the writer it is sent through follows the
/// replacement. Giving up on the first failure used to end this thread, and
/// with it every keystroke the pane would ever get.
fn forward_input(pane: Arc<SharedPane>, input_lost: Arc<AtomicBool>, ending: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut bytes = [0_u8; 4096];
        loop {
            let count = match stdin.read(&mut bytes) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            let deadline = Instant::now() + REATTACH_PATIENCE;
            while let Err(error) = pane.send_input(&bytes[..count]) {
                if ending.load(Ordering::Acquire) || Instant::now() >= deadline {
                    log::warn!("dropping relayed input after the stream was lost: {error:#}");
                    return;
                }
                input_lost.store(true, Ordering::Release);
                thread::sleep(RETRY_INTERVAL);
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
///
/// Reporting happens on a thread of its own ([`report_sizes`]). Asking the
/// daemon for the revision is a request with a timeout of fifteen seconds, and
/// on the output thread every `SIGWINCH` held the pane's output until it was
/// answered — long enough, twice over, for the daemon to give up on this relay
/// as a viewer that had stopped reading.
struct SizeReports {
    client: Arc<Client>,
    pane: Arc<SharedPane>,
    session_id: u64,
    /// The last revision this relay knows of, as a [`SessionRevision`].
    revision: AtomicU64,
    /// A report is wanted even without a resize: after a re-attach, whose new
    /// attachment is unmeasured until it reports.
    report_requested: AtomicBool,
}

impl SizeReports {
    /// Records the revision an arbitrated size arrived at, and releases the
    /// output held behind it.
    ///
    /// Nothing is done with the size itself: unlike the GUI terminal, this
    /// relay writes to a real terminal whose geometry Mosh owns. It has still
    /// observed the size boundary, so the shared reader may continue to the
    /// redraw queued after it.
    fn observe_arbitrated(&self) {
        if let Some(&(arbitrated, columns, lines)) = self.pane.take_revisioned_sizes().last() {
            self.revision.store(arbitrated.0, Ordering::Release);
            self.pane
                .finish_size_application((arbitrated, columns, lines));
        }
        // Whatever frame is still holding the stream is released too. The hold
        // is for a terminal that must apply a grid before the output drawn for
        // it, and this relay applies none. Waiting on the exact release is how
        // a relay once stopped reading its pane — every read answered "nothing
        // yet" without touching the socket — until the daemon gave up on it.
        if let Some(held) = self.pane.release_size_hold() {
            log::warn!(
                "relaying session {} pane {}: released output held behind a size frame \
                 ({} columns, {} lines, revision {}) that was never matched",
                self.session_id,
                self.pane.pane_id(),
                held.1,
                held.2,
                (held.0).0
            );
        }
    }

    /// Reports the terminal's current size at the session's current revision.
    fn report_terminal_size(&self) {
        let Some((columns, lines)) = terminal::size() else {
            return;
        };
        let revision = self.current_revision();
        self.revision.store(revision.0, Ordering::Release);
        if let Err(error) = self.pane.send_resize_for_revision(revision, columns, lines) {
            // A report can lose a race with a layout change, and the next one
            // carries the newer revision. Losing the pane over it would be
            // worse than the size being briefly wrong.
            log::warn!("relay-pane could not report the terminal size: {error:#}");
        }
    }

    fn current_revision(&self) -> SessionRevision {
        let known = SessionRevision(self.revision.load(Ordering::Acquire));
        let started = Instant::now();
        let answer = self.client.shared_snapshot(self.session_id);
        let took = started.elapsed();
        if took >= READ_GAP_WARNING {
            log::warn!(
                "relaying pane {} of session {}: the daemon took {took:?} to report the \
                 session's revision",
                self.pane.pane_id(),
                self.session_id
            );
        }
        match answer {
            Ok(state) => state.revision,
            Err(error) => {
                log::debug!(
                    "relaying pane {} of session {}: could not read the current revision, \
                     reporting at {}: {error:#}",
                    self.pane.pane_id(),
                    self.session_id,
                    known.0
                );
                known
            }
        }
    }
}

/// Reports the terminal's size whenever Mosh resizes it, off the output
/// thread: see [`SizeReports`].
fn report_sizes(sizes: Arc<SizeReports>, resized: Arc<AtomicBool>, ending: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !ending.load(Ordering::Acquire) {
            let resize = resized.swap(false, Ordering::SeqCst);
            let requested = sizes.report_requested.swap(false, Ordering::AcqRel);
            if resize || requested {
                sizes.report_terminal_size();
            }
            thread::sleep(IDLE_POLL);
        }
    });
}

/// Copies the pane's output to this process's terminal until the pane closes,
/// reporting a size change whenever one arrives.
fn forward_output(
    sizes: &SizeReports,
    reattach: &Reattach<'_>,
    stdout: &mut impl io::Write,
) -> Result<()> {
    let mut reader = sizes.pane.reader();
    let mut bytes = [0_u8; 16 * 1024];
    let mut last_read = Instant::now();
    let mut doing = "starting";
    loop {
        sizes.observe_arbitrated();
        let gap = last_read.elapsed();
        if gap >= READ_GAP_WARNING {
            log::warn!(
                "relaying session {} pane {}: the pane went unread for {gap:?} while {doing}",
                sizes.session_id,
                sizes.pane.pane_id()
            );
        }
        let read = reader.read(&mut bytes);
        last_read = Instant::now();
        match read {
            Ok(0) => return Ok(()),
            Ok(count) => {
                doing = "writing its output to the Mosh terminal";
                write_output(stdout, &bytes[..count])?;
            }
            // The shared reader reports every recoverable stall this way,
            // including its own read timeout and a stream that broke with no
            // replacement yet; only an ended pane is an end.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if reattach.needed(&sizes.pane) {
                    doing = "attaching the pane again";
                    match reattach.run(sizes, stdout)? {
                        Continue::Relaying => continue,
                        Continue::Ended => return Ok(()),
                    }
                }
                doing = "idle";
                thread::sleep(IDLE_POLL);
            }
            Err(error) => return Err(error).context("reading the pane's output"),
        }
    }
}

/// Writes to the terminal `zosh-server` gave this relay, saying so when that
/// took long enough to matter: while it blocks, nothing reads the pane.
fn write_output(stdout: &mut impl io::Write, bytes: &[u8]) -> Result<()> {
    let started = Instant::now();
    stdout
        .write_all(bytes)
        .and_then(|()| stdout.flush())
        .context("writing the pane's output")?;
    let took = started.elapsed();
    if took >= SLOW_WRITE {
        log::warn!(
            "writing {} bytes of pane output to the Mosh terminal took {took:?}; the pane was \
             not read meanwhile",
            bytes.len()
        );
    }
    Ok(())
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

#[cfg(test)]
#[path = "tests/relay.rs"]
mod tests;
