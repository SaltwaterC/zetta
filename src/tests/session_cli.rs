use super::*;
use std::io::Cursor;

use crate::{BackgroundPaneLayout, BackgroundSessionSummary};

fn catalog(process_id: u32, runner_id: u64, session_ids: &[u64]) -> BackgroundSessionCatalog {
    BackgroundSessionCatalog {
        version: 3,
        process_id,
        runner_id,
        sessions: session_ids
            .iter()
            .map(|id| BackgroundSessionSummary {
                id: *id,
                title: format!("Session {id}"),
                authentication_required: *id == 2,
                active_pane: 1,
                layout: BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
            })
            .collect(),
    }
}

#[test]
fn finds_full_and_unique_bare_session_ids() {
    let catalogs = [catalog(123, 7, &[1, 2]), catalog(456, 8, &[3])];

    let full = find_session(&catalogs, "123:7:2").unwrap();
    assert_eq!(
        (full.process_id, full.runner_id, full.session_id),
        (123, 7, 2)
    );
    assert!(full.authentication_required);

    let bare = find_session(&catalogs, "3").unwrap();
    assert_eq!(
        (bare.process_id, bare.runner_id, bare.session_id),
        (456, 8, 3)
    );
}

#[test]
fn rejects_ambiguous_bare_session_ids() {
    let catalogs = [catalog(123, 7, &[1]), catalog(456, 8, &[1])];
    let error = find_session(&catalogs, "1").unwrap_err().to_string();
    assert!(error.contains("ambiguous"));
}

#[test]
fn masked_secret_input_shows_stars_and_supports_backspace() {
    let mut input = Cursor::new(b"ab\x7fc\n".to_vec());
    let mut output = Vec::new();
    let secret = read_masked_secret(&mut input, &mut output).unwrap();

    assert_eq!(&*secret, "ac");
    assert_eq!(String::from_utf8(output).unwrap(), "**\x08 \x08*");
}

#[test]
fn masked_secret_input_can_clear_with_ctrl_u_and_continue_typing() {
    let mut input = Cursor::new(b"abc\x15de\n".to_vec());
    let mut output = Vec::new();
    let secret = read_masked_secret(&mut input, &mut output).unwrap();

    assert_eq!(&*secret, "de");
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "***\x08 \x08\x08 \x08\x08 \x08**"
    );
}
