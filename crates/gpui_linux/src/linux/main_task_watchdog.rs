//! Zetta patch: a watchdog for foreground tasks that block the event loop.
//!
//! Every foreground runnable is bracketed by [`MainTaskWatchdog::begin`], and a
//! thread reports on stderr any that has run for more than
//! [`MAIN_TASK_STALL_THRESHOLD`], with the location that spawned it. The loop
//! is otherwise silent while it is stuck, because the stuck task is what would
//! have logged. [`MainTaskWatchdog::active_summary`] names the task that was
//! running if the event loop itself terminates.
//!
//! It lived in the Wayland client until upstream moved runnable dispatch and the
//! event loop into the shared `LinuxPlatform`; it now watches every backend.

use std::{
    panic::Location,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

const MAIN_TASK_STALL_THRESHOLD: Duration = Duration::from_secs(2);

struct ActiveMainTask {
    id: u64,
    location: &'static Location<'static>,
    started: Instant,
}

#[derive(Default)]
struct MainTaskWatchdogState {
    next_id: u64,
    active: Option<ActiveMainTask>,
}

pub(crate) struct MainTaskWatchdog {
    state: Mutex<MainTaskWatchdogState>,
}

impl MainTaskWatchdog {
    fn global() -> &'static Arc<Self> {
        static WATCHDOG: OnceLock<Arc<MainTaskWatchdog>> = OnceLock::new();
        WATCHDOG.get_or_init(|| {
            let watchdog = Arc::new(MainTaskWatchdog {
                state: Mutex::new(MainTaskWatchdogState::default()),
            });
            let weak = Arc::downgrade(&watchdog);
            let _ = std::thread::Builder::new()
                .name("gpui-linux-watchdog".to_owned())
                .spawn(move || {
                    let mut reported_id = 0;
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        let Some(watchdog) = weak.upgrade() else {
                            return;
                        };
                        let stalled = {
                            let state = watchdog
                                .state
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            state.active.as_ref().and_then(|active| {
                                let elapsed = active.started.elapsed();
                                (active.id != reported_id && elapsed >= MAIN_TASK_STALL_THRESHOLD)
                                    .then_some((active.id, elapsed, active.location))
                            })
                        };
                        let Some((id, elapsed, location)) = stalled else {
                            continue;
                        };
                        eprintln!(
                            "Zetta diagnostic: main task {id} stalled for {} ms at {location}",
                            elapsed.as_millis(),
                        );
                        reported_id = id;
                    }
                });
            watchdog
        })
    }

    pub(crate) fn begin(location: &'static Location<'static>) -> MainTaskDiagnostic {
        let watchdog = Self::global().clone();
        let mut state = watchdog
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.next_id = state.next_id.wrapping_add(1);
        let id = state.next_id;
        let started = Instant::now();
        state.active = Some(ActiveMainTask {
            id,
            location,
            started,
        });
        drop(state);
        MainTaskDiagnostic { watchdog, id }
    }

    pub(crate) fn active_summary() -> String {
        let state = Self::global()
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state.active.as_ref() {
            Some(active) => format!(
                "task={} elapsed_ms={} location={}",
                active.id,
                active.started.elapsed().as_millis(),
                active.location,
            ),
            None => "idle".to_owned(),
        }
    }
}

pub(crate) struct MainTaskDiagnostic {
    watchdog: Arc<MainTaskWatchdog>,
    id: u64,
}

impl Drop for MainTaskDiagnostic {
    fn drop(&mut self) {
        let mut state = self
            .watchdog
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.active.as_ref().map(|active| active.id) == Some(self.id) {
            state.active = None;
        }
    }
}
