//! The existing native PTY read before joining the parser on child exit.
//! ConPTY's additional source-thread queue needs its own final-drain policy;
//! keeping this path separate preserves other platforms' exit behavior.

use super::*;

impl<T, U> EventLoop<T, U>
where
    T: tty::EventedPty + event::OnResize + Send + 'static,
    U: EventListener + Clone + Send + 'static,
{
    pub(super) fn drain_final_pty<X: Write>(
        &mut self,
        state: &mut State,
        parser: &mut ParserThread,
        writer: Option<&mut X>,
    ) {
        let _ = self.pty_read(state, parser, writer);
    }
}
