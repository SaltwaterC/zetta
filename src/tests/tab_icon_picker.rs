use super::*;

#[test]
fn icon_search_is_case_insensitive_and_matches_icon_names() {
    assert!(matching_tab_icons("TERMINAL").contains(&IconName::Terminal));
    assert!(matching_tab_icons("arrow").contains(&IconName::ArrowLeft));
    assert!(!matching_tab_icons("not-an-icon").contains(&IconName::Terminal));
}

#[test]
fn empty_icon_search_returns_every_icon() {
    assert_eq!(
        matching_tab_icons("").len(),
        <IconName as strum::IntoEnumIterator>::iter().count()
    );
}

#[test]
fn icon_options_include_none_and_filter_it_by_name() {
    assert_eq!(matching_tab_icon_options("none"), vec![None]);
    assert!(matching_tab_icon_options("").contains(&None));
    assert!(!matching_tab_icon_options("terminal").contains(&None));
}

#[test]
fn cli_icon_names_are_snake_case_and_include_none() {
    let names = tab_icon_completion_names().collect::<Vec<_>>();
    assert_eq!(names.first(), Some(&"none"));
    assert!(names.contains(&"terminal"));
    assert_eq!(parse_tab_icon_name("terminal"), Some(IconName::Terminal));
    assert_eq!(parse_tab_icon_name("Terminal"), Some(IconName::Terminal));
    assert_eq!(parse_tab_icon_name("not-an-icon"), None);
}
