use super::*;
const MODE: OutputMode = OutputMode { lf_implies_cr: true, wrap: true };
#[test]
fn raw_bytes_and_mode_seams_survive_transport_and_end() {
    let (writer, mut reader) = channel();
    writer.write(b"abc\r\n", MODE).unwrap();
    let next = OutputMode { lf_implies_cr: false, wrap: true };
    writer.write(b"x\n", next).unwrap();
    writer.end();
    assert_eq!(reader.mode().unwrap(), Some(MODE));
    let mut bytes = [0; 16];
    assert_eq!(reader.read(&mut bytes).unwrap(), 5);
    assert_eq!(&bytes[..5], b"abc\r\n");
    assert_eq!(reader.mode().unwrap(), Some(next));
    assert_eq!(reader.read(&mut bytes).unwrap(), 2);
    assert!(reader.ended());
}
#[test]
fn flush_waits_for_parser_acknowledgement_and_cancel_wakes_it() {
    let (writer, mut reader) = channel();
    writer.write(b"abc", MODE).unwrap();
    reader.read_exact(&mut [0; 3]).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let producer = writer.clone();
    let thread = std::thread::spawn(move || {
        sender.send(producer.flush()).unwrap();
    });
    assert!(receiver.recv_timeout(std::time::Duration::from_millis(20)).is_err());
    reader.acknowledgement().parsed(3);
    receiver.recv_timeout(std::time::Duration::from_secs(1)).unwrap().unwrap();
    thread.join().unwrap();
    writer.write(b"tail", MODE).unwrap();
    let producer = writer.clone();
    let thread = std::thread::spawn(move || producer.flush());
    writer.cancel();
    assert_eq!(thread.join().unwrap().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
}
#[test]
fn small_writes_coalesce_and_full_queue_close_unblocks_the_writer() {
    let (writer, _reader) = channel();
    for _ in 0..100 {
        writer.write(b"a", MODE).unwrap();
    }
    assert_eq!(writer.0.lock().ready.len(), 1);
    writer.write(&vec![b'a'; CAPACITY - 100], MODE).unwrap();
    let producer = writer.clone();
    let thread = std::thread::spawn(move || producer.write(b"blocked", MODE));
    writer.cancel();
    assert_eq!(thread.join().unwrap().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
}

#[test]
fn a_write_after_an_empty_mode_sample_waits_for_the_next_sample() {
    let (writer, mut reader) = channel();
    assert_eq!(reader.mode().unwrap(), None);
    writer.write(b"abc\n", MODE).unwrap();
    assert_eq!(reader.read(&mut [0; 8]).unwrap_err().kind(), io::ErrorKind::WouldBlock);
    assert_eq!(reader.mode().unwrap(), Some(MODE));
    assert_eq!(reader.read(&mut [0; 8]).unwrap(), 4);
}
#[test]
fn readiness_posts_once_until_the_reader_rearms() {
    let (writer, mut reader) = channel();
    let poller = Arc::new(Poller::new().unwrap());
    reader.register(&poller, Event::readable(7), PollMode::Oneshot);
    for _ in 0..100 {
        writer.write(b"x", MODE).unwrap();
    }
    let mut events = polling::Events::new();
    poller.wait(&mut events, Some(std::time::Duration::from_secs(1))).unwrap();
    assert_eq!(events.len(), 1);
    events.clear();
    poller.wait(&mut events, Some(std::time::Duration::ZERO)).unwrap();
    assert!(events.is_empty());
    reader.rearm();
    poller.wait(&mut events, Some(std::time::Duration::from_secs(1))).unwrap();
    assert_eq!(events.len(), 1);
}
