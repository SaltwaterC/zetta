use super::*;

use std::{fs, path::Path, sync::mpsc, thread, time::Duration};

use age::secrecy::ExposeSecret as _;

use crate::persistence::IdentitySet;

const PATIENCE: Duration = Duration::from_secs(10);

fn store(directory: &Path) -> (PersistenceStore, IdentitySet) {
    let identity = age::x25519::Identity::generate();
    let path = directory.join("identity.txt");
    fs::write(&path, format!("{}\n", identity.to_string().expose_secret())).unwrap();
    let store = PersistenceStore::open(directory, &[identity.to_public().to_string()])
        .unwrap()
        .unwrap();
    (store, IdentitySet::from_paths(&[path]).unwrap())
}

/// A started queue, and a receiver that hears each time it reports room.
fn started(store: Option<PersistenceStore>) -> (Arc<PersistenceQueue>, mpsc::Receiver<()>) {
    let queue = Arc::new(PersistenceQueue::new(store, true));
    let (room, rooms) = mpsc::channel();
    queue.start(move || {
        let _ = room.send(());
    });
    (queue, rooms)
}

fn flushed_scrollback(
    queue: &PersistenceQueue,
    session_id: u64,
    identities: &IdentitySet,
) -> Vec<u8> {
    let mut store = queue.lock();
    let store = store.as_mut().unwrap();
    store.flush_segments().unwrap();
    store.read_scrollback(session_id, identities).unwrap()
}

fn pane_segments(directory: &Path, session_id: u64) -> Vec<String> {
    let prefix = format!("session-{session_id}-pane-");
    fs::read_dir(directory.join("persistence"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with(&prefix))
        .collect()
}

#[test]
fn the_drain_queues_output_while_the_store_is_busy() {
    let directory = tempfile::tempdir().unwrap();
    let (store, identities) = store(directory.path());
    let (queue, _rooms) = started(Some(store));

    // Holding the store stands in for a rotation encrypting and publishing a
    // segment. Inline, the drain waited for it under the registry lock.
    let busy = queue.lock();
    let (done, finished) = mpsc::channel();
    let drain = {
        let queue = Arc::clone(&queue);
        thread::spawn(move || {
            queue.append(7, 1, b"read while the disk was busy");
            done.send(()).unwrap();
        })
    };
    finished
        .recv_timeout(PATIENCE)
        .expect("appending output must not wait for the store");
    drain.join().unwrap();
    drop(busy);

    assert_eq!(
        flushed_scrollback(&queue, 7, &identities),
        b"read while the disk was busy"
    );
}

#[test]
fn the_store_is_handed_out_only_after_earlier_output_is_applied() {
    let directory = tempfile::tempdir().unwrap();
    let (store, identities) = store(directory.path());
    let (queue, _rooms) = started(Some(store));

    let busy = queue.lock();
    for chunk in [&b"one "[..], b"two ", b"three"] {
        queue.append(7, 1, chunk);
    }
    // The worker is now waiting for the store. Without the wait in `lock`, the
    // flush below would usually win that race and publish nothing.
    drop(busy);

    assert_eq!(flushed_scrollback(&queue, 7, &identities), b"one two three");
}

#[test]
fn forgetting_a_session_follows_its_queued_output() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _identities) = store(directory.path());
    let (queue, _rooms) = started(Some(store));

    let busy = queue.lock();
    queue.append(7, 1, b"the session's last output");
    drop(busy);
    queue.lock().as_mut().unwrap().forget(7).unwrap();
    queue.lock().as_mut().unwrap().flush_segments().unwrap();

    // Applied after the forget, the output would have been buffered again and
    // published as a segment no record owns, to be replayed into whichever
    // session next reuses the id.
    assert!(pane_segments(directory.path(), 7).is_empty());
}

#[test]
fn a_full_queue_holds_the_drain_off_until_the_worker_makes_room() {
    let (queue, rooms) = started(None);
    let busy = queue.lock();
    let chunk = vec![b'x'; 1024 * 1024];
    while !queue.backlogged() {
        queue.append(7, 1, &chunk);
    }
    assert!(
        rooms.try_recv().is_err(),
        "nothing has been applied, so there is no room to report"
    );

    drop(busy);
    rooms
        .recv_timeout(PATIENCE)
        .expect("the worker must wake the drain once it has made room");
    assert!(!queue.backlogged());
}

#[test]
fn a_queue_under_its_bound_never_holds_the_drain_off() {
    let (queue, rooms) = started(None);
    let busy = queue.lock();
    queue.append(7, 1, &vec![b'x'; QUEUE_LIMIT_BYTES - 1]);
    assert!(!queue.backlogged());
    drop(busy);

    // Applied without crossing the bound, so the drain was never told to stop
    // and must not be woken on that account. A batch reports room before it
    // counts as applied, so once `lock` returns any report has been made.
    drop(queue.lock());
    assert!(rooms.try_recv().is_err());
}

#[test]
fn output_queued_before_persistence_was_disabled_does_not_reach_the_next_store() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _identities) = store(directory.path());
    let (queue, _rooms) = started(Some(store));

    let mut current = queue.lock();
    queue.append(7, 1, b"read in disk mode");
    // As `configure_daemon` does: the outgoing store is flushed, the
    // read-only one that replaces it is installed with persistence disabled.
    current.as_mut().unwrap().flush_segments().unwrap();
    let replacement = PersistenceStore::open_with_recovery(directory.path(), None)
        .unwrap()
        .unwrap();
    *current = Some(replacement);
    queue.set_enabled(&mut current, false);
    drop(current);

    queue.lock().as_mut().unwrap().flush_segments().unwrap();
    assert!(pane_segments(directory.path(), 7).is_empty());
}

#[test]
fn a_disabled_queue_does_not_take_output() {
    let directory = tempfile::tempdir().unwrap();
    let (store, _identities) = store(directory.path());
    let queue = Arc::new(PersistenceQueue::new(Some(store), false));
    queue.start(|| {});

    queue.append(7, 1, b"memory mode");
    assert_eq!(queue.queued_bytes.load(Ordering::Acquire), 0);
    queue.lock().as_mut().unwrap().flush_segments().unwrap();
    assert!(pane_segments(directory.path(), 7).is_empty());
}

#[test]
fn without_a_worker_output_is_applied_by_the_caller() {
    let directory = tempfile::tempdir().unwrap();
    let (store, identities) = store(directory.path());
    let queue = PersistenceQueue::new(Some(store), true);

    queue.append(7, 1, b"no worker");
    assert_eq!(flushed_scrollback(&queue, 7, &identities), b"no worker");
}
