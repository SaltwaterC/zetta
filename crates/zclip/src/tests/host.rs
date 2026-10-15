use super::*;

/// Paste withheld, as a tab starts; copy allowed, as it starts.
const COPY_ONLY: Permissions = Permissions {
    copy: true,
    paste: false,
};
const BOTH: Permissions = Permissions {
    copy: true,
    paste: true,
};

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
            COPY_ONLY,
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
    let response = host.handle(
        request(1, Message::Paste),
        COPY_ONLY,
        |_| Ok(()),
        || Ok(None),
    );
    assert!(matches!(response.unwrap().message, Message::Error(_)));
    host.handle(
        request(2, Message::Copy),
        COPY_ONLY,
        |_| Ok(()),
        || Ok(None),
    );
    let data = Message::Data {
        sequence: 0,
        bytes: b"once".to_vec(),
    };
    host.handle(request(2, data.clone()), COPY_ONLY, |_| Ok(()), || Ok(None));
    assert_eq!(
        host.handle(request(2, data), COPY_ONLY, |_| Ok(()), || Ok(None))
            .unwrap()
            .message,
        Message::Ack { next_sequence: 1 }
    );
    let mut copied = String::new();
    host.handle(
        request(2, Message::End),
        COPY_ONLY,
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
        BOTH,
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
                    BOTH,
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
    host.handle(
        request(4, Message::Copy),
        COPY_ONLY,
        |_| Ok(()),
        || Ok(None),
    );
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
        COPY_ONLY,
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
        BOTH,
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
        host.handle(cancel.clone(), COPY_ONLY, |_| Ok(()), || Ok(None))
            .is_none()
    );
    assert!(host.sessions.is_empty());
    // Nor is an unsolicited error answered.
    assert!(
        host.handle(cancel, COPY_ONLY, |_| Ok(()), || Ok(None))
            .is_none()
    );
    assert!(matches!(
        answer(&mut host, 1, Message::End),
        Message::Error(_)
    ));
}

fn paste_frame(
    host: &mut Host,
    id: u8,
    message: Message,
    allow_paste: bool,
    text: &str,
) -> Message {
    host.handle(
        request(id, message),
        Permissions {
            copy: true,
            paste: allow_paste,
        },
        |_| panic!("nothing should reach the clipboard"),
        || Ok(Some(text.to_owned())),
    )
    .expect("an answer")
    .message
}

/// The review's reproduction: a paste one byte longer than a chunk, started
/// while allowed, and acknowledged after the tab's permission was withdrawn.
/// The acknowledgement used to fetch the last byte of the captured clipboard.
#[test]
fn an_ack_after_paste_is_revoked_returns_no_data() {
    let mut host = Host::default();
    let text = "x".repeat(CHUNK_SIZE + 1);
    assert!(matches!(
        paste_frame(&mut host, 1, Message::Paste, true, &text),
        Message::Data { sequence: 0, .. }
    ));
    let response = paste_frame(&mut host, 1, Message::Ack { next_sequence: 1 }, false, "");
    assert_eq!(response, Message::Error(PASTE_DISABLED.into()));
    assert!(host.sessions.is_empty());
}

/// Revocation does not wait for the helper's next frame, and ends only pastes:
/// a copy writes the clipboard rather than reading it.
#[test]
fn revoking_paste_ends_every_paste_and_keeps_copies() {
    let mut host = Host::default();
    let text = "y".repeat(CHUNK_SIZE + 1);
    paste_frame(&mut host, 1, Message::Paste, true, &text);
    paste_frame(&mut host, 2, Message::Paste, true, &text);
    answer(&mut host, 3, Message::Copy);
    assert_eq!(host.sessions.len(), 3);
    host.revoke_paste();
    assert_eq!(host.sessions.keys().collect::<Vec<_>>(), vec![&[3; 16]]);
    // Re-allowing paste does not bring a revoked transfer back.
    assert!(matches!(
        paste_frame(&mut host, 1, Message::Ack { next_sequence: 1 }, true, ""),
        Message::Error(_)
    ));
    assert_eq!(
        answer(&mut host, 3, data(0, b"c")),
        Message::Ack { next_sequence: 1 }
    );
}

/// Any frame the host handles while paste is disallowed ends pastes in flight,
/// including another transfer's, so a missed revocation is caught up on.
#[test]
fn a_frame_handled_while_paste_is_disallowed_ends_pastes() {
    let mut host = Host::default();
    let text = "z".repeat(CHUNK_SIZE + 1);
    paste_frame(&mut host, 1, Message::Paste, true, &text);
    assert_eq!(
        paste_frame(&mut host, 2, Message::Probe, false, ""),
        Message::Ready
    );
    assert!(host.sessions.is_empty());
}

#[test]
fn a_fresh_paste_works_once_permission_is_restored() {
    let mut host = Host::default();
    let text = "w".repeat(CHUNK_SIZE + 1);
    paste_frame(&mut host, 1, Message::Paste, true, &text);
    host.revoke_paste();
    assert_eq!(
        paste_frame(&mut host, 2, Message::Paste, false, &text),
        Message::Error(PASTE_DISABLED.into())
    );
    let mut received = Vec::new();
    let mut response = paste_frame(&mut host, 2, Message::Paste, true, &text);
    while let Message::Data { sequence, bytes } = response {
        received.extend(bytes);
        response = paste_frame(
            &mut host,
            2,
            Message::Ack {
                next_sequence: sequence + 1,
            },
            true,
            "",
        );
    }
    assert_eq!(response, Message::Done);
    assert_eq!(received, text.as_bytes());
    assert!(host.sessions.is_empty());
}

/// The erasure covers the allocation, not only its length: a reader that
/// grew or truncated the buffer can leave clipboard bytes in spare capacity.
#[test]
fn erasing_a_paste_buffer_overwrites_its_spare_capacity() {
    let mut bytes = Vec::with_capacity(64);
    bytes.extend_from_slice(&[0xa5; 48]);
    bytes.truncate(5);
    let capacity = bytes.capacity();
    erase(&mut bytes);
    // SAFETY: `erase` wrote every byte of the allocation.
    unsafe { bytes.set_len(capacity) };
    assert!(bytes.iter().all(|&byte| byte == 0));
}

#[test]
fn next_expiry_names_the_earliest_transfer_and_expire_ends_it() {
    let mut host = Host::default();
    assert_eq!(host.next_expiry(), None);
    paste_frame(
        &mut host,
        1,
        Message::Paste,
        true,
        &"v".repeat(CHUNK_SIZE + 1),
    );
    answer(&mut host, 2, Message::Copy);
    let session = host.sessions.get_mut(&[1; 16]).unwrap();
    session.activity -= SESSION_TIMEOUT + Duration::from_secs(1);
    let stale = session.activity + SESSION_TIMEOUT;
    assert_eq!(host.next_expiry(), Some(stale));
    host.expire();
    assert_eq!(host.sessions.keys().collect::<Vec<_>>(), vec![&[2; 16]]);
    assert!(host.next_expiry().unwrap() > Instant::now());
}

/// Answers `message` with copy allowed or not, recording what reached the
/// clipboard.
fn copy_frame(host: &mut Host, id: u8, message: Message, allow_copy: bool) -> (Message, String) {
    let mut copied = String::new();
    let message = host
        .handle(
            request(id, message),
            Permissions {
                copy: allow_copy,
                paste: false,
            },
            |text| {
                copied = text.to_owned();
                Ok(())
            },
            || Ok(None),
        )
        .expect("an answer")
        .message;
    (message, copied)
}

/// A copy spooled while allowed and ended after the tab withdrew copy never
/// reaches the clipboard.
#[test]
fn an_end_after_copy_is_revoked_never_reaches_the_clipboard() {
    let mut host = Host::default();
    copy_frame(&mut host, 1, Message::Copy, true);
    copy_frame(&mut host, 1, data(0, b"late"), true);
    let (response, copied) = copy_frame(&mut host, 1, Message::End, false);
    assert_eq!(response, Message::Error(COPY_DISABLED.into()));
    assert_eq!(copied, "");
    assert!(host.sessions.is_empty());
}

#[test]
fn a_chunk_after_copy_is_revoked_is_refused_and_ends_the_copy() {
    let mut host = Host::default();
    copy_frame(&mut host, 1, Message::Copy, true);
    let (response, _) = copy_frame(&mut host, 1, data(0, b"a"), false);
    assert_eq!(response, Message::Error(COPY_DISABLED.into()));
    assert!(host.sessions.is_empty());
    // Re-allowing copy does not bring the revoked transfer back.
    let (response, copied) = copy_frame(&mut host, 1, Message::End, true);
    assert!(matches!(response, Message::Error(_)));
    assert_eq!(copied, "");
}

#[test]
fn a_copy_is_refused_while_copy_is_disallowed() {
    let mut host = Host::default();
    let (response, _) = copy_frame(&mut host, 1, Message::Copy, false);
    assert_eq!(response, Message::Error(COPY_DISABLED.into()));
    assert!(host.sessions.is_empty());
    let (response, _) = copy_frame(&mut host, 2, Message::Copy, true);
    assert_eq!(response, Message::Ack { next_sequence: 0 });
}

/// Revocation does not wait for the helper's next frame, and ends only copies.
#[test]
fn revoking_copy_ends_every_copy_and_keeps_pastes() {
    let mut host = Host::default();
    copy_frame(&mut host, 1, Message::Copy, true);
    copy_frame(&mut host, 2, Message::Copy, true);
    paste_frame(
        &mut host,
        3,
        Message::Paste,
        true,
        &"p".repeat(CHUNK_SIZE + 1),
    );
    host.revoke_copy();
    assert_eq!(host.sessions.keys().collect::<Vec<_>>(), vec![&[3; 16]]);
    assert!(matches!(
        paste_frame(&mut host, 3, Message::Ack { next_sequence: 1 }, true, ""),
        Message::Data { sequence: 1, .. }
    ));
}

/// Any frame handled while copy is disallowed ends copies in flight, so a
/// missed revocation is caught up on.
#[test]
fn a_frame_handled_while_copy_is_disallowed_ends_copies() {
    let mut host = Host::default();
    copy_frame(&mut host, 1, Message::Copy, true);
    let (response, _) = copy_frame(&mut host, 2, Message::Probe, false);
    assert_eq!(response, Message::Ready);
    assert!(host.sessions.is_empty());
}
