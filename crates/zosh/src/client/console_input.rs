//! Reading the Windows console: a thread turns crossterm's events into
//! `InputEvent`s and wakes the session loop, which applies them as keys,
//! pastes and resizes.

use super::*;

pub(super) fn millis_until(deadline: Instant) -> u64 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    u64::try_from(remaining.as_micros().div_ceil(1_000)).unwrap_or(u64::MAX)
}

/// What the Windows input thread hands the session loop.
pub(super) enum InputEvent {
    /// A console event, from an interactive console.
    Console(Event),
    /// Bytes from a redirected stdin.
    Bytes(Vec<u8>),
    /// Redirected stdin reached its end.
    Closed,
    /// Reading the console failed.
    Failed(io::Error),
}

/// Reads stdin on a thread of its own and raises `wake` with everything it
/// reads, which is how input on Windows ends the loop's wait: `WSAPoll`
/// watches only sockets, and a console is not one.
pub(super) fn spawn_input_reader(interactive: bool, wake: Arc<Wake>) -> mpsc::Receiver<InputEvent> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let send = |event| {
            let sent = sender.send(event).is_ok();
            wake.notify();
            sent
        };
        if interactive {
            loop {
                match event::read() {
                    Ok(event) => {
                        if !send(InputEvent::Console(event)) {
                            return;
                        }
                    }
                    Err(error) => {
                        send(InputEvent::Failed(error));
                        return;
                    }
                }
            }
        }
        let mut stdin = io::stdin();
        let mut bytes = [0_u8; 4096];
        loop {
            match stdin.read(&mut bytes) {
                Ok(0) | Err(_) => {
                    send(InputEvent::Closed);
                    return;
                }
                Ok(count) => {
                    if !send(InputEvent::Bytes(bytes[..count].to_vec())) {
                        return;
                    }
                }
            }
        }
    });
    receiver
}

/// Applies one console event. A resize is taken as a cue to read the window's
/// size rather than as the size itself: the console reports its screen
/// buffer, which in a classic console with scrollback is taller than the
/// window.
pub(super) fn apply_console_event(
    session: &mut ClientSession,
    escape: &mut EscapeState,
    event: Event,
    size: &mut (u16, u16),
    pending_resize: &mut Option<(u16, u16)>,
) -> Option<EscapeAction> {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat => {
            let bytes = key_bytes(key, session.displayed().application_cursor());
            let (send, action) = escape.feed_all(&bytes);
            if !send.is_empty() {
                session.send_input(&send);
            }
            action
        }
        Event::Paste(text) => {
            session.send_input(text.as_bytes());
            None
        }
        Event::Resize(..) => {
            follow_terminal_size(session, size, pending_resize);
            None
        }
        _ => None,
    }
}
