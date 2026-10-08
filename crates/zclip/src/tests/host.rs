use super::*;

fn request(id: u8, message: Message) -> Frame {
    Frame {
        id: [id; 16],
        message,
    }
}

#[test]
fn copy_commits_only_after_valid_utf8_end() {
    let mut host = Host::default();
    let copied = std::cell::RefCell::new(String::new());
    let mut handle = |frame| {
        host.handle(
            frame,
            false,
            |text| {
                *copied.borrow_mut() = text.to_owned();
                Ok(())
            },
            || Ok(None),
        )
        .unwrap()
        .message
    };
    assert_eq!(
        handle(request(1, Message::Copy)),
        Message::Ack { next_sequence: 0 }
    );
    assert_eq!(
        handle(request(
            1,
            Message::Data {
                sequence: 0,
                bytes: b"he".to_vec(),
            }
        )),
        Message::Ack { next_sequence: 1 }
    );
    assert_eq!(*copied.borrow(), "");
    assert_eq!(handle(request(1, Message::End)), Message::Done);
    assert_eq!(*copied.borrow(), "he");

    assert_eq!(
        handle(request(2, Message::Copy)),
        Message::Ack { next_sequence: 0 }
    );
    handle(request(
        2,
        Message::Data {
            sequence: 0,
            bytes: vec![0xff],
        },
    ));
    assert!(matches!(
        handle(request(2, Message::End)),
        Message::Error(_)
    ));
    assert_eq!(*copied.borrow(), "he");
}

#[test]
fn denied_paste_and_duplicate_copy_chunk() {
    let mut host = Host::default();
    let response = host.handle(request(1, Message::Paste), false, |_| Ok(()), || Ok(None));
    assert!(matches!(response.unwrap().message, Message::Error(_)));
    host.handle(request(2, Message::Copy), false, |_| Ok(()), || Ok(None));
    let data = Message::Data {
        sequence: 0,
        bytes: b"once".to_vec(),
    };
    host.handle(request(2, data.clone()), false, |_| Ok(()), || Ok(None));
    assert_eq!(
        host.handle(request(2, data), false, |_| Ok(()), || Ok(None))
            .unwrap()
            .message,
        Message::Ack { next_sequence: 1 }
    );
    let mut copied = String::new();
    host.handle(
        request(2, Message::End),
        false,
        |text| {
            copied = text.into();
            Ok(())
        },
        || Ok(None),
    );
    assert_eq!(copied, "once");
}

#[test]
fn paste_streams_in_chunks() {
    let mut host = Host::default();
    let text = "a".repeat(CHUNK_SIZE * 2 + 7);
    let mut received = Vec::new();
    let mut response = host.handle(
        request(3, Message::Paste),
        true,
        |_| Ok(()),
        || Ok(Some(text.clone())),
    );
    let mut sequence = 0;
    loop {
        match response.unwrap().message {
            Message::Data {
                sequence: actual,
                bytes,
            } => {
                assert_eq!(actual, sequence);
                received.extend(bytes);
                sequence += 1;
                response = host.handle(
                    request(
                        3,
                        Message::Ack {
                            next_sequence: sequence,
                        },
                    ),
                    true,
                    |_| Ok(()),
                    || Ok(None),
                );
            }
            Message::Done => break,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(received, text.as_bytes());
}

#[test]
fn stale_transfer_is_discarded_before_the_next_request() {
    let mut host = Host::default();
    host.handle(request(4, Message::Copy), false, |_| Ok(()), || Ok(None));
    host.sessions.get_mut(&[4; 16]).unwrap().activity -=
        SESSION_TIMEOUT + std::time::Duration::from_secs(1);
    let response = host.handle(
        request(
            4,
            Message::Data {
                sequence: 0,
                bytes: b"late".to_vec(),
            },
        ),
        false,
        |_| Ok(()),
        || Ok(None),
    );
    assert!(matches!(response.unwrap().message, Message::Error(_)));
    assert!(host.sessions.is_empty());
}

fn limited(transfer_bytes: usize, held_bytes: usize) -> Host {
    Host {
        limits: Limits {
            transfer_bytes,
            held_bytes,
            ..Limits::default()
        },
        ..Host::default()
    }
}

fn data(sequence: u64, bytes: &[u8]) -> Message {
    Message::Data {
        sequence,
        bytes: bytes.to_vec(),
    }
}

fn answer(host: &mut Host, id: u8, message: Message) -> Message {
    host.handle(
        request(id, message),
        true,
        |_| panic!("nothing should reach the clipboard"),
        || Ok(Some("0123456789".into())),
    )
    .expect("an answer")
    .message
}

/// Each chunk is valid on its own; only their sum is too large.
#[test]
fn repeated_valid_chunks_stop_at_the_transfer_limit() {
    let mut host = limited(10, 100);
    assert_eq!(
        answer(&mut host, 1, Message::Copy),
        Message::Ack { next_sequence: 0 }
    );
    for sequence in 0..2 {
        assert_eq!(
            answer(&mut host, 1, data(sequence, b"abcd")),
            Message::Ack {
                next_sequence: sequence + 1
            }
        );
    }
    assert!(matches!(
        answer(&mut host, 1, data(2, b"abcd")),
        Message::Error(_)
    ));
    assert!(host.sessions.is_empty());
    assert!(matches!(
        answer(&mut host, 1, Message::End),
        Message::Error(_)
    ));
}

#[test]
fn concurrent_transfers_share_one_byte_budget() {
    let mut host = limited(10, 12);
    answer(&mut host, 1, Message::Copy);
    answer(&mut host, 2, Message::Copy);
    assert_eq!(
        answer(&mut host, 1, data(0, b"12345678")),
        Message::Ack { next_sequence: 1 }
    );
    assert_eq!(
        answer(&mut host, 2, data(0, b"abcd")),
        Message::Ack { next_sequence: 1 }
    );
    assert!(matches!(
        answer(&mut host, 2, data(1, b"e")),
        Message::Error(_)
    ));
    // A paste is held in memory and counts against the same budget.
    assert!(matches!(
        answer(&mut host, 3, Message::Paste),
        Message::Error(_)
    ));
    assert_eq!(host.sessions.len(), 1);
}

#[test]
fn a_paste_larger_than_the_transfer_limit_is_refused() {
    let mut host = limited(9, 100);
    assert!(matches!(
        answer(&mut host, 1, Message::Paste),
        Message::Error(_)
    ));
    assert!(host.sessions.is_empty());
    let mut host = limited(10, 100);
    assert!(matches!(
        answer(&mut host, 1, Message::Paste),
        Message::Data { .. }
    ));
}

/// Data refreshes the idle timeout but not the transfer's lifetime.
#[test]
fn a_transfer_that_keeps_sending_still_ends_at_its_lifetime() {
    let mut host = Host::default();
    answer(&mut host, 1, Message::Copy);
    answer(&mut host, 1, data(0, b"a"));
    let session = host.sessions.get_mut(&[1; 16]).unwrap();
    session.started -= TRANSFER_LIFETIME + Duration::from_secs(1);
    session.activity = Instant::now();
    assert!(matches!(
        answer(&mut host, 1, data(1, b"b")),
        Message::Error(_)
    ));
    assert!(host.sessions.is_empty());
}

/// An abandoned session's spool is released, and with it its share of the
/// byte budget.
#[test]
fn an_abandoned_transfer_releases_its_bytes() {
    let mut host = limited(10, 10);
    answer(&mut host, 1, Message::Copy);
    answer(&mut host, 1, data(0, b"0123456789"));
    answer(&mut host, 2, Message::Copy);
    assert!(matches!(
        answer(&mut host, 2, data(0, b"a")),
        Message::Error(_)
    ));
    host.sessions.get_mut(&[1; 16]).unwrap().activity -= SESSION_TIMEOUT + Duration::from_secs(1);
    answer(&mut host, 2, Message::Copy);
    assert_eq!(
        answer(&mut host, 2, data(0, b"a")),
        Message::Ack { next_sequence: 1 }
    );
    assert_eq!(host.sessions.len(), 1);
}

/// A helper that gives up says so with an error frame; the host drops the
/// transfer and does not answer a helper that has stopped reading.
#[test]
fn a_helper_error_cancels_its_transfer_without_an_answer() {
    let mut host = Host::default();
    answer(&mut host, 1, Message::Copy);
    answer(&mut host, 1, data(0, b"partial"));
    let cancel = request(1, Message::Error("cancelled".into()));
    assert!(
        host.handle(cancel.clone(), false, |_| Ok(()), || Ok(None))
            .is_none()
    );
    assert!(host.sessions.is_empty());
    // Nor is an unsolicited error answered.
    assert!(
        host.handle(cancel, false, |_| Ok(()), || Ok(None))
            .is_none()
    );
    assert!(matches!(
        answer(&mut host, 1, Message::End),
        Message::Error(_)
    ));
}
