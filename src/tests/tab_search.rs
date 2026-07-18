use super::*;

#[test]
fn stale_tab_search_work_is_rejected() {
    let search = TabSearch {
        tab_id: 7,
        query: "cargo".into(),
        cursor: 0,
        select_all: false,
        generation: 4,
        matches: Vec::new(),
        active_match: None,
        task: None,
    };
    assert!(tab_search_request_is_current(Some(&search), 7, 4, "cargo"));
    assert!(!tab_search_request_is_current(Some(&search), 7, 3, "cargo"));
    assert!(!tab_search_request_is_current(Some(&search), 7, 4, "rust"));
}
