use super::*;
use crate::background_sessions::{BackgroundPaneLayout, BackgroundSessionCatalog};
use std::{fs, sync::mpsc, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(10);

fn sessions(title: &str, count: u64) -> Vec<BackgroundSessionSummary> {
    (0..count)
        .map(|id| BackgroundSessionSummary {
            id,
            title: title.into(),
            authentication_required: false,
            active_pane: id,
            layout: BackgroundPaneLayout::Pane { pane_id: id },
            panes: Vec::new(),
            held: false,
            scoped_to: None,
            key_envelope: None,
        })
        .collect()
}

fn take_first(worker: &Worker) -> Publication {
    worker
        .queue
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .first
        .take()
        .unwrap()
}

#[test]
fn slow_storage_keeps_first_and_latest_titles_and_seals_acknowledged_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let mut publisher = SessionCatalogPublisher::new(directory.path());
    let runner_id = publisher.runner_id();
    let worker = Arc::new(Worker::default());
    worker.submit(Publication::Snapshot(runner_id, sessions("first", 256)));
    let first = take_first(&worker);
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let background = worker.clone();
    let writes = event_tx.clone();
    let thread = std::thread::spawn(move || {
        let mut first_write = true;
        background.drain(first, &mut |job| match job {
            Publication::Snapshot(_, sessions) => {
                if first_write {
                    started_tx.send(()).unwrap();
                    release_rx.recv_timeout(TIMEOUT).unwrap();
                    first_write = false;
                }
                let title = sessions[0].title.clone();
                publisher.publish_sessions(sessions).unwrap();
                writes.send(title).unwrap();
            }
            Publication::Complete(completion) => completion(),
            _ => panic!("unexpected job"),
        });
        publisher
    });
    started_rx.recv_timeout(TIMEOUT).unwrap();
    // Simulate rapid title changes across many detached sessions while the
    // first filesystem operation is blocked. Submission must stay responsive.
    for generation in 1..=100 {
        worker.submit(Publication::Snapshot(
            runner_id,
            sessions(&generation.to_string(), 256),
        ));
    }
    worker.submit(Publication::Complete(Box::new(move || {
        event_tx.send("ack".into()).unwrap();
    })));
    for generation in 101..=200 {
        worker.submit(Publication::Snapshot(
            runner_id,
            sessions(&generation.to_string(), 256),
        ));
    }
    assert!(event_rx.try_recv().is_err(), "no early acknowledgement");
    assert_eq!(
        worker
            .queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .len(),
        3,
        "one snapshot per side of the acknowledgement fence"
    );
    release_tx.send(()).unwrap();
    for expected in ["first", "100", "ack", "200"] {
        assert_eq!(event_rx.recv_timeout(TIMEOUT).unwrap(), expected);
    }
    let _publisher = thread.join().unwrap();
    let path = directory
        .path()
        .join(format!("zetta-{}-{runner_id}.json", std::process::id()));
    let catalog: BackgroundSessionCatalog =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(catalog.sessions.len(), 256);
    assert!(
        catalog
            .sessions
            .iter()
            .all(|session| session.title == "200")
    );
}

#[test]
fn first_publication_is_reserved_before_the_worker_runs() {
    let worker = Worker::default();
    worker.submit(Publication::Snapshot(1, sessions("first", 1)));
    worker.submit(Publication::Snapshot(1, sessions("second", 1)));
    worker.submit(Publication::Snapshot(1, sessions("latest", 1)));
    let mut titles = Vec::new();
    worker.drain(take_first(&worker), &mut |job| {
        let Publication::Snapshot(_, sessions) = job else {
            panic!("unexpected job")
        };
        titles.push(sessions[0].title.clone());
    });
    assert_eq!(titles, ["first", "latest"]);

    // A new idle period starts another immediate first publication.
    worker.submit(Publication::Snapshot(1, sessions("next", 1)));
    assert!(matches!(take_first(&worker), Publication::Snapshot(1, _)));
}

#[test]
fn pending_runners_do_not_overwrite_each_other() {
    let worker = Worker::default();
    worker.submit(Publication::Snapshot(1, sessions("first", 1)));
    worker.submit(Publication::Snapshot(2, sessions("other", 1)));
    worker.submit(Publication::Snapshot(1, sessions("old", 1)));
    worker.submit(Publication::Snapshot(1, sessions("latest", 1)));
    let mut written = Vec::new();
    worker.drain(take_first(&worker), &mut |job| {
        let Publication::Snapshot(id, sessions) = job else {
            panic!("unexpected job")
        };
        written.push((id, sessions[0].title.clone()));
    });
    assert_eq!(
        written,
        [
            (1, "first".into()),
            (2, "other".into()),
            (1, "latest".into())
        ]
    );
}

fn settled() {
    let (sender, receiver) = mpsc::channel();
    after_pending_publications(move || sender.send(()).unwrap());
    receiver.recv_timeout(TIMEOUT).unwrap();
}

#[test]
fn catalog_deletion_recreation_and_drop_are_ordered() {
    let directory = tempfile::tempdir().unwrap();
    let publisher = BackgroundCatalogPublisher::new(SessionCatalogPublisher::new(directory.path()));
    let path = directory.path().join(format!(
        "zetta-{}-{}.json",
        std::process::id(),
        publisher.runner_id()
    ));
    publisher.publish(sessions("original", 1));
    settled();
    assert!(path.exists());
    publisher.publish(Vec::new());
    settled();
    assert!(!path.exists());
    publisher.publish(sessions("original", 1));
    settled();
    assert!(path.exists());

    for _ in 0..100 {
        publisher.publish(sessions("queued before drop", 1));
    }
    drop(publisher);
    settled();
    assert!(!path.exists(), "no late write recreates a dropped runner");
}

#[test]
fn failed_publication_releases_acknowledgement_and_allows_retry() {
    let directory = tempfile::tempdir().unwrap();
    let storage = directory.path().join("storage");
    fs::write(&storage, "not a directory").unwrap();
    let publisher = BackgroundCatalogPublisher::new(SessionCatalogPublisher::new(&storage));
    publisher.publish(sessions("first", 1));
    settled();
    fs::remove_file(&storage).unwrap();
    publisher.publish(sessions("retry", 1));
    settled();
    let path = storage.join(format!(
        "zetta-{}-{}.json",
        std::process::id(),
        publisher.runner_id()
    ));
    let catalog: BackgroundSessionCatalog =
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(catalog.sessions[0].title, "retry");
    drop(publisher);
    settled();
}
