//! Ordered, off-thread publication of the application's session catalogs.
//!
//! The worker owns the synchronous publishers, including their file-removing
//! destructors. The first job is reserved immediately; only snapshots waiting
//! behind it may be replaced. Completion fences seal that pending work so an
//! explicit operation is acknowledged after its publication has been attempted.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, OnceLock},
};

use super::{BackgroundSessionSummary, SessionCatalogPublisher};

enum Publication {
    Register(SessionCatalogPublisher),
    Snapshot(u64, Vec<BackgroundSessionSummary>),
    Complete(Box<dyn FnOnce() + Send>),
    Remove(u64),
}

#[derive(Default)]
struct Queue {
    busy: bool,
    writing: bool,
    first: Option<Publication>,
    pending: VecDeque<Publication>,
}

impl Queue {
    fn push(&mut self, publication: Publication) {
        if !self.busy {
            self.busy = true;
            self.writing = matches!(publication, Publication::Snapshot(..));
            self.first = Some(publication);
            return;
        }
        if self.writing
            && let Publication::Snapshot(runner_id, _) = &publication
        {
            // Never replace a snapshot across an operation's completion or a
            // runner's removal. The reserved first job is never coalesced.
            for pending in self.pending.iter_mut().rev() {
                match pending {
                    Publication::Snapshot(id, _) if id == runner_id => {
                        *pending = publication;
                        return;
                    }
                    Publication::Complete(_) | Publication::Remove(_) => break,
                    _ => {}
                }
            }
        }
        self.pending.push_back(publication);
    }

    fn finished(&mut self) -> Option<Publication> {
        let next = self.pending.pop_front();
        self.busy = next.is_some();
        self.writing = matches!(next, Some(Publication::Snapshot(..)));
        next
    }
}

#[derive(Default)]
struct Worker {
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl Worker {
    fn submit(&self, publication: Publication) {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(publication);
        self.ready.notify_one();
    }

    fn run(&self, mut apply: impl FnMut(Publication)) {
        loop {
            let publication = {
                let mut queue = self
                    .queue
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                loop {
                    if let Some(first) = queue.first.take() {
                        break first;
                    }
                    queue = self
                        .ready
                        .wait(queue)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            };
            self.drain(publication, &mut apply);
        }
    }

    fn drain(&self, mut publication: Publication, apply: &mut impl FnMut(Publication)) {
        loop {
            // No queue lock is held during serialization, storage I/O, or
            // completion. Producers never wait for a slow filesystem.
            apply(publication);
            let next = self
                .queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finished();
            let Some(next) = next else { break };
            publication = next;
        }
    }
}

fn worker() -> &'static Arc<Worker> {
    static WORKER: OnceLock<Arc<Worker>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let worker = Arc::new(Worker::default());
        let background = worker.clone();
        std::thread::Builder::new()
            .name("session-catalog".into())
            .spawn(move || {
                let mut publishers = HashMap::new();
                background.run(|publication| match publication {
                    Publication::Register(publisher) => {
                        publishers.insert(publisher.runner_id(), publisher);
                    }
                    Publication::Snapshot(runner_id, sessions) => {
                        if let Some(publisher) = publishers.get_mut(&runner_id)
                            && let Err(error) = publisher.publish_sessions(sessions)
                        {
                            eprintln!("Could not publish background session catalog: {error:#}");
                        }
                    }
                    Publication::Complete(completion) => completion(),
                    Publication::Remove(runner_id) => {
                        publishers.remove(&runner_id);
                    }
                });
            })
            .expect("starting session catalog worker");
        worker
    })
}

pub(super) struct BackgroundCatalogPublisher {
    runner_id: u64,
}

impl BackgroundCatalogPublisher {
    pub(super) fn new(publisher: SessionCatalogPublisher) -> Self {
        let runner_id = publisher.runner_id();
        worker().submit(Publication::Register(publisher));
        Self { runner_id }
    }

    pub(super) fn runner_id(&self) -> u64 {
        self.runner_id
    }

    pub(super) fn publish(&self, sessions: Vec<BackgroundSessionSummary>) {
        worker().submit(Publication::Snapshot(self.runner_id, sessions));
    }
}

impl Drop for BackgroundCatalogPublisher {
    fn drop(&mut self) {
        // Cleanup follows every queued write, so a late write cannot recreate
        // a runner's catalog after it has gone away. Never join on the GUI.
        worker().submit(Publication::Remove(self.runner_id));
    }
}

pub(crate) fn after_pending_publications(completion: impl FnOnce() + Send + 'static) {
    worker().submit(Publication::Complete(Box::new(completion)));
}

#[cfg(test)]
#[path = "../tests/background_sessions/publication.rs"]
mod tests;
