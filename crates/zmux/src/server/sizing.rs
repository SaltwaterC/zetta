//! The size a pane's pty runs at.
//!
//! A shared pane has several viewers with different windows, so its size is
//! arbitrated down to the smallest of them rather than taken from whichever
//! client resized last: a client showing fewer columns than the pty would
//! otherwise see the shell wrap where its own grid does not.

use super::*;

/// The size every shared client must show the pane at: the smallest any of
/// them asked for, falling back to the size the daemon last applied.
pub(super) fn effective_size(pane: &Pane) -> (u16, u16) {
    match &pane.attachment {
        Attachment::Shared(clients) => smallest_size(
            clients.iter().map(|client| (client.columns, client.lines)),
            pane.size,
        ),
        _ => (pane.size.columns, pane.size.lines),
    }
}

/// The smallest of the sizes asked for, or `fallback` when none were.
///
/// Split out from [`effective_size`] because a `SharedClient` owns a live
/// connection: the arbitration is the part worth testing on its own, and it
/// cannot be tested through a type that needs a socket to exist.
///
/// Independently per axis, as tmux does — the pane has to fit inside every
/// viewer, and a viewer that is wider but shorter constrains only the height.
pub(super) fn smallest_size(
    sizes: impl Iterator<Item = (u16, u16)>,
    fallback: TerminalSize,
) -> (u16, u16) {
    let mut smallest: Option<(u16, u16)> = None;
    for (columns, lines) in sizes {
        smallest = Some(match smallest {
            Some((best_columns, best_lines)) => (best_columns.min(columns), best_lines.min(lines)),
            None => (columns, lines),
        });
    }
    smallest.unwrap_or((fallback.columns, fallback.lines))
}

/// Applies a size to a pane's terminal, recording it as the pane's size.
pub(super) fn apply_size(_daemon: &Daemon, pane: &mut Pane, columns: u16, lines: u16) {
    use alacritty_terminal::event::OnResize as _;
    #[cfg(windows)]
    if let Err(error) = _daemon.pty_host.resize(pane.console_id, columns, lines) {
        log::warn!(
            "could not resize pseudoconsole {} to {columns}x{lines}: {error:#}",
            pane.console_id
        );
    }
    pane.pty.on_resize(window_size(TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    }));
    pane.size = TerminalSize {
        columns,
        lines,
        cell_width: 0,
        cell_height: 0,
    };
    // What is kept of the pane wraps where the pane wraps, or a reattach would
    // show the session rewrapped at a width nothing was drawn at.
    pane.retained.resize(columns, lines);
}

/// The size the pane's terminal is actually running at.
///
/// Asked of the terminal rather than remembered. A client spawns a pane before
/// its window has laid the pane out, so the size it sends is a stand-in — 80x24 —
/// and on Unix the resize that follows goes straight to the descriptor that
/// client holds, telling the multiplexer nothing. The remembered size is
/// therefore the stand-in for the whole life of an exclusively-held pane, and
/// seeding a 98x51 screen into a grid that size came back rewrapped and
/// interleaved: a full-screen program's screen crushed into 80 columns and 24
/// rows, which is what a joined session looked like.
pub(super) fn terminal_size(pane: &Pane) -> Option<(u16, u16)> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd as _;

        let mut size: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: the descriptor is this process's pty master, and `size` is a
        // `winsize` the kernel fills in.
        let read =
            unsafe { libc::ioctl(pane.pty.file().as_raw_fd(), libc::TIOCGWINSZ, &raw mut size) };
        (read == 0 && size.ws_col > 0 && size.ws_row > 0).then_some((size.ws_col, size.ws_row))
    }
    #[cfg(not(unix))]
    {
        let _ = pane;
        None
    }
}

/// Gives the retained screen the geometry the handed-over screen was drawn at,
/// then reads that screen into it.
///
/// The two have to agree: a snapshot is lines of text with no width of their own,
/// so a grid of a different width wraps them somewhere the program never did.
///
/// The remembered size is brought along for the same reason. On Unix a client
/// resizes through the descriptor it holds and tells the multiplexer nothing, so
/// `pane.size` is still the stand-in the spawn carried — and an upgrade records
/// *that* for the pane and rebuilds the retained screen from it in the next
/// image. A screen drawn at the real width, rebuilt at the stand-in's, comes back
/// as a full-screen program wrapped into fragments of itself.
pub(super) fn seed_retained_screen(pane: &mut Pane, snapshot: Vec<u8>) {
    if let Some((columns, lines)) = terminal_size(pane) {
        pane.retained.resize(columns, lines);
        pane.size.columns = columns;
        pane.size.lines = lines;
    }
    pane.retained.seed(snapshot);
}

/// Keeps a copy of `chunk` for a client that is mid-handover.
///
/// It handed its screen over and has not rejoined yet, so these are the bytes it
/// alone is missing. Bounded, and oldest-first when it overflows: a handover that
/// never completes must not grow without limit.
pub(super) fn record_handover_output(pane: &mut Pane, chunk: &[u8]) {
    let Some(handover) = pane.handed_over.as_mut() else {
        return;
    };
    handover.output.extend_from_slice(chunk);
    if handover.output.len() > HANDOVER_OUTPUT_LIMIT {
        let excess = handover.output.len() - HANDOVER_OUTPUT_LIMIT;
        handover.output.drain(..excess);
    }
}

/// Tells every shared client the size the pane is now shown at.
pub(super) fn broadcast_size(
    session_id: u64,
    pane_id: u64,
    attachment: &mut Attachment,
    handover_waiters: usize,
    columns: u16,
    lines: u16,
) {
    if !matches!(attachment, Attachment::Shared(_)) {
        return;
    }
    let event = Event::Size {
        session_id,
        pane_id,
        columns,
        lines,
    };
    // Queued alongside the pane's output rather than written past it, so a
    // viewer applies the new size at the point in the stream where it happened.
    match crate::transport::encode_message(&event) {
        Ok(frame) => queue_for_shared_clients(attachment, handover_waiters, &Arc::from(frame)),
        Err(error) => log::warn!("could not frame a pane's new size: {error:#}"),
    }
}

/// Hands one framed message to every shared client, dropping those that cannot
/// take it.
///
/// `try_send` never blocks: a full queue means this viewer is not keeping up, and
/// making the pane wait for it would stall every other viewer and the drain with
/// it. Collapses the attachment when that leaves nobody.
pub(super) fn queue_for_shared_clients(
    attachment: &mut Attachment,
    handover_waiters: usize,
    frame: &Arc<[u8]>,
) {
    let Attachment::Shared(clients) = attachment else {
        return;
    };
    let mut failed = Vec::new();
    for client in clients.iter() {
        // No size ceiling here. Dropping a viewer for having a large backlog
        // meant dropping every viewer of any sustained output, because a terminal
        // that parses and renders is always slower than a program that only
        // writes: `zetta benchmark output --size 1000` cut the viewer off 4 MiB in,
        // every time, and left the pane connected to nothing. The backlog is
        // bounded by `relay_backpressure`, which stops reading the pane instead of
        // punishing whoever is reading it.
        client
            .relay
            .queued
            .fetch_add(frame.len(), Ordering::Relaxed);
        if client.relay.frames.try_send(frame.clone()).is_err() {
            client
                .relay
                .queued
                .fetch_sub(frame.len(), Ordering::Relaxed);
            failed.push(client.client_id.clone());
        }
    }
    if failed.is_empty() {
        return;
    }
    log::debug!(
        "dropped {} shared client(s) that stopped keeping up",
        failed.len()
    );
    clients.retain(|client| !failed.contains(&client.client_id));
    collapse_empty_shared(attachment, handover_waiters);
}

/// The exclusive attachment a client process id maps to. A client that does
/// not identify itself (`0`) is recorded as holding nothing, which preserves
/// the original behaviour for test paths that do not name a process.
pub(super) fn exclusive_attachment(client_process_id: u32) -> Attachment {
    match client_process_id {
        0 => Attachment::None,
        process_id => Attachment::Exclusive(process_id),
    }
}

/// How long an attach waits for a pane's holder to answer a revoke before
/// giving up. Generous: the holder has to stop its terminal, snapshot a large
/// screen, and re-attach, all between two network round trips.
pub(super) const REVOKE_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(all(test, unix))]
#[path = "../tests/server/sizing.rs"]
mod tests;
