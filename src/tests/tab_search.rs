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
        limit_reached: false,
        total_count: 0,
        task: None,
    };
    assert!(tab_search_request_is_current(Some(&search), 7, 4, "cargo"));
    assert!(!tab_search_request_is_current(Some(&search), 7, 3, "cargo"));
    assert!(!tab_search_request_is_current(Some(&search), 7, 4, "rust"));
}

#[test]
fn tab_search_is_targeted_only_by_its_own_tab() {
    let search = TabSearch {
        tab_id: 7,
        query: String::new(),
        cursor: 0,
        select_all: false,
        generation: 0,
        matches: Vec::new(),
        active_match: None,
        limit_reached: false,
        total_count: 0,
        task: None,
    };

    assert!(tab_search_targets_tab(Some(&search), 7));
    assert!(!tab_search_targets_tab(Some(&search), 8));
    assert!(!tab_search_targets_tab(None, 7));
}
