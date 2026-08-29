use super::*;
use gpui::{Bounds, TestAppContext, UniformListScrollHandle, point, px};

struct DropdownMenuHarness {
    query: &'static str,
    option_count: usize,
    scroll: UniformListScrollHandle,
}

impl Render for DropdownMenuHarness {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let option_count = self.option_count;
        let scroll = self.scroll.clone();
        let option_rows =
            uniform_list("dropdown-options-list", option_count, move |range, _, _| {
                range
                    .map(|index| {
                        div()
                            .id(format!("dropdown-option-{index}"))
                            .h(DROPDOWN_OPTION_ROW_HEIGHT)
                            .px_2()
                            .py_1()
                            .when(index == 0, |row| {
                                row.debug_selector(|| "dropdown-first-option".to_owned())
                            })
                            .when(index == 6, |row| {
                                row.debug_selector(|| "dropdown-seventh-option".to_owned())
                            })
                            .when(index == 7, |row| {
                                row.debug_selector(|| "dropdown-eighth-option".to_owned())
                            })
                            .child(format!("Option {index}"))
                    })
                    .collect::<Vec<_>>()
            })
            .with_sizing_behavior(ListSizingBehavior::Infer)
            .max_h(DROPDOWN_LIST_VIEWPORT_HEIGHT)
            .track_scroll(&scroll)
            .into_any_element();
        let options_region = div()
            .flex_none()
            .max_h(DROPDOWN_OPTIONS_MAX_HEIGHT)
            .p_1()
            .debug_selector(|| "dropdown-options-region".to_owned())
            .child(option_rows);
        let no_matches = option_count == 0;

        let menu = div()
            .id("dropdown-menu")
            .w(px(320.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .when(!self.query.is_empty(), |menu| {
                menu.child(
                    div()
                        .flex_none()
                        .debug_selector(|| "dropdown-search-banner".to_owned())
                        .px_2()
                        .py_1()
                        .child(format!("Search: {}", self.query)),
                )
            })
            .child(if no_matches {
                div()
                    .flex_none()
                    .debug_selector(|| "dropdown-no-matches".to_owned())
                    .p_1()
                    .child("No matches")
                    .into_any_element()
            } else {
                options_region.into_any_element()
            });

        div()
            .size_full()
            .relative()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .size_full()
                    .max_w(px(980.))
                    .max_h(px(680.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().flex_none().h(px(48.)))
                    .child(div().flex_none().h(px(36.)))
                    .child(div().flex_1())
                    .child(deferred(
                        anchored().position(point(px(24.), px(24.))).child(menu),
                    )),
            )
    }
}

struct DropdownMenuGeometry {
    search: Option<Bounds<Pixels>>,
    first_option: Option<Bounds<Pixels>>,
    seventh_option: Option<Bounds<Pixels>>,
    eighth_option: Option<Bounds<Pixels>>,
    no_matches: Option<Bounds<Pixels>>,
    options_region: Option<Bounds<Pixels>>,
    scrollable: bool,
}

fn render_dropdown_menu(
    query: &'static str,
    option_count: usize,
    cx: &mut TestAppContext,
) -> DropdownMenuGeometry {
    let scroll = UniformListScrollHandle::new();
    let scroll_for_view = scroll.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| DropdownMenuHarness {
        query,
        option_count,
        scroll: scroll_for_view,
    });
    cx.run_until_parked();

    DropdownMenuGeometry {
        search: cx.debug_bounds("dropdown-search-banner"),
        first_option: cx.debug_bounds("dropdown-first-option"),
        seventh_option: cx.debug_bounds("dropdown-seventh-option"),
        eighth_option: cx.debug_bounds("dropdown-eighth-option"),
        no_matches: cx.debug_bounds("dropdown-no-matches"),
        options_region: cx.debug_bounds("dropdown-options-region"),
        scrollable: scroll.is_scrollable(),
    }
}

#[gpui::test]
fn dropdown_search_banner_sits_above_a_capped_virtualized_list(cx: &mut TestAppContext) {
    let geometry = render_dropdown_menu("option", 64, cx);
    let search = geometry.search.expect("search banner bounds");
    let first_option = geometry.first_option.expect("first option bounds");
    let options_region = geometry.options_region.expect("options region bounds");

    assert!(
        search.bottom() <= first_option.top(),
        "the search banner must not overlap the first option: {search:?} vs {first_option:?}"
    );
    assert!(
        options_region.size.height <= px(260.),
        "the options list must retain its 260px height cap: {:?}",
        geometry.options_region
    );
    assert!(
        geometry.scrollable,
        "a long options list must remain scrollable"
    );
    assert!(
        geometry
            .seventh_option
            .expect("seventh visible option bounds")
            .bottom()
            <= options_region.bottom(),
        "the last visible option must be fully inside the capped region: {:?} vs {options_region:?}",
        geometry.seventh_option
    );
    assert!(
        geometry.eighth_option.is_none(),
        "the capped list must not render a partial eighth option: {:?}",
        geometry.eighth_option
    );
}

#[gpui::test]
fn dropdown_search_layout_handles_no_search_and_no_matches(cx: &mut TestAppContext) {
    let no_search = render_dropdown_menu("", 64, cx);
    assert!(
        no_search.search.is_none(),
        "an empty query must not render a search banner"
    );
    assert!(
        no_search.first_option.is_some(),
        "options must still render without a search query"
    );
    assert!(
        no_search
            .options_region
            .expect("options region bounds")
            .size
            .height
            <= px(260.),
        "an unfiltered options list must retain its 260px height cap: {:?}",
        no_search.options_region
    );
    assert!(
        no_search.scrollable,
        "an unfiltered long list must remain scrollable"
    );
    assert!(
        no_search.eighth_option.is_none(),
        "an unfiltered capped list must not render a partial option: {:?}",
        no_search.eighth_option
    );

    let short_search = render_dropdown_menu("option", 2, cx);
    assert!(
        short_search
            .options_region
            .expect("options region bounds")
            .size
            .height
            < px(260.),
        "a short filtered list should size to its contents instead of leaving a capped gap: {:?}",
        short_search.options_region
    );

    let no_matches = render_dropdown_menu("missing", 0, cx);
    let search = no_matches.search.expect("search banner bounds");
    let empty_state = no_matches.no_matches.expect("no-match message bounds");
    assert!(
        search.bottom() <= empty_state.top(),
        "the no-match message must remain below the search banner: {search:?} vs {empty_state:?}"
    );
    assert!(
        no_matches.first_option.is_none(),
        "no-match state must not render an option row"
    );
}
