//! The daemon's encrypted store, and the ordered worker that writes pane output
//! into it.
//!
//! The drain reads detached and shared panes while holding `Daemon::sessions`,
//! and every byte it reads is appended to the store. An append is a buffer
//! extension, but one in every 8 MiB (or five minutes) of a pane's output
//! rotates its segment: the store encrypts it, publishes it and rewrites
//! the manifest before returning. Done inline, that held the global registry
//! lock for the whole rotation, so every other shared pane's output and input,
//! and every session operation, waited on the disk.
//!
//! The drain therefore only queues output here, and one worker applies the
//! queue to the store in the order it was read. Rotation keeps its thresholds
//! and its cadence; only the thread it runs on, and the locks held around it,
//! change.
//!
//! Everything else reaches the store through [`PersistenceQueue::lock`], which
//! first waits until the output queued before the call has been applied. That
//! keeps one order across the store: a save, forget or flush sees every byte the
//! drain read before it, so a forgotten session cannot have a segment published
//! after its files were removed, and the flush at shutdown or before an upgrade
//! covers all output read up to that point. Those calls stay synchronous and
//! return the store's own `Result`, which is their acknowledgement — nothing
//! whose completion promises durability is made asynchronous. Output was never
//! durable before rotation, so queueing it weakens nothing: an append that
//! fails is logged and its bytes dropped, exactly as when it ran inline.
//!
//! The queue is bounded by bytes rather than allowed to grow with a slow disk.
//! Past [`QUEUE_LIMIT_BYTES`] the drain stops reading panes rather than block
//! under the registry lock, so their output stays in the terminal and the
//! program waits, as relay backpressure already does for a slow viewer. The
//! bound is global rather than per session: one worker applies the queue in
//! order, so no session's output could overtake another's anyway. The worker
//! wakes the drain once there is room.

use std::{
    panic::AssertUnwindSafe,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use crate::persistence::PersistenceStore;

/// How much read output may wait for the worker before the drain stops reading.
/// Two segments' worth: enough to absorb a rotation's encryption and publication
/// without holding a pane, small next to the per-pane segment buffers the store
/// already keeps.
pub(super) const QUEUE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// The store, and the output still on its way into it.
pub(super) struct PersistenceQueue {
    store: Mutex<Option<PersistenceStore>>,
    /// Whether new output is persisted at all. Changed only while the store
    /// lock is held, and read by the worker under it, so the worker never
    /// applies output to a store replaced by a mode that does not persist it.
    enabled: AtomicBool,
    state: Mutex<QueueState>,
    /// Signalled when output is queued.
    queued: Condvar,
    /// Signalled when queued output has been applied.
    applied: Condvar,
    /// Bytes queued and not yet applied. Kept outside `state` so the drain can
    /// check for backpressure on every pane without taking another lock.
    queued_bytes: AtomicUsize,
}

#[derive(Default)]
struct QueueState {
    appends: Vec<Append>,
    /// Appends ever queued, and ever applied. A caller of
    /// [`PersistenceQueue::lock`] waits for `applied` to reach the `queued` it
    /// observed, which is "everything queued before me".
    queued: u64,
    applied: u64,
    worker_started: bool,
}

struct Append {
    session_id: u64,
    pane_id: u64,
    bytes: Vec<u8>,
}

impl PersistenceQueue {
    pub(super) fn new(store: Option<PersistenceStore>, enabled: bool) -> Self {
        Self {
            store: Mutex::new(store),
            enabled: AtomicBool::new(enabled),
            state: Mutex::new(QueueState::default()),
            queued: Condvar::new(),
            applied: Condvar::new(),
            queued_bytes: AtomicUsize::new(0),
        }
    }

    /// Starts the worker. `on_room` runs whenever the queue was over its bound
    /// before a batch was applied, which is when a drain holding panes off may
    /// read them again.
    ///
    /// Until this is called output is applied by the caller of
    /// [`Self::append`], which keeps a daemon built without its workers — as
    /// tests build one — from waiting on a worker that does not exist.
    pub(super) fn start(self: &Arc<Self>, on_room: impl Fn() + Send + Sync + 'static) {
        let mut state = self.lock_state();
        if state.worker_started {
            return;
        }
        state.worker_started = true;
        drop(state);
        let queue = Arc::clone(self);
        let on_room = Arc::new(on_room);
        super::spawn_worker("zmux persistence", move || {
            let queue = Arc::clone(&queue);
            let on_room = Arc::clone(&on_room);
            Box::new(move || queue.run(&*on_room))
        });
    }

    /// Whether output is persisted. Callers that persist a whole session check
    /// this before building one.
    pub(super) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// Sets [`Self::enabled`]. Takes the store guard to say that it must be
    /// held: the worker reads the flag under it, so the store and whether to
    /// write to it change together.
    pub(super) fn set_enabled(
        &self,
        _store: &mut MutexGuard<'_, Option<PersistenceStore>>,
        enabled: bool,
    ) {
        self.enabled.store(enabled, Ordering::Release);
    }

    /// Whether the drain should stop reading panes until the worker catches up.
    pub(super) fn backlogged(&self) -> bool {
        self.queued_bytes.load(Ordering::Acquire) >= QUEUE_LIMIT_BYTES
    }

    /// Queues one read of a pane's output. Called from the drain under the
    /// registry lock, so this copies the bytes and returns: no store lock, no
    /// encryption and no I/O.
    pub(super) fn append(&self, session_id: u64, pane_id: u64, bytes: &[u8]) {
        if bytes.is_empty() || !self.enabled() {
            return;
        }
        let append = Append {
            session_id,
            pane_id,
            bytes: bytes.to_vec(),
        };
        let mut state = self.lock_state();
        if !state.worker_started {
            drop(state);
            self.apply(std::slice::from_ref(&append));
            return;
        }
        self.queued_bytes
            .fetch_add(append.bytes.len(), Ordering::AcqRel);
        state.appends.push(append);
        state.queued += 1;
        drop(state);
        self.queued.notify_one();
    }

    /// The store, once every append queued before this call has been applied.
    ///
    /// Waits for the worker rather than for the disk in general: output queued
    /// after the call began may land before or after the caller's operation,
    /// as it could when both raced for the store's lock directly. A caller
    /// removing a session removes it from the registry first, so the drain has
    /// queued its last output by then and the wait covers all of it.
    pub(super) fn lock(&self) -> MutexGuard<'_, Option<PersistenceStore>> {
        let mut state = self.lock_state();
        let target = state.queued;
        while state.applied < target {
            state = self
                .applied
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        drop(state);
        self.lock_store()
    }

    fn run(&self, on_room: &(dyn Fn() + Send + Sync)) {
        loop {
            let appends = {
                let mut state = self.lock_state();
                while state.appends.is_empty() {
                    state = self
                        .queued
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                std::mem::take(&mut state.appends)
            };
            // A panic here would otherwise end the worker with the batch
            // unaccounted for, and every later `lock` would wait for it for
            // ever. The store lock it poisons is reported to the next caller,
            // as a panic inside the store always was.
            if std::panic::catch_unwind(AssertUnwindSafe(|| self.apply(&appends))).is_err() {
                log::error!("applying queued pane output to the encrypted store panicked");
            }
            let bytes = appends.iter().map(|append| append.bytes.len()).sum();
            let before = self.queued_bytes.fetch_sub(bytes, Ordering::AcqRel);
            // Taken from the value this batch replaced, not re-read: a drain
            // that saw the queue over its bound saw it before this subtraction,
            // so this batch, or a later one it raced, is the one that wakes it.
            if before >= QUEUE_LIMIT_BYTES {
                on_room();
            }
            self.lock_state().applied += appends.len() as u64;
            self.applied.notify_all();
        }
    }

    fn apply(&self, appends: &[Append]) {
        let mut store = self.lock_store();
        if !self.enabled() {
            return;
        }
        let Some(store) = store.as_mut() else {
            return;
        };
        for append in appends {
            if let Err(error) =
                store.append_scrollback(append.session_id, append.pane_id, &append.bytes)
            {
                log::warn!(
                    "could not persist pane {} output: {error:#}",
                    append.pane_id
                );
            }
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_store(&self) -> MutexGuard<'_, Option<PersistenceStore>> {
        // `unwrap`, not recovery: a panic inside the store can leave its
        // manifest disagreeing with the files it describes, and writing on
        // top of that would make the disagreement durable.
        self.store.lock().unwrap()
    }
}

#[cfg(test)]
#[path = "../tests/server/persistence_queue.rs"]
mod tests;
