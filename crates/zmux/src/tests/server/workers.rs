use super::*;

#[test]
fn an_empty_shared_set_reports_no_input() {
    // The attribution a pane's exit carries. With no viewers there is nobody to
    // have typed, and an exclusive holder's own keystrokes are the truth for it
    // rather than something the daemon can see.
    assert!(!shared_input_sent(&Attachment::Shared(Vec::new())));
    assert!(!shared_input_sent(&Attachment::Exclusive(7)));
    assert!(!shared_input_sent(&Attachment::None));
    assert!(!shared_input_sent(&Attachment::Revoking { holder: 7 }));
}
