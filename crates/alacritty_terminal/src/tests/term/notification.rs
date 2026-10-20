//! OSC 99 assembly, after the cases of Shitty's notification suites.
use super::*;
use crate::term::test::TermSize;
use crate::vte::ansi::{Processor, StdSyncHandler};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
struct Listener {
    replies: Rc<RefCell<Vec<u8>>>,
    shown: Rc<RefCell<Vec<DesktopNotification>>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(reply) => self.replies.borrow_mut().extend(reply.bytes()),
            Event::DesktopNotification(notification) => self.shown.borrow_mut().push(notification),
            _ => (),
        }
    }
}

struct Harness {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    listener: Listener,
}

impl Harness {
    fn new(enabled: bool) -> Self {
        let listener = Listener::default();
        let config = Config { desktop_notifications: enabled, ..Config::default() };
        let term = Term::new(config, &TermSize::new(8, 2), listener.clone());
        Self { term, parser: Processor::new(), listener }
    }

    fn write(&mut self, bytes: &[u8]) -> &mut Self {
        self.parser.advance(&mut self.term, bytes);
        self
    }

    fn send(&mut self, metadata: &str, payload: &[u8]) -> &mut Self {
        let mut sequence = format!("\x1b]99;{metadata};").into_bytes();
        sequence.extend_from_slice(payload);
        sequence.extend_from_slice(b"\x1b\\");
        self.write(&sequence)
    }

    /// `(id, title, body)` of everything shown since the last call.
    fn shown(&self) -> Vec<(Option<String>, String, String)> {
        self.listener
            .shown
            .take()
            .into_iter()
            .map(|shown| (shown.id, shown.title, shown.body))
            .collect()
    }

    fn replies(&self) -> Vec<u8> {
        self.listener.replies.take()
    }
}

fn shown(id: Option<&str>, title: &str, body: &str) -> (Option<String>, String, String) {
    (id.map(str::to_owned), title.to_owned(), body.to_owned())
}

#[test]
fn a_frontend_that_cannot_deliver_neither_shows_nor_answers() {
    let mut harness = Harness::new(false);
    harness.send("", b"Hello").send("i=q:p=?", b"");
    assert!(harness.shown().is_empty());
    assert!(harness.replies().is_empty());
}

#[test]
fn the_default_payload_is_a_title_and_a_body_alone_is_promoted() {
    let mut harness = Harness::new(true);
    harness.send("", b"Hello world").send("p=body", b"Body only");
    assert_eq!(harness.shown(), [shown(None, "Hello world", ""), shown(None, "Body only", "")]);
}

#[test]
fn chunks_concatenate_per_part_until_done() {
    let mut harness = Harness::new(true);
    harness.send("i=job:p=title:d=0", b"Build").send("i=job:p=body:d=0", b"half ");
    assert!(harness.shown().is_empty());
    harness.send("i=job:p=body", b"done");
    assert_eq!(harness.shown(), [shown(Some("job"), "Build", "half done")]);
}

#[test]
fn a_payload_may_contain_semicolons() {
    let mut harness = Harness::new(true);
    harness.send("", b"a;b;c");
    assert_eq!(harness.shown(), [shown(None, "a;b;c", "")]);
}

#[test]
fn base64_may_split_between_quartets_and_mix_with_plain_chunks() {
    let mut harness = Harness::new(true);
    harness.send("i=job:e=1:d=0", b"QnV").send("i=job:e=1", b"pbGQ=");
    harness.send("i=mix:e=1:d=0", b"aGV4").send("i=mix", b" tail");
    assert_eq!(
        harness.shown(),
        [shown(Some("job"), "Build", ""), shown(Some("mix"), "hex tail", "")]
    );
}

#[test]
fn base64_survives_every_pty_boundary() {
    let sequence = b"\x1b]99;i=job:e=1;QnVpbGQ=\x1b\\";
    for split in 1..sequence.len() {
        let mut harness = Harness::new(true);
        harness.write(&sequence[..split]).write(&sequence[split..]);
        assert_eq!(harness.shown(), [shown(Some("job"), "Build", "")], "split at {split}");
    }
}

#[test]
fn broken_base64_and_unfinished_quartets_drop_the_notification() {
    let mut harness = Harness::new(true);
    harness.send("e=1", b"@@@@").send("e=1", b"!!!=").send("e=1", b"QnV");
    harness.send("i=x:e=1:d=0", b"QnV").send("i=x", b"plain");
    assert!(harness.shown().is_empty());
}

#[test]
fn invalid_identifiers_and_known_values_reject_the_sequence() {
    let mut harness = Harness::new(true);
    for metadata in ["i=bad=id", "i=bad value", "i=bad,comma", "d=2", "d=yes", "e=2", "o=never"] {
        harness.send(metadata, b"ignored");
    }
    harness.send("i=bad=id:p=?", b"");
    assert!(harness.shown().is_empty());
    assert!(harness.replies().is_empty());
}

#[test]
fn unknown_keys_are_skipped_and_unknown_payloads_ignored() {
    let mut harness = Harness::new(true);
    harness.send("z=future", b"Hello").send("i=job:p=icon", b"ignored");
    assert_eq!(harness.shown(), [shown(None, "Hello", "")]);
}

#[test]
fn text_must_be_utf8_without_controls() {
    let mut harness = Harness::new(true);
    harness.send("", b"bad\xffutf8").send("e=1", b"G1sybQ==").send("", "é€😀".as_bytes());
    assert_eq!(harness.shown(), [shown(None, "é€😀", "")]);
}

#[test]
fn chunk_limits_are_2048_plain_and_4096_encoded() {
    let mut harness = Harness::new(true);
    harness.send("", &[b'A'; 2048]).send("", &[b'A'; 2049]);
    let encoded = Base64.encode([b'B'; 3072]);
    harness.send("e=1", encoded.as_bytes());
    harness.send("e=1", Base64.encode([b'B'; 3073]).as_bytes());
    let shown = harness.shown();
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0].1.len(), 2048);
    assert_eq!(shown[1].1, "B".repeat(3072));
}

#[test]
fn a_notification_is_limited_to_8192_bytes_and_consumed_when_over() {
    let mut harness = Harness::new(true);
    for _ in 0..3 {
        harness.send("i=max:d=0", &[b'A'; 2048]);
    }
    harness.send("i=max", &[b'A'; 2048]);
    assert_eq!(harness.shown(), [shown(Some("max"), &"A".repeat(8192), "")]);
    for _ in 0..4 {
        harness.send("i=over:d=0", &[b'B'; 2048]);
    }
    harness.send("i=over:d=0", b"B").send("i=over", b"B").send("", b"ok");
    assert_eq!(harness.shown(), [shown(None, "ok", "")]);
}

#[test]
fn close_abandons_an_unfinished_notification_and_ids_are_reusable() {
    let mut harness = Harness::new(true);
    harness.send("i=job:d=0", b"Half").send("i=job:p=close", b"").send("i=job", b"Next");
    harness.send("i=job", b"Again");
    assert_eq!(harness.shown(), [shown(Some("job"), "Next", ""), shown(Some("job"), "Again", "")]);
}

#[test]
fn metadata_from_any_chunk_applies() {
    let mut harness = Harness::new(true);
    harness.send("i=job:d=0:o=unfocused", b"Build").send("i=job:w=0:f=WmV0dGE=", b"");
    let shown = harness.listener.shown.take();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].occasion, NotificationOccasion::Unfocused);
    assert_eq!(shown[0].expiry, NotificationExpiry::Never);
    assert_eq!(shown[0].app_name.as_deref(), Some("Zetta"));
}

#[test]
fn the_query_lists_only_what_is_delivered_in_the_queried_terminator() {
    let mut harness = Harness::new(true);
    harness.send("i=query:p=?", b"").write(b"\x1b]99;p=?;\x07");
    assert_eq!(
        harness.replies(),
        b"\x1b]99;i=query:p=?;a=focus:o=always,unfocused,invisible:p=title,body,?:w=1\x1b\\\
          \x1b]99;p=?;a=focus:o=always,unfocused,invisible:p=title,body,?:w=1\x07"
    );
}
