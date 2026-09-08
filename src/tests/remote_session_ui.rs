use super::*;
use gpui::{TestAppContext, UniformListScrollHandle, px, size};

struct RemoteSessionSuggestionsHarness {
    suggestions: Vec<String>,
    scroll: UniformListScrollHandle,
    selected: Option<usize>,
}

impl Render for RemoteSessionSuggestionsHarness {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let suggestions = remote_session_suggestion_rows(
            &gpui::WeakEntity::<Zetta>::new_invalid(),
            &ThemeColors::light(),
            &self.scroll,
            self.suggestions.clone(),
            self.selected,
        );
        let suggestions = remote_session_suggestion_list(suggestions, &self.scroll, window, cx);
        div().size_full().child(
            div()
                .w(px(320.))
                .flex_none()
                .debug_selector(|| "remote-session-suggestions-region".to_owned())
                .child(suggestions),
        )
    }
}

fn picker_with_suggestions(target: &str) -> RemoteSessionPicker {
    RemoteSessionPicker {
        target: TextField::new(target),
        suggestions: vec![
            "production".to_owned(),
            "prod-west".to_owned(),
            "prod-staging".to_owned(),
            "staging".to_owned(),
        ],
        ..Default::default()
    }
}

#[test]
fn remote_picker_starts_on_the_target_field() {
    let picker = RemoteSessionPicker::default();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert!(picker.target.text.is_empty());
    assert!(picker.port.text.is_empty());
    assert!(picker.sessions.is_empty());
    assert!(!picker.loading);
    assert!(picker.suggestion_navigation.is_none());
}

#[test]
fn down_selects_the_first_host_without_leaving_the_target_field() {
    let mut picker = picker_with_suggestions("");

    assert!(picker.navigate_suggestions(false));
    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(picker.target.text, "production");
    assert_eq!(picker.suggestion_navigation.as_ref().unwrap().selected, 0);
    assert!(!picker.visible_suggestions().is_empty());
}

#[test]
fn suggestion_navigation_wraps_through_matching_hosts() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(false);
    assert_eq!(picker.target.text, "prod-west");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "production");
    picker.navigate_suggestions(true);
    assert_eq!(picker.target.text, "prod-staging");
}

#[test]
fn suggestion_navigation_preserves_the_original_filter() {
    let mut picker = picker_with_suggestions("prod");

    picker.navigate_suggestions(false);

    assert_eq!(picker.target.text, "production");
    assert_eq!(
        picker.suggestion_navigation.as_ref().unwrap().filter,
        "prod"
    );
    assert_eq!(
        picker.visible_suggestions(),
        vec!["production", "prod-west", "prod-staging"]
    );
}

#[test]
fn editing_the_target_resets_suggestion_navigation() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);
    picker
        .suggestion_scroll
        .scroll_to_item(3, ScrollStrategy::Top);

    picker.target = TextField::new("productionx");
    picker.reset_suggestion_navigation();

    assert!(picker.suggestion_navigation.is_none());
    assert!(picker.visible_suggestions().is_empty());
    assert_eq!(picker.suggestion_scroll.logical_scroll_top_index(), 0);
}

#[gpui::test]
fn ssh_host_suggestions_keep_all_aliases_in_a_six_row_viewport(cx: &mut TestAppContext) {
    cx.update(|cx| theme::init(theme::LoadThemes::JustBase, cx));
    let aliases = (0..12)
        .map(|index| format!("host-{index}"))
        .collect::<Vec<_>>();
    let picker = RemoteSessionPicker {
        suggestions: aliases.clone(),
        ..Default::default()
    };
    assert_eq!(picker.visible_suggestions(), aliases);

    let scroll = UniformListScrollHandle::new();
    let scroll_for_view = scroll.clone();
    let aliases_for_view = picker.suggestions.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| RemoteSessionSuggestionsHarness {
        suggestions: aliases_for_view,
        scroll: scroll_for_view,
        selected: None,
    });
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.run_until_parked();

    let region = cx
        .debug_bounds("remote-session-suggestions-region")
        .expect("the SSH suggestion list should be laid out");
    assert!(
        region.size.height <= px(168.),
        "the suggestion viewport must be capped at six rows: {region:?}"
    );
    assert!(
        scroll.is_scrollable(),
        "more than six SSH aliases must make the suggestion list scrollable"
    );
    let scrollbar = cx
        .debug_bounds("remote-session-suggestions-scrollbar")
        .expect("the SSH suggestion list should render a scrollbar layer");
    assert!(
        scrollbar.size.height > px(0.),
        "the SSH suggestion scrollbar should have visible bounds: {scrollbar:?}"
    );
    assert!(
        cx.debug_bounds("remote-session-suggestion-5").is_some(),
        "the sixth alias should fit in the initial viewport"
    );
    assert!(
        cx.debug_bounds("remote-session-suggestion-6").is_none(),
        "the seventh alias should be virtualized outside the initial viewport"
    );

    scroll.scroll_to_item(11, ScrollStrategy::Bottom);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();

    let last_alias = cx
        .debug_bounds("remote-session-suggestion-11")
        .expect("the last alias should be reachable at the end of the list");
    assert!(
        last_alias.bottom() <= region.bottom(),
        "the last alias should be inside the suggestion viewport: {last_alias:?} vs {region:?}"
    );
    assert_eq!(scroll.is_scrolled_to_end(), Some(true));
}

#[gpui::test]
fn keyboard_suggestion_navigation_reveals_the_selected_alias(cx: &mut TestAppContext) {
    cx.update(|cx| theme::init(theme::LoadThemes::JustBase, cx));
    let aliases = (0..12)
        .map(|index| format!("host-{index}"))
        .collect::<Vec<_>>();
    let mut picker = RemoteSessionPicker {
        suggestions: aliases.clone(),
        ..Default::default()
    };
    for _ in 0..8 {
        assert!(picker.navigate_suggestions(false));
    }
    let selected = picker
        .suggestion_navigation
        .as_ref()
        .expect("keyboard navigation should select an alias")
        .selected;
    assert_eq!(selected, 7);

    let scroll = picker.suggestion_scroll.clone();
    let (_view, cx) = cx.add_window_view(move |_, _| RemoteSessionSuggestionsHarness {
        suggestions: aliases,
        scroll,
        selected: Some(selected),
    });
    cx.simulate_resize(size(px(400.), px(300.)));
    cx.run_until_parked();

    let region = cx
        .debug_bounds("remote-session-suggestions-region")
        .expect("the SSH suggestion list should be laid out");
    let selected_alias = cx
        .debug_bounds("remote-session-suggestion-7")
        .expect("keyboard navigation should scroll the selected alias into view");
    assert!(
        selected_alias.origin.y >= region.origin.y && selected_alias.bottom() <= region.bottom(),
        "the keyboard-selected alias should be inside the suggestion viewport: {selected_alias:?} vs {region:?}"
    );
}

#[test]
fn target_navigation_keeps_the_current_value_for_enter_to_load() {
    let mut picker = picker_with_suggestions("prod");
    picker.navigate_suggestions(false);

    let target = Zetta::remote_target_from_picker(&picker).unwrap();

    assert_eq!(picker.field, RemoteSessionField::Target);
    assert_eq!(target.destination(), "production");
}

#[test]
fn remote_picker_parses_optional_ports_and_rejects_invalid_values() {
    let mut picker = RemoteSessionPicker {
        target: TextField::new("dev.example"),
        ..Default::default()
    };

    let target = Zetta::remote_target_from_picker(&picker).unwrap();
    assert_eq!(target.destination(), "dev.example");
    assert_eq!(target.port(), None);

    picker.port = TextField::new("2200");
    assert_eq!(
        Zetta::remote_target_from_picker(&picker).unwrap().port(),
        Some(2200)
    );

    picker.port = TextField::new("0");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
    picker.port = TextField::new("not-a-port");
    assert!(Zetta::remote_target_from_picker(&picker).is_err());
}
