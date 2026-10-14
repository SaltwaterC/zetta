use super::*;

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
    for index in 0..16 {
        assert_eq!(ansi_index(legacy_index(index)), index);
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
    assert_eq!(unprocessed(&utf16("a\r\nb\x1b[1m")), "a♪◙b←[1m");
    assert_eq!(unprocessed(&utf16("\0\x7f\u{85}é🙂")), " ⌂?é🙂");
    assert_eq!(unprocessed(&[0xd800, 0x41]), "\u{fffd}A");
}

#[test]
fn processed_legacy_output_acts_on_bs_tab_cr_lf_and_bell_only() {
    let oem = |byte: u8| (byte == 0x1b).then_some('←');
    assert_eq!(processed(&utf16("a\nb\r\nc"), true, oem), "a\r\nb\r\nc");
    assert_eq!(processed(&utf16("a\nb"), false, oem), "a\nb");
    assert_eq!(
        processed(&utf16("\x1b[31m\x07\x08\t\0"), true, oem),
        "←[31m\x07\x08\t "
    );
    // A code page without glyphs for controls (UTF-8) drops them.
    assert_eq!(processed(&utf16("x\x1by"), true, |_| None), "xy");
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
    // An exact palette color maps to its own entry: Campbell's bright yellow.
    assert_eq!(
        attributes(
            &pen(PenColor::Rgb(0xf9, 0xf1, 0xa5), PenColor::Default),
            &palette
        ),
        0x0e
    );
    // xterm 196 is pure red, nearer Campbell's red (197,15,31) than its
    // bright red (231,72,86).
    assert_eq!(
        attributes(&pen(PenColor::Indexed(196), PenColor::Default), &palette),
        0x04
    );
    let mut reversed = pen(PenColor::Default, PenColor::Default);
    reversed.reverse = true;
    reversed.underline = true;
    assert_eq!(attributes(&reversed, &palette), 0xc007);
}
