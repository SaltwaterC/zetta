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
fn virtualized_grid_uses_row_indices_at_column_boundaries() {
    assert_eq!(tab_icon_row(0), 0);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS - 1), 0);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS), 1);
    assert_eq!(tab_icon_row(TAB_ICON_COLUMNS * 3 + 2), 3);
}

#[test]
fn picker_filters_cached_entries_without_rebuilding_labels() {
    let entries = build_icon_entries(&[IconName::Terminal, IconName::Folder]).into();
    let mut picker =
        TabIconPicker::new(TabIconPickerTarget::Tab(0), Some(IconName::Folder), entries);

    assert_eq!(picker.selected, 2);
    assert_eq!(picker.entries[0].label.as_ref(), "Terminal");
    assert_eq!(picker.entries[0].search_label, "terminal");

    let options = picker.options();
    assert_eq!(options.as_ref(), &[None, Some(0), Some(1)]);
    assert_eq!(picker.icon_for_option(options[0]), None);
    assert_eq!(picker.icon_for_option(options[2]), Some(IconName::Folder));

    picker.query.text = "folder".to_owned();
    let filtered = picker.options();
    assert_eq!(filtered.as_ref(), &[Some(1)]);
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
