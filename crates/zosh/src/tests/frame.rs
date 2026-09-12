use super::*;

use mosh_rs::Base64Key;

/// A session with nothing on the other end. The frame decision reads only the
/// size of the displayed screen and what the caller passes it, so no server is
/// needed to exercise it — and the two loops that share it are what this
/// pins, not the protocol.
fn offline_session(columns: u16, rows: u16) -> ClientSession {
    let key = Base64Key::from_printable(&"A".repeat(22)).expect("a canonical all-zero key");
    MoshSession::connect_with_screen("127.0.0.1", 1, &key, DisplayScreen::new(rows, columns))
        .expect("binding a local socket needs no peer")
}

/// Until the server catches up with a resize the client asked for, every frame
/// still describes the old geometry and must not be painted; the frame that
/// does catch up is a whole-screen repaint.
#[test]
fn a_pending_resize_skips_frames_until_the_server_reports_the_new_size() {
    let session = offline_session(80, 24);

    assert_eq!(next_frame(&session, &[], Some((100, 30))), Frame::Skip);
    assert_eq!(
        next_frame(&session, &[], Some((80, 24))),
        Frame::Repaint {
            resolves_pending: true
        }
    );
    assert_eq!(next_frame(&session, &[], None), Frame::Paint);
}

/// A resize the server started has to be repainted whole as well, or the next
/// incremental frame lands on a screen of a different shape.
#[test]
fn a_server_reported_resize_repaints_when_no_resize_is_in_flight() {
    let session = offline_session(80, 24);
    let resized = [HostEvent::Resize {
        width: 80,
        height: 24,
    }];

    assert_eq!(
        next_frame(&session, &resized, None),
        Frame::Repaint {
            resolves_pending: false
        }
    );
    assert_eq!(
        next_frame(&session, &resized, Some((100, 30))),
        Frame::Skip,
        "the client's own resize is still in flight and decides the frame"
    );
}
