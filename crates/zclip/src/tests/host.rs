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
    assert!(matches!(response.message, Message::Error(_)));
    host.handle(request(2, Message::Copy), false, |_| Ok(()), || Ok(None));
    let data = Message::Data {
        sequence: 0,
        bytes: b"once".to_vec(),
    };
    host.handle(request(2, data.clone()), false, |_| Ok(()), || Ok(None));
    assert_eq!(
        host.handle(request(2, data), false, |_| Ok(()), || Ok(None))
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
        match response.message {
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
    assert!(matches!(response.message, Message::Error(_)));
    assert!(host.sessions.is_empty());
}
