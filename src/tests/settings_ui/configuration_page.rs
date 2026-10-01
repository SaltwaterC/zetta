use super::*;
use crate::settings_ui::controls::tests::configuration_editor;

fn editor() -> SettingsEditor {
    configuration_editor(
        &Config::parse(
            r#"{"profiles":[{"name":"Toolbox","program":"/bin/sh"}]}"#,
            None,
            None,
        )
        .unwrap(),
    )
}

/// The page and its tab order are built from one list, so this is what keeps
/// a new row from being drawn but unreachable, or reachable but not drawn.
#[test]
fn every_drawn_row_is_one_tab_stop_in_draw_order() {
    let editor = editor();
    let layout = configuration_layout(&editor);
    let controls = configuration_controls(&editor);

    let row_controls = layout
        .iter()
        .filter_map(|item| match item {
            ConfigurationItem::Row(row) => Some(row.control()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut positions = row_controls.iter().map(|control| {
        controls
            .iter()
            .position(|candidate| candidate == control)
            .unwrap_or_else(|| panic!("{control:?} is drawn but not in the tab order"))
    });
    let mut previous = positions.next().unwrap();
    for position in positions {
        assert!(position > previous, "the tab order follows the draw order");
        previous = position;
    }
    let mut unique = controls.clone();
    unique.sort_by_key(|control| format!("{control:?}"));
    unique.dedup();
    assert_eq!(
        unique.len(),
        controls.len(),
        "no control is a tab stop twice"
    );
}

/// Two of the page's groups used to have no heading, so the window rows read
/// as part of the background-session section above them.
#[test]
fn every_row_sits_under_a_heading() {
    let layout = configuration_layout(&editor());
    assert!(matches!(
        layout.first(),
        Some(ConfigurationItem::Heading(_))
    ));
    let headings = layout
        .iter()
        .filter(|item| matches!(item, ConfigurationItem::Heading(_)))
        .count();
    assert!(headings >= 6, "{headings} headings");
}

/// Sentence case: after the first word, only names and acronyms are
/// capitalised.
#[test]
fn row_labels_are_sentence_case_and_every_row_is_described() {
    const PROPER: &[&str] = &[
        "SSH", "HTTP", "TFTP", "TCP", "UDP", "macOS", "Focus", "Zosh", "Mosh", "OpenSSH",
    ];
    for item in configuration_layout(&editor()) {
        let ConfigurationItem::Row(row) = item else {
            continue;
        };
        assert!(!row.description().is_empty(), "{row:?} has no description");
        for word in row.label().split_whitespace().skip(1) {
            assert!(
                PROPER.contains(&word) || !word.starts_with(char::is_uppercase),
                "{:?} is not sentence case",
                row.label()
            );
        }
    }
}
