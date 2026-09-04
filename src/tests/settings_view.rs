use super::*;

/// Every page the dialog can show, in the order the tab row and the keyboard
/// tab order both use. Spelled out here rather than derived from
/// `SETTINGS_PAGE_TABS`, so a page added to one and not the other fails rather
/// than agreeing with itself.
const EVERY_PAGE: [SettingsPage; 5] = [
    SettingsPage::Configuration,
    SettingsPage::Themes,
    SettingsPage::Keymap,
    SettingsPage::PaneTemplates,
    SettingsPage::Projects,
];

#[test]
fn the_tab_row_offers_every_settings_page_exactly_once() {
    let pages: Vec<SettingsPage> = SETTINGS_PAGE_TABS.iter().map(|(page, ..)| *page).collect();
    assert_eq!(pages, EVERY_PAGE);
}

#[test]
fn every_page_tab_has_a_distinct_element_id_and_a_label() {
    let mut ids: Vec<&str> = SETTINGS_PAGE_TABS.iter().map(|(_, id, _)| *id).collect();
    ids.sort_unstable();
    let unique = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        unique,
        "two tabs share an element id, so GPUI would treat them as one"
    );
    for (_, id, label) in SETTINGS_PAGE_TABS {
        assert!(id.starts_with("settings-"), "unexpected element id: {id}");
        assert!(!label.is_empty());
    }
}
