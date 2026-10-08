use super::*;

fn handover(version: u32) -> Handover {
    Handover {
        version,
        generation: 7,
        next_session_id: 8,
        next_pane_id: 9,
        retention: crate::retention::Retention::None,
        sessions: Vec::new(),
        recipient_grants: None,
    }
}

/// The handover crosses a pipe, announced by its handle value. Sent to this
/// same process here, which is the replacement's side of the duplication.
#[test]
fn handover_round_trips_through_a_pipe() {
    let mut announcement = Vec::new();

    send_handover_to(
        unsafe { GetCurrentProcess() },
        &mut announcement,
        &handover(HANDOVER_VERSION),
    )
    .unwrap();
    let received = receive_handover_from(announcement.as_slice()).unwrap();

    assert_eq!(received.generation, 7);
}

#[test]
fn a_handle_that_is_not_a_pipe_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let file = File::create(directory.path().join("not-a-pipe")).unwrap();
    let value = std::os::windows::io::IntoRawHandle::into_raw_handle(file) as usize;

    let error = receive_handover_from(format!("{value}\n").as_bytes()).unwrap_err();
    assert!(error.to_string().contains("not a pipe"), "{error:#}");
}

/// A daemon from before the pipe still upgrades into this image with a file;
/// it is read once and then gone.
#[test]
fn a_file_handover_from_an_older_daemon_is_read_once_and_removed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zmux-handover-old.json");
    fs::write(
        &path,
        serde_json::to_vec(&handover(FILE_HANDOVER_VERSION)).unwrap(),
    )
    .unwrap();

    let received = receive_handover(&HandoverSource::File(path.clone())).unwrap();

    assert_eq!(received.generation, 7);
    assert!(
        !path.exists(),
        "the plaintext handover outlived its reading"
    );
}

#[test]
fn a_transport_only_carries_its_own_version() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("zmux-handover-new.json");
    fs::write(
        &path,
        serde_json::to_vec(&handover(HANDOVER_VERSION)).unwrap(),
    )
    .unwrap();
    assert!(receive_handover(&HandoverSource::File(path)).is_err());

    assert!(accepts_handover_version(HANDOVER_VERSION));
    assert!(accepts_handover_version(FILE_HANDOVER_VERSION));
    assert!(!accepts_handover_version(HANDOVER_VERSION + 1));
}
