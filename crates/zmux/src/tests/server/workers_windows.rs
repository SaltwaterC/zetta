use super::*;
use std::io::{PipeReader, PipeWriter, Read, Write};

fn wait_pair() -> (DrainWait, Stream) {
    let (reader, writer) = Stream::pair().unwrap();
    reader.set_nonblocking(true).unwrap();
    writer.set_nonblocking(true).unwrap();
    (DrainWait::new(reader), writer)
}

fn pipes() -> (tty::Pty, PipeWriter, PipeReader) {
    let (output, producer) = std::io::pipe().unwrap();
    let (consumer, input) = std::io::pipe().unwrap();
    let (pty, _exits) = tty::attach(output.into(), input.into(), 0).unwrap();
    (pty, producer, consumer)
}

fn settle(wait: &mut DrainWait) {
    // Initial registration and a racing completion can both post. Consume
    // those packets before asserting that an idle registration stays asleep.
    for _ in 0..8 {
        wait.wait_ready(Duration::ZERO).unwrap();
        if wait.events.is_empty() {
            return;
        }
    }
    panic!("readiness did not settle");
}

fn assert_idle(wait: &mut DrainWait) {
    let started = Instant::now();
    wait.wait_ready(Duration::from_millis(100)).unwrap();
    assert!(wait.events.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(80));
}

#[test]
fn idle_wait_uses_the_deadline_and_control_work_interrupts_it() {
    let (mut wait, mut control) = wait_pair();
    assert_idle(&mut wait);
    // A notification already queued must not be discarded before the wait.
    control.write_all(b".").unwrap();
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    assert!(wait.events.iter().any(|event| event.key == CONTROL));
    assert_idle(&mut wait);

    let sender = thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        control.write_all(b".").unwrap();
        control
    });
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    assert!(wait.events.iter().any(|event| event.key == CONTROL));
    let _control = sender.join().unwrap();
    assert_idle(&mut wait);
}

#[test]
fn output_after_arming_wakes_the_wait_and_rearms_for_the_next_burst() {
    let (mut wait, _control) = wait_pair();
    let (mut pty, mut producer, _consumer) = pipes();
    for _ in 0..3 {
        arm_pipes(&mut pty, &wait.poller, true, false);
        settle(&mut wait);
        assert_idle(&mut wait);
        let sender = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            producer.write_all(b"echo").unwrap();
            producer
        });
        wait.wait_ready(Duration::from_secs(1)).unwrap();
        assert!(
            wait.events
                .iter()
                .any(|event| event.key == PIPE && event.readable)
        );
        producer = sender.join().unwrap();
        let mut bytes = [0; 4];
        assert_eq!(pty.reader().try_read(&mut bytes), 4);
        assert_eq!(&bytes, b"echo");
    }
    pty.pause_reader().unwrap();
}

#[test]
fn buffered_output_survives_disarming_and_exclusive_handover() {
    let (mut wait, _control) = wait_pair();
    let (mut pty, mut producer, _consumer) = pipes();
    arm_pipes(&mut pty, &wait.poller, true, false);
    settle(&mut wait);
    producer.write_all(b"before").unwrap();
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    // The queue is already readable when its interest is installed again.
    arm_pipes(&mut pty, &wait.poller, false, false);
    settle(&mut wait);
    arm_pipes(&mut pty, &wait.poller, true, false);
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    assert!(wait.events.iter().any(|event| event.key == PIPE));
    pty.pause_reader().unwrap();
    arm_pipes(&mut pty, &wait.poller, false, false);
    let mut bytes = [0; 6];
    assert_eq!(pty.read_buffered(&mut bytes), 6);
    assert_eq!(&bytes, b"before");
    settle(&mut wait);

    producer.write_all(b"after!").unwrap();
    // Neither disarming nor another idle pass can restart an exclusive reader.
    arm_pipes(&mut pty, &wait.poller, false, false);
    assert_idle(&mut wait);
    assert_eq!(pty.read_buffered(&mut bytes), 0);
    pty.resume_reader();
    arm_pipes(&mut pty, &wait.poller, true, false);
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    assert!(wait.events.iter().any(|event| event.key == PIPE));
    assert_eq!(pty.reader().try_read(&mut bytes), 6);
    assert_eq!(&bytes, b"after!");
    pty.pause_reader().unwrap();
}

#[test]
fn a_full_input_queue_wakes_when_the_consumer_makes_room() {
    let (mut wait, _control) = wait_pair();
    let (mut pty, _producer, mut consumer) = pipes();
    let chunk = vec![b'x'; 64 * 1024];
    let mut accepted = 0;
    loop {
        let count = pty.writer().write(&chunk).unwrap();
        accepted += count;
        assert!(
            accepted < 16 * 1024 * 1024,
            "the input queue must be bounded"
        );
        if count == 0 {
            break;
        }
    }
    arm_pipes(&mut pty, &wait.poller, false, true);
    settle(&mut wait);
    // The native pipe writer may have made a final partial write before it
    // blocked. Fill that newly available room too, then arm the blocked write.
    loop {
        let count = pty.writer().write(&chunk).unwrap();
        accepted += count;
        if count == 0 {
            break;
        }
    }
    arm_pipes(&mut pty, &wait.poller, false, true);
    settle(&mut wait);
    assert_idle(&mut wait);
    let reader = thread::spawn(move || {
        let mut bytes = vec![0; accepted];
        consumer.read_exact(&mut bytes).unwrap();
        assert!(bytes.iter().all(|byte| *byte == b'x'));
    });
    wait.wait_ready(Duration::from_secs(1)).unwrap();
    assert!(
        wait.events
            .iter()
            .any(|event| event.key == PIPE && event.writable)
    );
    reader.join().unwrap();
}

#[test]
fn large_output_crosses_the_bounded_pipe_in_order_and_hangup_does_not_spin() {
    let (mut wait, _control) = wait_pair();
    let (mut pty, mut producer, _consumer) = pipes();
    let expected: Vec<u8> = (0..4 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    let payload = expected.clone();
    let writer = thread::spawn(move || producer.write_all(&payload).unwrap());
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while received.len() < expected.len() && Instant::now() < deadline {
        arm_pipes(&mut pty, &wait.poller, true, false);
        wait.wait_ready(Duration::from_secs(1)).unwrap();
        let mut buffer = [0; 16 * 1024];
        let count = pty.reader().try_read(&mut buffer);
        received.extend_from_slice(&buffer[..count]);
    }
    assert_eq!(received, expected);
    writer.join().unwrap();
    pty.pause_reader().unwrap();
    arm_pipes(&mut pty, &wait.poller, false, false);
    settle(&mut wait);
    assert_idle(&mut wait);
}
