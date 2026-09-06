use super::*;

fn healthy() -> LinkHealth {
    LinkHealth {
        since_heard_ms: 100,
        since_ack_ms: 100,
    }
}

#[test]
fn healthy_links_do_not_paint_a_status_bar() {
    let mut notifier = Notifier::new(Some("Ctrl-^"));
    assert!(notifier.bar(healthy(), 1_000, 80).is_empty());
}

#[test]
fn late_links_report_missing_contact_or_reply() {
    let mut notifier = Notifier::new(Some("Ctrl-^"));
    let contact = notifier.bar(
        LinkHealth {
            since_heard_ms: 12_000,
            since_ack_ms: 12_000,
        },
        20_000,
        80,
    );
    let text: String = contact
        .iter()
        .map(|cell| cell.cell.contents.as_str())
        .collect();
    assert!(text.starts_with("zosh: Last contact 12 seconds ago."));
    assert!(text.contains("To quit: Ctrl-^ ."));

    let reply = notifier.bar(
        LinkHealth {
            since_heard_ms: 100,
            since_ack_ms: 15_000,
        },
        20_000,
        80,
    );
    let text: String = reply
        .iter()
        .map(|cell| cell.cell.contents.as_str())
        .collect();
    assert!(text.starts_with("zosh: Last reply 15 seconds ago."));
}

#[test]
fn messages_expire_and_are_clipped_to_one_row() {
    let mut notifier = Notifier::new(None);
    notifier.say("message", 1_000);
    assert_eq!(notifier.bar(healthy(), 1_000, 7).len(), 7);
    assert!(notifier.bar(healthy(), 2_000, 7).is_empty());
}
