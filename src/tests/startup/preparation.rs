use super::*;
use std::{sync::mpsc, time::Duration};

#[test]
fn preparation_overlaps_the_platform_without_moving_its_thread() {
    let caller = thread::current().id();
    let (prepared_tx, prepared_rx) = mpsc::channel();
    let (platform_tx, platform_rx) = mpsc::channel();
    let (platform, launch) = prepare_in_parallel(
        move || {
            assert_ne!(thread::current().id(), caller);
            prepared_tx.send(()).unwrap();
            platform_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            Ok("launch")
        },
        || {
            assert_eq!(thread::current().id(), caller);
            prepared_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            platform_tx.send(()).unwrap();
            "platform"
        },
    )
    .unwrap();
    assert_eq!((platform, launch), ("platform", "launch"));
}

#[test]
fn preparation_errors_reach_the_caller() {
    let result = prepare_in_parallel(|| Err::<(), _>(anyhow!("invalid launch")), || ());
    assert_eq!(result.unwrap_err().to_string(), "invalid launch");
}

#[test]
fn a_panicked_preparation_returns_an_error() {
    let result = prepare_in_parallel(|| -> Result<()> { panic!("failed preparation") }, || ());
    assert_eq!(
        result.unwrap_err().to_string(),
        "launch preparation panicked"
    );
}
