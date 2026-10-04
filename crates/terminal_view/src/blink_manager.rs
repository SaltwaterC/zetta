//! Cursor blink cadence and input pauses, separate from terminal input/rendering.
//!
//! Input makes the cursor visible immediately and moves a single pause task's
//! deadline to 500 ms later. Epochs invalidate both ordinary blink callbacks and
//! pauses when blinking is disabled; stale callbacks must not resume blinking.

use std::time::{Duration, Instant};

use gpui::{Context, Task};

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub(super) struct BlinkManager {
    blink_epoch: usize,
    paused: bool,
    pub(super) visible: bool,
    enabled: bool,
    pause_deadline: Option<(Instant, usize)>,
    pause_task: Option<Task<()>>,
}

impl BlinkManager {
    pub(super) fn new() -> Self {
        Self {
            blink_epoch: 0,
            paused: false,
            visible: true,
            enabled: false,
            pause_deadline: None,
            pause_task: None,
        }
    }

    fn next_epoch(&mut self) -> usize {
        self.blink_epoch += 1;
        self.blink_epoch
    }

    pub(super) fn enable(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        self.visible = false;
        self.blink(self.blink_epoch, cx);
    }

    pub(super) fn disable(&mut self, cx: &mut Context<Self>) {
        self.enabled = false;
        self.visible = true;
        self.next_epoch();
        cx.notify();
    }

    pub(super) fn pause(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.paused = true;
        let epoch = self.next_epoch();
        self.pause_deadline = Some((
            cx.background_executor().now() + CURSOR_BLINK_INTERVAL,
            epoch,
        ));
        cx.notify();
        if self.pause_task.is_none() {
            self.pause_task = Some(cx.spawn(async move |this, cx| {
                loop {
                    let remaining = this.update(cx, Self::resume_pause).ok().flatten();
                    let Some(remaining) = remaining else {
                        break;
                    };
                    cx.background_executor().timer(remaining).await;
                }
            }));
        }
    }

    /// An extended pause sleeps only until the latest input's deadline, never
    /// another full interval from this wake. Use the executor clock so tests
    /// exercise the same deadline calculation as native scheduling.
    fn resume_pause(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        let (deadline, epoch) = self.pause_deadline?;
        if epoch == self.blink_epoch {
            let remaining = deadline.saturating_duration_since(cx.background_executor().now());
            if !remaining.is_zero() {
                return Some(remaining);
            }
            self.paused = false;
            self.blink(epoch, cx);
        }
        // A disable invalidates the pause without resuming it, just as the
        // detached timer's epoch check did. A later input may start a new pause.
        self.pause_deadline = None;
        self.pause_task.take();
        None
    }

    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if !self.enabled || self.paused || epoch != self.blink_epoch {
            return;
        }
        self.visible = !self.visible;
        cx.notify();
        let next_epoch = self.next_epoch();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CURSOR_BLINK_INTERVAL).await;
            this.update(cx, |this, cx| this.blink(next_epoch, cx)).ok();
        })
        .detach();
    }
}

#[cfg(test)]
#[path = "tests/blink_manager.rs"]
mod tests;
