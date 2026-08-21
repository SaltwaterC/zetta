use super::*;

#[test]
fn handover_round_trips_through_private_state() {
    let directory = tempfile::tempdir().unwrap();
    let handover = Handover {
        version: HANDOVER_VERSION,
        generation: 7,
        next_session_id: 8,
        next_pane_id: 9,
        retention: crate::retention::Retention::None,
        sessions: Vec::new(),
    };
    let (path, ready) = write_handover(directory.path(), &handover).unwrap();
    assert_eq!(read_handover(&path).unwrap().generation, 7);
    remove_handover(&path, &ready);
    assert!(!path.exists());
}
