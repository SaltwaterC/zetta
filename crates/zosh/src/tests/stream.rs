use super::*;

use std::time::{Duration, Instant};

/// The pipe is what an embedder reads a pane through, so its two ends have to
/// hold up the contract a pipe does: a reader blocks until there is something
/// to read, and sees end of file once the loop has gone.
#[test]
fn the_output_pipe_blocks_until_bytes_arrive_and_ends_when_the_loop_does() {
    let pipe = Arc::new(OutputPipe::default());
    let writer = Arc::clone(&pipe);
    let reader = thread::spawn(move || {
        let mut buffer = [0_u8; 4];
        let started = Instant::now();
        let read = pipe.read(&mut buffer).expect("a live pipe reads");
        let waited = started.elapsed();
        let mut bytes = buffer[..read].to_vec();
        loop {
            let read = pipe.read(&mut buffer).expect("a live pipe reads");
            if read == 0 {
                return (bytes, waited);
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
    });

    // Nothing has been written yet, so the reader is parked rather than
    // spinning on an empty buffer or reporting a premature end of file.
    thread::sleep(Duration::from_millis(50));
    writer.write(b"first").expect("a read pipe accepts a frame");
    writer
        .write(b"second")
        .expect("a read pipe accepts a frame");
    writer.close();

    let (bytes, waited) = reader.join().expect("the reader thread finishes");
    assert_eq!(bytes, b"firstsecond", "every painted byte reaches the pane");
    assert!(
        waited >= Duration::from_millis(40),
        "an empty pipe must park its reader rather than report end of file"
    );
}

/// Frames arrive and are drained continuously, so the buffer behind the pipe
/// wraps; a read that takes the two halves of a wrapped ring in the wrong
/// order would corrupt the pane rather than fail.
#[test]
fn bytes_come_back_in_order_across_a_wrapped_buffer() {
    let pipe = OutputPipe::default();
    let mut written = Vec::new();
    let mut read_back = Vec::new();
    let mut buffer = [0_u8; 7];
    for round in 0..200_u16 {
        let frame = (0..13_u8)
            .map(|index| index.wrapping_add(round as u8))
            .collect::<Vec<_>>();
        pipe.write(&frame).expect("a live pipe accepts a frame");
        written.extend_from_slice(&frame);
        // Less than was written, so the unread remainder keeps moving the
        // ring's head and tail past each other.
        let read = pipe.read(&mut buffer).expect("a live pipe reads");
        read_back.extend_from_slice(&buffer[..read]);
    }
    pipe.close();
    loop {
        let read = pipe.read(&mut buffer).expect("a live pipe reads");
        if read == 0 {
            break;
        }
        read_back.extend_from_slice(&buffer[..read]);
    }

    assert_eq!(read_back, written);
}

/// A consumer that goes away must not leave the session loop painting into a
/// buffer nothing will ever drain.
#[test]
fn an_abandoned_pipe_reports_a_broken_pipe_to_the_session_loop() {
    let pipe = OutputPipe::default();
    pipe.write(b"painted").expect("a live pipe accepts a frame");
    pipe.abandon();

    let error = pipe.write(b"more").expect_err("an abandoned pipe refuses");
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
}

/// The bound exists to stall a session whose consumer has stopped reading,
/// and has to let go the moment that consumer disappears — otherwise the loop
/// is parked for the lifetime of the process.
#[test]
fn a_full_pipe_waits_for_the_reader_and_gives_up_when_it_is_abandoned() {
    let pipe = Arc::new(OutputPipe::default());
    pipe.write(&vec![b'x'; OUTPUT_CAPACITY])
        .expect("a frame is always accepted whole");

    let writer = Arc::clone(&pipe);
    let blocked = thread::spawn(move || writer.write(b"one more frame"));
    thread::sleep(Duration::from_millis(20));
    assert!(
        !blocked.is_finished(),
        "a full pipe must wait for the consumer"
    );

    pipe.abandon();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !blocked.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let result = blocked.join().expect("the blocked writer thread finishes");
    assert_eq!(
        result.expect_err("an abandoned pipe refuses").kind(),
        io::ErrorKind::BrokenPipe
    );
}

/// Input carries the answer to an OSC 10/11 query the remote program made,
/// mixed in with ordinary keystrokes; the answer belongs to the protocol's
/// terminal-response channel and must not reach the remote program as typing.
#[test]
fn a_colour_query_response_is_taken_out_of_the_input_it_arrives_in() {
    let mut proxy = TerminalQueryProxy::default();
    proxy.register_query(b"\x1b]10;?\x07");

    assert_eq!(
        proxy.filter(b"a\x1b]10;rgb:aaaa/bbbb/cccc\x07b"),
        vec![
            ProxiedInput::User(b"a".to_vec()),
            ProxiedInput::TerminalResponse(b"\x1b]10;rgb:aaaa/bbbb/cccc\x07".to_vec()),
            ProxiedInput::User(b"b".to_vec()),
        ]
    );
}

/// A wake-up has to be visible to the loop's wait, or a keystroke waits out
/// the idle timeout instead of going out now.
#[test]
fn a_wake_up_is_seen_by_the_wait_and_cleared_by_draining_it() {
    let wake = Wake::new().expect("a wake-up pipe");
    wake.notify();

    let started = Instant::now();
    wait_for_wake(&wake, 5_000);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a pending wake-up must not wait out the timeout"
    );

    wake.drain();
    let started = Instant::now();
    wait_for_wake(&wake, 50);
    assert!(
        started.elapsed() >= Duration::from_millis(40),
        "a drained wake-up must let the loop wait again"
    );
}

/// The session-shaped wait, without a session: on Unix the wake-up is a
/// descriptor in the poll set, and elsewhere it is the condition the wait is
/// on, so both are exercised through the same call the loop makes.
#[cfg(unix)]
fn wait_for_wake(wake: &Wake, timeout_ms: u64) {
    let mut descriptors = [libc::pollfd {
        fd: wake.read_descriptor(),
        events: libc::POLLIN,
        revents: 0,
    }];
    // SAFETY: the descriptor is borrowed from a live wake-up pipe and the
    // array outlives the call.
    unsafe {
        libc::poll(descriptors.as_mut_ptr(), 1, timeout_ms as libc::c_int);
    }
}

#[cfg(not(unix))]
fn wait_for_wake(wake: &Wake, timeout_ms: u64) {
    wake.wait(timeout_ms);
}
