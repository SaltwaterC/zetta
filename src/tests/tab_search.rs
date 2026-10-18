use super::*;

#[gpui::test]
fn tab_search_edits_repaint_before_results_after_resize(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        theme_settings::init(
            theme::LoadThemes::All(Box::new(crate::zetta_assets::ZettaAssets)),
            cx,
        );
        let registry = ThemeRegistry::global(cx);
        theme_settings::load_bundled_themes(&registry);
        theme::GlobalTheme::update_theme(cx, registry.get("One Light").unwrap());
        terminal::terminal_settings::TerminalSettings::init(cx);
    });
    let (zetta, cx) = cx.add_window_view(|window, cx| {
        let mut config = Config::defaults(None, None);
        config.profiles.clear();
        let mut zetta = Zetta::new(
            config,
            None,
            ZettaLaunchOptions {
                no_mux: true,
                ..Default::default()
            },
            window,
            cx,
        );
        zetta.tab_search = Some(TabSearch {
            tab_id: 7,
            query: TextField::default(),
            generation: 0,
            panes: Vec::new(),
            active_match: None,
            limit_reached: false,
            total_count: 0,
            complete: true,
            task: None,
        });
        zetta.tab_search_focus.focus(window, cx);
        zetta
    });
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(px(400.), px(300.)));
    cx.run_until_parked();
    for (keys, query, selector) in [
        ("n", "n", "tab-search-query:n"),
        ("e", "ne", "tab-search-query:ne"),
        ("backspace", "n", "tab-search-query:n"),
        ("backspace", "", "tab-search-query:"),
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        zetta.read_with(cx, |zetta, _| {
            let search = zetta.tab_search.as_ref().unwrap();
            assert_eq!(search.query.text, query);
            assert_eq!(search.complete, query.is_empty());
        });
        assert!(
            cx.debug_bounds(selector).is_some(),
            "query {query:?} never painted"
        );
    }
    zetta.update(cx, |zetta, cx| {
        let search = zetta.tab_search.as_mut().unwrap();
        search.query = TextField::new("needle");
        search.total_count = 13_107_200;
        search.limit_reached = true;
        search.complete = true;
        cx.notify();
    });
    cx.simulate_resize(gpui::size(px(180.), px(300.)));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("tab-search-query:needle")
            .unwrap()
            .size
            .width
            >= px(80.)
    );
}

#[test]
fn stale_tab_search_work_is_rejected() {
    let search = TabSearch {
        tab_id: 7,
        query: TextField::new("cargo"),
        generation: 4,
        panes: Vec::new(),
        active_match: None,
        limit_reached: false,
        total_count: 0,
        complete: true,
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
        query: TextField::default(),
        generation: 0,
        panes: Vec::new(),
        active_match: None,
        limit_reached: false,
        total_count: 0,
        complete: true,
        task: None,
    };

    assert!(tab_search_targets_tab(Some(&search), 7));
    assert!(!tab_search_targets_tab(Some(&search), 8));
    assert!(!tab_search_targets_tab(None, 7));
}

fn search_over(cx: &mut gpui::TestAppContext, shown: &[usize]) -> TabSearch {
    let panes = shown
        .iter()
        .enumerate()
        .map(|(index, &shown)| TabSearchPane {
            pane_id: index as u64,
            stack_id: None,
            terminal: cx.new(|cx| {
                TerminalBuilder::new_display_only(
                    terminal::terminal_settings::CursorShape::Block,
                    terminal::terminal_settings::AlternateScroll::On,
                    None,
                    0,
                    cx.background_executor(),
                    util::paths::PathStyle::local(),
                )
                .subscribe(cx)
            }),
            shown,
            total_count: shown,
            limit_reached: false,
            complete: false,
            _subscription: None,
        })
        .collect();
    TabSearch {
        tab_id: 7,
        query: TextField::new("cargo"),
        generation: 0,
        panes,
        active_match: None,
        limit_reached: false,
        total_count: 0,
        complete: false,
        task: None,
    }
}

fn pane_match(search: &TabSearch, index: usize) -> Option<(u64, usize)> {
    search
        .match_at(index)
        .map(|search_match| (search_match.pane_id, search_match.match_index))
}

#[gpui::test]
fn tab_search_matches_are_counted_across_panes_in_order(cx: &mut gpui::TestAppContext) {
    let search = search_over(cx, &[2, 0, 3]);

    assert_eq!(search.match_count(), 5);
    assert_eq!(pane_match(&search, 0), Some((0, 0)));
    assert_eq!(pane_match(&search, 1), Some((0, 1)));
    // The empty pane is skipped.
    assert_eq!(pane_match(&search, 2), Some((2, 0)));
    assert_eq!(pane_match(&search, 4), Some((2, 2)));
    assert_eq!(pane_match(&search, 5), None);
}

#[gpui::test]
fn the_active_match_keeps_its_place_as_older_matches_arrive(cx: &mut gpui::TestAppContext) {
    let mut search = search_over(cx, &[2, 3]);
    // The second pane's newest match.
    search.active_match = Some(4);

    // Older matches in the active pane go in front of it.
    assert!(!search.add_pane_matches(1, 10));
    assert_eq!(search.active_match, Some(14));
    assert_eq!(pane_match(&search, 14), Some((1, 12)));

    // So do any in a pane before it.
    assert!(!search.add_pane_matches(0, 5));
    assert_eq!(search.active_match, Some(19));
    assert_eq!(pane_match(&search, 19), Some((1, 12)));

    // A pane after it changes nothing before it.
    search.active_match = Some(3);
    assert!(!search.add_pane_matches(1, 4));
    assert_eq!(search.active_match, Some(3));
    assert_eq!(pane_match(&search, 3), Some((0, 3)));
}

#[gpui::test]
fn the_first_matches_found_become_active(cx: &mut gpui::TestAppContext) {
    let mut search = search_over(cx, &[0, 0]);

    assert!(!search.add_pane_matches(1, 0));
    assert_eq!(search.active_match, None);
    assert!(search.add_pane_matches(1, 3));
    assert_eq!(search.active_match, Some(2));
    assert_eq!(pane_match(&search, 2), Some((1, 2)));
}
