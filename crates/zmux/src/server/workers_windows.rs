//! Windows drain readiness: the control socket and bounded ConPTY pipe queues
//! share one IOCP poller. Only daemon-owned readers may be registered, because
//! registering a lazy pipe reader starts its source thread.

use super::*;
use alacritty_terminal::tty::EventedReadWrite as _;
use polling::{Event, Events, PollMode, Poller};

const CONTROL: usize = 0;
const PIPE: usize = 1;

pub(in crate::server) struct DrainWait {
    waker: Stream,
    poller: Arc<Poller>,
    events: Events,
}

impl DrainWait {
    pub(super) fn new(waker: Stream) -> Self {
        let poller = Arc::new(Poller::new().expect("creating the drain poller"));
        // SAFETY: this object owns the socket and deletes its registration in
        // Drop, before either the socket or poller is destroyed.
        unsafe { poller.add(&waker, Event::readable(CONTROL)) }
            .expect("registering the drain control socket");
        Self {
            waker,
            poller,
            events: Events::new(),
        }
    }

    pub(super) fn wait(&mut self, daemon: &Arc<Daemon>, mut timeout: Duration) {
        let backlogged = persistence_backlogged(daemon);
        {
            let mut sessions = daemon
                .sessions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for pane in sessions
                .iter_mut()
                .flat_map(|session| session.panes.iter_mut())
            {
                let owned = !pane.exited && drain_reads(&pane.attachment);
                let mut relay_blocked = false;
                if owned && let Attachment::Shared(clients) = &pane.attachment {
                    for client in clients {
                        if client.relay.queued.load(Ordering::Relaxed) >= RELAY_BACKPRESSURE_BYTES {
                            relay_blocked = true;
                            timeout = timeout
                                .min(RELAY_STALL_TIMEOUT.saturating_sub(client.wrote_at.elapsed()));
                        }
                    }
                }
                arm_pipes(
                    &mut pane.pty,
                    &self.poller,
                    owned && !backlogged && !relay_blocked,
                    owned && !pane.pending_input.is_empty(),
                );
            }
        }
        // Do not consume control bytes before waiting: a transition between the
        // last drain pass and registration must cause another pass immediately.
        if let Err(error) = self.wait_ready(timeout) {
            log::debug!("waiting on the drain poller failed: {error:#}");
            thread::sleep(HANGUP_BACKOFF);
        }
    }

    fn wait_ready(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.poller.modify(&self.waker, Event::readable(CONTROL))?;
        self.events.clear();
        self.poller.wait(&mut self.events, Some(timeout))?;
        drain_waker(&mut self.waker);
        Ok(())
    }
}

/// Recheck buffered readiness when arming, covering completion between the last
/// read/write and registration. Removing interest never resumes a paused reader;
/// pause_pane_reader remains the ownership barrier before handing out handles.
fn arm_pipes(pty: &mut tty::Pty, poller: &Arc<Poller>, read: bool, write: bool) {
    if read {
        pty.reader()
            .register(poller, Event::readable(PIPE), PollMode::Oneshot);
        pty.reader().rearm();
    } else {
        pty.reader().deregister();
    }
    if write {
        pty.writer()
            .register(poller, Event::writable(PIPE), PollMode::Oneshot);
    } else {
        pty.writer().deregister();
    }
}

impl Drop for DrainWait {
    fn drop(&mut self) {
        let _ = self.poller.delete(&self.waker);
    }
}

#[cfg(test)]
#[path = "../tests/server/workers_windows.rs"]
mod tests;
