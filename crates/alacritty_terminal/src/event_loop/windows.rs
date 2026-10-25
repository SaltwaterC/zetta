//! Final ConPTY output draining after a child exit. The source thread can hold
//! more than one gather batch, and an empty in-memory read is not native EOF:
//! the source may still be publishing output. Drain the bounded read-ahead
//! until the console's output pipe ends, which a released pseudoconsole does
//! right after its last client exits. Where the end cannot be observed, wait
//! briefly for refills instead, before the parser is joined and exit is reported.

use super::*;
use std::time::Duration;

// ConPTY can publish its last buffered bytes shortly after the child exit is
// reported. Leave enough room for the source reader to deliver that refill
// before treating an empty read as the end of output.
const QUIET_PERIOD: Duration = Duration::from_millis(100);
// The parser pool, source pipe and kernel pipe are bounded. A generous cap
// also ends the drain if a surviving descendant continues writing after the
// watched child exits; it must not keep the window open indefinitely.
const MAX_FINAL_BYTES: usize = 4 * READ_BUFFER_SIZE;

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Clone + Send + 'static,
{
    pub(super) fn drain_final_pty<X: Write>(
        &mut self,
        state: &mut State,
        parser: &mut ParserThread,
        mut writer: Option<&mut X>,
    ) {
        let mut deadline = Instant::now() + QUIET_PERIOD;
        let mut drained = 0;
        let mut events = Events::new();
        loop {
            match self.pty_read(state, parser, writer.as_deref_mut()) {
                Ok(0) if self.pty.output_ended() => break,
                Ok(0) => {},
                Ok(count) => {
                    drained += count;
                    if drained >= MAX_FINAL_BYTES {
                        break;
                    }
                    // Backpressure from a blocked parser is not a quiet pipe.
                    // Start the quiet period after publishing each real batch.
                    deadline = Instant::now() + QUIET_PERIOD;
                    continue;
                },
                Err(_) => break,
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || self.pty.rearm_read().is_err() {
                break;
            }
            events.clear();
            if self.poll.wait(&mut events, Some(remaining)).is_err() {
                break;
            }
        }
    }
}
