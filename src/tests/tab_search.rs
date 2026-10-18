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
        zetta.tab_search = Some(TabSearch::new(7, cx));
        zetta.tab_search_focus.focus(window, cx);
        zetta
    });
    cx.run_until_parked();
    zetta.read_with(cx, |zetta, _| {
        assert!(tab_search_targets_tab(zetta.tab_search.as_ref(), 7));
        assert!(!tab_search_targets_tab(zetta.tab_search.as_ref(), 8));
        assert!(!tab_search_targets_tab(None, 7));
    });
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
            assert_eq!(search.status.total_count, 0);
        });
        assert!(
            cx.debug_bounds(selector).is_some(),
            "query {query:?} never painted"
        );
    }
    zetta.update(cx, |zetta, cx| {
        let search = zetta.tab_search.as_mut().unwrap();
        search.query = TextField::new("needle");
        search.status.total_count = 13_107_200;
        search.status.limit_reached = true;
        search.status.complete = true;
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
