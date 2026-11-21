use super::*;

#[test]
fn resumed_identifiers_never_collide_with_what_was_adopted() {
    // The failure this guards against: a handover claiming the next session is
    // 1 while session 1 is right there in it. The next spawn then reuses the
    // identifier, and the multiplexer holds two sessions no client can tell
    // apart — they list identically, and attaching one is a coin toss.
    let (session, pane) = next_ids_after(&[1], &[1], 1, 1);
    assert_eq!(session, 2);
    assert_eq!(pane, 2);

    // A handover that is ahead of what it carries is still respected: it knows
    // about identifiers that were issued and then released.
    let (session, pane) = next_ids_after(&[1], &[1], 9, 7);
    assert_eq!(session, 9);
    assert_eq!(pane, 7);

    // Several sessions, and panes numbered independently of them.
    let (session, pane) = next_ids_after(&[3, 1, 2], &[5, 9, 2], 0, 0);
    assert_eq!(session, 4);
    assert_eq!(pane, 10);

    // Nothing adopted: start where the handover says.
    assert_eq!(next_ids_after(&[], &[], 4, 6), (4, 6));
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[test]
fn an_owner_is_adopted_as_the_previous_image_recorded_it() {
    let this = ProcessIdentity::of(std::process::id());
    let started = this.started.expect("this platform reports start times");

    // Taken as written, including a start time that no longer matches: the
    // owner it named has gone, whoever holds its PID now.
    let gone = adopt_owner(Some(this.pid), Some(started + 1)).unwrap();
    assert!(!gone.is(this.pid));
    assert_eq!(adopt_owner(None, None), None);

    // An unavailable start time stays unavailable; adoption must not guess one.
    let previous = adopt_owner(Some(this.pid), None).unwrap();
    assert_eq!(
        previous,
        ProcessIdentity {
            pid: this.pid,
            started: None
        }
    );
}

/// A Windows daemon goes on running beside the replacement it handed over to,
/// and once flushed the scrollback it still held on its way out — rewriting
/// the persistence manifest under the replacement with its own older copy.
/// The flush now happens before the replacement starts, and a daemon that has
/// handed over leaves the directory, the socket and the endpoint alone.
#[cfg(all(not(target_os = "macos"), feature = "session-persistence"))]
#[test]
fn a_daemon_that_handed_over_leaves_what_its_replacement_owns() {
    use crate::server::secret_check::tests::fixtures::test_daemon;

    let segments = |directory: &std::path::Path| {
        std::fs::read_dir(directory)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("session-7-pane-1-segment-")
            })
            .count()
    };
    for handed_over in [true, false] {
        let (daemon, directory) = test_daemon();
        let store_directory = directory.path().join("store");
        let identity = age::x25519::Identity::generate();
        let mut store = crate::persistence::PersistenceStore::open(
            &store_directory,
            &[identity.to_public().to_string()],
        )
        .unwrap()
        .unwrap();
        store.seal(7, &crate::persistence::Seal::Store).unwrap();
        store.append_scrollback(7, 1, b"buffered").unwrap();
        *daemon.persistence.lock() = Some(store);
        let socket = directory.path().join("socket");
        let endpoint = directory.path().join("endpoint");
        std::fs::write(&socket, "").unwrap();
        std::fs::write(&endpoint, "").unwrap();

        daemon.handed_over.store(handed_over, Ordering::SeqCst);
        crate::server::startup::shut_down(&daemon, &socket, &endpoint);

        assert_eq!(
            segments(&store_directory.join("persistence")),
            usize::from(!handed_over),
            "handed over: {handed_over}"
        );
        assert_eq!(socket.exists(), handed_over);
        assert_eq!(endpoint.exists(), handed_over);
    }
}
