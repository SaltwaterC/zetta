use super::*;

use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn wait_on(watched: &[WaitHandle], timeout_ms: u64) -> (Ready, Duration) {
    let started = Instant::now();
    let ready = Waiter::default()
        .wait(watched, std::iter::empty(), timeout_ms)
        .expect("waiting");
    (ready, started.elapsed())
}

/// A wake-up has to be visible to the loop's wait, or a keystroke waits out
/// the session's deadline instead of going out now.
#[test]
fn a_wake_up_ends_the_wait_and_draining_it_lets_the_loop_wait_again() {
    let wake = Wake::new().expect("a wake-up");
    // Raised twice before the loop looks: still one pending wake-up.
    wake.notify();
    wake.notify();

    let (ready, elapsed) = wait_on(&[wake.handle()], 5_000);
    assert!(ready.is_ready(0));
    assert!(
        elapsed < Duration::from_secs(1),
        "a pending wake-up must not wait out the timeout"
    );

    wake.drain();
    let (ready, elapsed) = wait_on(&[wake.handle()], 50);
    assert!(!ready.is_ready(0));
    assert!(
        elapsed >= Duration::from_millis(40),
        "a drained wake-up must let the loop wait again"
    );
}

#[test]
fn a_wake_up_from_another_thread_ends_a_long_wait() {
    let wake = std::sync::Arc::new(Wake::new().expect("a wake-up"));
    let raiser = {
        let wake = std::sync::Arc::clone(&wake);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            wake.notify();
        })
    };
    let (ready, elapsed) = wait_on(&[wake.handle()], 10_000);
    raiser.join().unwrap();
    assert!(ready.is_ready(0));
    assert!(elapsed < Duration::from_secs(5), "waited {elapsed:?}");
}

#[test]
fn only_the_handles_that_are_ready_are_reported() {
    let quiet = Wake::new().expect("a wake-up");
    let raised = Wake::new().expect("a wake-up");
    raised.notify();
    let (ready, _) = wait_on(&[quiet.handle(), raised.handle()], 1_000);
    assert!(!ready.is_ready(0));
    assert!(ready.is_ready(1));
}

/// The point of the module on Windows: a datagram for the session ends the
/// wait the moment it arrives, rather than when the timeout does.
#[test]
fn a_datagram_on_a_session_socket_ends_the_wait() {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
    let wake = Wake::new().expect("a wake-up");
    #[cfg(unix)]
    let handle = {
        use std::os::fd::AsRawFd as _;
        socket.as_raw_fd()
    };
    #[cfg(windows)]
    let handle = {
        use std::os::windows::io::AsRawSocket as _;
        socket.as_raw_socket()
    };

    let mut waiter = Waiter::default();
    let started = Instant::now();
    let quiet = waiter
        .wait(&[wake.handle()], std::iter::once(handle), 50)
        .unwrap();
    assert!(!quiet.is_ready(0));
    assert!(started.elapsed() >= Duration::from_millis(40));

    sender
        .send_to(b"frame", socket.local_addr().unwrap())
        .unwrap();
    let started = Instant::now();
    let ready = waiter
        .wait(&[wake.handle()], std::iter::once(handle), 10_000)
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the datagram did not end the wait"
    );
    // Socket readiness is not reported by position; the loop pumps anyway.
    assert!(!ready.is_ready(0));
}
