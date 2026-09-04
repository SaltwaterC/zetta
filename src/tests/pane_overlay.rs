use super::*;

/// The hint line is the only place the picker says which keys do what, and each
/// section binds the arrows differently — brightness in `Color`, opacity in
/// `Opacity`. A section that fell back to another's hint would document keys it
/// does not answer to.
#[test]
fn every_picker_section_has_its_own_hint() {
    let hints: Vec<&str> = OverlayPickerSection::ALL
        .iter()
        .copied()
        .map(overlay_picker_hint)
        .collect();
    for hint in &hints {
        assert!(
            hint.contains("Tab switch")
                && hint.contains("Enter apply")
                && hint.contains("Esc cancel"),
            "every section keeps the three keys that leave it: {hint}"
        );
    }
    let mut unique = hints.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        hints.len(),
        "two sections share a hint, so one of them describes keys it does not have"
    );
}

/// `Color` is the section where the arrows move within a colour rather than
/// between options, so it is the one whose hint has to name the axes.
#[test]
fn the_colour_section_documents_the_axes_its_arrows_move() {
    let hint = overlay_picker_hint(OverlayPickerSection::Color);
    for axis in ["saturation", "brightness", "hue", "hex"] {
        assert!(hint.contains(axis), "the colour hint omits {axis}: {hint}");
    }
}

/// Tab walks the sections in a cycle, so every section is reachable from every
/// other one; a section left out of `ALL` would be drawn and never focusable.
#[test]
fn tabbing_through_the_sections_visits_each_one_and_returns() {
    let start = OverlayPickerSection::FontSize;
    let mut section = start;
    let mut visited = Vec::new();
    for _ in 0..OverlayPickerSection::ALL.len() {
        visited.push(section);
        section = section.step(1);
    }
    assert_eq!(section, start, "the walk must return to where it started");
    for expected in OverlayPickerSection::ALL {
        assert!(
            visited.contains(&expected),
            "{expected:?} is never reached by Tab"
        );
    }
}

/// Shift-Tab is the same walk backwards, so it has to undo a Tab rather than
/// follow its own order.
#[test]
fn shift_tab_reverses_the_tab_walk() {
    for section in OverlayPickerSection::ALL {
        assert_eq!(section.step(1).step(-1), section);
        assert_eq!(section.step(-1).step(1), section);
    }
    assert_eq!(
        OverlayPickerSection::FontSize.step(-1),
        *OverlayPickerSection::ALL.last().unwrap(),
        "shift-Tab from the first section wraps to the last"
    );
}

/// The size row is built from `OverlayFontSize::ALL`, and `zetta overlay
/// --size` from `CLI_NAMES`. A size added to one and not the other is either
/// settable and never offered, or offered and unsettable from the CLI.
#[test]
fn the_size_row_and_the_cli_offer_the_same_sizes() {
    assert!(
        OverlayFontSize::ALL.contains(&OverlayFontSize::DEFAULT),
        "the default size has to be one the row can select"
    );
    let offered: Vec<&str> = OverlayFontSize::ALL
        .iter()
        .map(|size| size.cli_name())
        .collect();
    assert_eq!(offered, OverlayFontSize::CLI_NAMES);
    for (index, name) in OverlayFontSize::CLI_NAMES.iter().enumerate() {
        assert_eq!(
            OverlayFontSize::parse(name),
            Some(OverlayFontSize::ALL[index]),
            "{name} does not parse back to the size it names"
        );
    }
}
