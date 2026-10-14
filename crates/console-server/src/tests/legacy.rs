use super::*;

fn unknown() -> Column {
    Column::UNKNOWN
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn legacy_and_ansi_indices_swap_red_and_blue_only() {
    // FOREGROUND_BLUE is ANSI blue (4), FOREGROUND_RED is ANSI red (1).
    assert_eq!(ansi_index(0x1), 4);
    assert_eq!(ansi_index(0x4), 1);
    assert_eq!(ansi_index(0x2), 2);
    assert_eq!(ansi_index(0xc), 9);
    // conhost's 256-color table starts with the same swap, inverted.
    for index in 0..16u8 {
        let legacy = u16::from(INDEX256_TO_INDEX16[usize::from(index)]);
        assert_eq!(ansi_index(legacy), index);
    }
}

#[test]
fn attributes_equal_to_the_defaults_keep_the_terminal_default_colors() {
    assert_eq!(sgr(0x07, 0x07), "\x1b[0m");
    // Bright red on default background, as Write-Host -ForegroundColor Red.
    assert_eq!(sgr(0x0c, 0x07), "\x1b[0;91m");
    // cmd's `color 1f`: bright white on blue.
    assert_eq!(sgr(0x1f, 0x07), "\x1b[0;97;44m");
    assert_eq!(sgr(0x4007, 0x07), "\x1b[0;7m");
    // A pane whose defaults are not 7/0 maps its own pair to default colors.
    assert_eq!(sgr(0x1f, 0x1f), "\x1b[0m");
    assert_eq!(sgr(0x07, 0x1f), "\x1b[0;37;40m");
}

#[test]
fn unprocessed_output_shows_controls_as_glyphs_never_as_controls() {
    assert_eq!(
        unprocessed(&utf16("a\r\nb\x1b[1m"), &mut unknown()),
        "a♪◙b←[1m"
    );
    assert_eq!(
        unprocessed(&utf16("\0\x7f\u{85}é🙂"), &mut unknown()),
        " ⌂?é🙂"
    );
    assert_eq!(unprocessed(&[0xd800, 0x41], &mut unknown()), "\u{fffd}A");
}

#[test]
fn processed_legacy_output_acts_on_bs_tab_cr_lf_and_bell_only() {
    let oem = |byte: u8| (byte == 0x1b).then_some('←');
    assert_eq!(
        processed(&utf16("a\nb\r\nc"), true, &mut unknown(), oem),
        "a\r\nb\r\nc"
    );
    assert_eq!(
        processed(&utf16("a\nb"), false, &mut unknown(), oem),
        "a\nb"
    );
    assert_eq!(
        processed(&utf16("\x1b[31m\x07\x08\t\0"), true, &mut unknown(), oem),
        "←[31m\x07\x08\t "
    );
    // A code page without glyphs for controls (UTF-8) drops them.
    assert_eq!(
        processed(&utf16("x\x1by"), true, &mut unknown(), |_| None),
        "xy"
    );
}

#[test]
fn reported_attributes_follow_the_console_defaults_and_palette() {
    let palette = Palette::default();
    let pen = |foreground, background| Pen {
        foreground,
        background,
        reverse: false,
        underline: false,
    };
    assert_eq!(
        attributes(&pen(PenColor::Default, PenColor::Default), &palette),
        0x07
    );
    // ANSI bright red is legacy FOREGROUND_RED | FOREGROUND_INTENSITY.
    assert_eq!(
        attributes(&pen(PenColor::Indexed(9), PenColor::Indexed(4)), &palette),
        0x1c
    );
    // RGB compresses to RRRGGGBB and goes through conhost's table:
    // Campbell's bright yellow comes back as bright yellow.
    assert_eq!(
        attributes(
            &pen(PenColor::Rgb(0xf9, 0xf1, 0xa5), PenColor::Default),
            &palette
        ),
        0x0e
    );
    // Indexed colors use conhost's table, not the palette: xterm 196 is
    // bright red, and 33 (0,135,255) is dark cyan, as ft_host's
    // TestVT256Grid expects.
    assert_eq!(
        attributes(
            &pen(PenColor::Indexed(196), PenColor::Indexed(33)),
            &palette
        ),
        0x3c
    );
    let mut reversed = pen(PenColor::Default, PenColor::Default);
    reversed.reverse = true;
    reversed.underline = true;
    assert_eq!(attributes(&reversed, &palette), 0xc007);
}

#[test]
fn a_processed_tab_writes_spaces_to_the_next_stop_within_the_row() {
    let oem = |_: u8| None;
    let at = |at| Column {
        at,
        width: 80,
        wrap: false,
    };
    // "yx", back over the x, then a tab: the x is overwritten.
    assert_eq!(
        processed(&utf16("yx\x08\t|"), true, &mut at(1), oem),
        "yx\x08      |"
    );
    assert_eq!(processed(&utf16("\t"), true, &mut at(8), oem), "        ");
    // Never past the right edge.
    let mut column = at(78);
    assert_eq!(processed(&utf16("\t"), true, &mut column, oem), "  ");
    assert_eq!(column.at, 80);
    // A carriage return starts the count again.
    assert_eq!(
        processed(&utf16("abc\r\t"), true, &mut at(0), oem),
        "abc\r        "
    );
}

#[test]
fn legacy_text_ending_in_the_last_column_wraps_at_once_as_conpty_writes_it() {
    let oem = |_: u8| None;
    let edge = |wrap| Column {
        at: 3,
        width: 4,
        wrap,
    };
    // The run ends in the last column: the console is already on the next line.
    let mut column = edge(true);
    assert_eq!(processed(&utf16("Y"), true, &mut column, oem), "Y\r\n");
    assert_eq!(column.at, 0);
    // A run that carries on past the edge is the terminal's own wrap...
    let mut column = edge(true);
    assert_eq!(processed(&utf16("YZ"), true, &mut column, oem), "YZ");
    assert_eq!(column.at, 1);
    // ...but a control after the last column finds the console wrapped.
    let mut column = edge(true);
    assert_eq!(
        processed(&utf16("Y\r\n"), true, &mut column, oem),
        "Y\r\n\r\n"
    );
    // Without wrap the cursor stays in the last column.
    let mut column = edge(false);
    assert_eq!(processed(&utf16("YZ"), true, &mut column, oem), "YZ");
    assert_eq!(column.at, 4);
    let mut column = edge(true);
    assert_eq!(unprocessed(&utf16("Y"), &mut column), "Y\r\n");
}
