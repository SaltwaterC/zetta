use super::*;
use crate::protocol;

fn catalog(process_id: u32, runner_id: u64, session_ids: &[u64]) -> BackgroundSessionCatalog {
    BackgroundSessionCatalog {
        version: protocol::CATALOG_VERSION,
        process_id,
        runner_id,
        sessions: session_ids
            .iter()
            .map(|id| protocol::BackgroundSessionSummary {
                id: *id,
                title: format!("Session {id}"),
                authentication_required: *id == 2,
                active_pane: 1,
                layout: protocol::BackgroundPaneLayout::Pane { pane_id: 1 },
                panes: Vec::new(),
                held: false,
                scoped_to: None,
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
    assert!(error.contains("ambiguous"), "{error}");
}

#[test]
fn rejects_zero_components_in_full_session_ids() {
    let error = find_session(&[catalog(123, 7, &[1])], "0:7:1")
        .unwrap_err()
        .to_string();
    assert!(error.contains("positive whole numbers"), "{error}");
}

#[test]
fn reconnect_origin_requires_positive_process_and_attention_ids() {
    assert_eq!(
        parse_reconnect_origin("123", "456"),
        Some(ReconnectOrigin {
            process_id: 123,
            attention_id: 456,
        })
    );
    for (process_id, attention_id) in [("0", "456"), ("123", "0"), ("not-a-pid", "456")] {
        assert_eq!(parse_reconnect_origin(process_id, attention_id), None);
    }
}
