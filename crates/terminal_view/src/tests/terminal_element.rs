use super::*;
use gpui::Hsla;

#[test]
fn sextant_glyphs_are_decorative_and_decode_block_element_gaps() {
    assert!(TerminalElement::is_decorative_character('\u{1FB00}'));
    assert!(TerminalElement::is_decorative_character('\u{1FB3B}'));
    assert!(!TerminalElement::is_decorative_character('\u{1FB3C}'));

    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB00}'),
        Some(0b00_0001)
    );
    // U+1FB13 and U+1FB14 sit on either side of the gap for `▌`.
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB13}'),
        Some(0b01_0100)
    );
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB14}'),
        Some(0b01_0110)
    );
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB3B}'),
        Some(0b11_1110)
    );
    assert_eq!(TerminalElement::sextant_char_to_filled_bits('█'), None);
    assert_eq!(
        TerminalElement::sextant_char_to_filled_bits('\u{1FB3C}'),
        None
    );
}

#[test]
fn block_element_rects_merge_full_cell_runs_in_both_axes() {
    let color = Hsla::default();
    let mut regions = Vec::new();
    for line in 0..2 {
        for column in 0..2 {
            assert!(TerminalElement::collect_block_element_regions(
                LayoutPoint::new(line, column),
                '█',
                color,
                &mut regions,
            ));
        }
    }

    let rects = TerminalElement::block_element_regions_to_rects(regions);
    assert_eq!(rects.len(), 1);
    assert_eq!(rects[0].point.line, 0);
    assert_eq!(rects[0].point.column, 0);
    assert_eq!(rects[0].num_of_columns, 16);
    assert_eq!(rects[0].num_of_lines, 48);
}

#[test]
fn block_element_subcell_geometry_matches_unicode_forms() {
    let color = Hsla::default();

    for (ch, expected_column, expected_line, expected_columns, expected_lines) in [
        ('\u{2581}', 0, 21, 8, 3),
        ('\u{2587}', 0, 3, 8, 21),
        ('\u{258F}', 0, 0, 1, 24),
        ('\u{2595}', 7, 0, 1, 24),
    ] {
        let mut regions = Vec::new();
        assert!(TerminalElement::collect_block_element_regions(
            LayoutPoint::new(0, 0),
            ch,
            color,
            &mut regions,
        ));

        let rects = TerminalElement::block_element_regions_to_rects(regions);
        assert_eq!(rects.len(), 1, "unexpected rect count for {ch}");
        assert_eq!(rects[0].point.column, expected_column, "column for {ch}");
        assert_eq!(rects[0].point.line, expected_line, "line for {ch}");
        assert_eq!(rects[0].num_of_columns, expected_columns, "width for {ch}");
        assert_eq!(rects[0].num_of_lines, expected_lines, "height for {ch}");
    }

    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '\u{259A}',
        color,
        &mut regions,
    ));
    let quadrants = TerminalElement::block_element_regions_to_rects(regions);
    assert_eq!(quadrants.len(), 2);
    assert!(quadrants.iter().any(|rect| {
        rect.point.column == 0
            && rect.point.line == 0
            && rect.num_of_columns == 4
            && rect.num_of_lines == 12
    }));
    assert!(quadrants.iter().any(|rect| {
        rect.point.column == 4
            && rect.point.line == 12
            && rect.num_of_columns == 4
            && rect.num_of_lines == 12
    }));

    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '\u{2592}',
        color,
        &mut regions,
    ));
    let shades = TerminalElement::block_element_regions_to_rects(regions);
    assert_eq!(shades.len(), 1);
    assert_eq!(shades[0].num_of_columns, 8);
    assert_eq!(shades[0].num_of_lines, 24);
    assert_eq!(shades[0].color, color.opacity(0.5));

    let mut regions = Vec::new();
    assert!(TerminalElement::collect_block_element_regions(
        LayoutPoint::new(0, 0),
        '\u{1FB00}',
        color,
        &mut regions,
    ));
    let sextants = TerminalElement::block_element_regions_to_rects(regions);
    assert_eq!(sextants.len(), 1);
    assert_eq!(sextants[0].point.column, 0);
    assert_eq!(sextants[0].point.line, 0);
    assert_eq!(sextants[0].num_of_columns, 4);
    assert_eq!(sextants[0].num_of_lines, 8);
}

#[test]
fn block_element_glyphs_stay_inside_one_cell() {
    let color = Hsla::default();

    for codepoint in (0x2580..=0x259F).chain(0x1FB00..=0x1FB3B) {
        let ch = char::from_u32(codepoint).expect("block element codepoint is valid Unicode");
        let mut regions = Vec::new();
        assert!(
            TerminalElement::collect_block_element_regions(
                LayoutPoint::new(0, 0),
                ch,
                color,
                &mut regions,
            ),
            "U+{codepoint:04X} {ch} should use custom painting"
        );
        assert!(
            !regions.is_empty(),
            "U+{codepoint:04X} {ch} should cover at least one subcell"
        );

        let mut filled = [[false; BLOCK_SUBCELL_COLUMNS as usize]; BLOCK_SUBCELL_LINES as usize];
        for region in regions {
            assert!(
                (0..BLOCK_SUBCELL_LINES).contains(&region.start_line)
                    && (0..BLOCK_SUBCELL_LINES).contains(&region.end_line)
                    && (0..BLOCK_SUBCELL_COLUMNS).contains(&region.start_col)
                    && (0..BLOCK_SUBCELL_COLUMNS).contains(&region.end_col),
                "U+{codepoint:04X} {ch} paints outside its cell: {region:?}"
            );
            for line in region.start_line..=region.end_line {
                for column in region.start_col..=region.end_col {
                    assert!(
                        !filled[line as usize][column as usize],
                        "U+{codepoint:04X} {ch} paints subcell ({line}, {column}) twice"
                    );
                    filled[line as usize][column as usize] = true;
                }
            }
        }
    }
}
