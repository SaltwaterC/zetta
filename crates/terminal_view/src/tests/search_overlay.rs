use super::*;
use crate::tests::link_test_view;

#[gpui::test]
async fn search_query_repaints_before_results_after_window_resize(cx: &mut gpui::TestAppContext) {
    let (view, cx) = link_test_view(cx);
    cx.run_until_parked();
    cx.simulate_resize(gpui::size(px(400.), px(300.)));
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.search_scrollback(&SearchScrollback, window, cx);
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("terminal-search-query:").is_some());

    // Do not advance the 75 ms search debounce: these frames must come from
    // editing the field itself, with no search result or terminal output to
    // trigger a repaint. Check rendered elements, not just the backing String.
    for (keys, query, selector) in [
        ("n", "n", "terminal-search-query:n"),
        ("e", "ne", "terminal-search-query:ne"),
        ("backspace", "n", "terminal-search-query:n"),
        ("left delete", "", "terminal-search-query:"),
        ("\u{e9}", "\u{e9}", "terminal-search-query:\u{e9}"),
    ] {
        cx.simulate_keystrokes(keys);
        cx.run_until_parked();
        view.update(cx, |view, _| {
            assert_eq!(view.search_query.as_deref(), Some(query));
            assert_eq!(view.search_status.complete, query.is_empty());
            assert_eq!(view.search_status.total_count, 0);
        });
        assert!(
            cx.debug_bounds(selector).is_some(),
            "field did not repaint {query:?}"
        );
    }
}

#[gpui::test]
async fn search_query_keeps_space_beside_large_counts_after_narrowing(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx) = link_test_view(cx);
    cx.run_until_parked();
    view.update_in(cx, |view, window, cx| {
        view.search_scrollback(&SearchScrollback, window, cx);
        view.insert_search_text("needle", cx);
        view.search_status = SearchStatus {
            active_match: None,
            shown: 0,
            total_count: 13_107_200,
            limit_reached: true,
            complete: true,
        };
        cx.notify();
    });
    cx.run_until_parked();
    for width in [180., 320., 800.] {
        cx.simulate_resize(gpui::size(px(width), px(300.)));
        cx.run_until_parked();
        let bounds = cx.debug_bounds("terminal-search-query:needle").unwrap();
        assert!(
            bounds.size.width >= px(80.),
            "counter squeezed the query out at {width}px: {bounds:?}"
        );
    }
}
